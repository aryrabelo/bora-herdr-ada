#!/usr/bin/env python3
"""Deterministic PR review: checks the pushed diff against binding AGENTS.md rules.

Why deterministic instead of a model: measured over one review session, these
four checks caught 4 of 5 real defects at near-zero cost; the model reviewer
(scripts/independent_review.sh, now removed) caught 1 at several minutes and
real API cost per push. The checks below are diff-scoped rules a model could
also apply, just without the latency or the flakiness.

Each check is a small pure function over parsed git data (name-status list,
added-lines-by-path, commit list) so it can be unit tested with synthetic
input instead of a real git repo. main() does the git plumbing and wiring.

Findings BLOCK the merge (exit 1): unlike the model reviewer, these are
violations of written rules, not opinions, so there is nothing to weigh.
"""

from __future__ import annotations

import collections
import os
import re
import subprocess
import sys
from typing import NamedTuple

RECORD_SEP = "\x1e"
FIELD_SEP = "\x1f"  # not NUL: subprocess argv cannot contain embedded null bytes

SEVERITY_ORDER = ("critical", "high", "medium", "low")

GENERATED_PATH_PREFIXES = (
    "docs/versions/",
    "docs/preview/",
    "website/src/content/docs/",
)
GENERATED_PATH_EXACT = (
    "website/latest.json",
    "website/preview.json",
)

PACKAGE_SECTION_RE = re.compile(r"^\s*\[package\]\s*$")
SECTION_HEADER_RE = re.compile(r"^\s*\[.*\]\s*$")
VERSION_LINE_RE = re.compile(r'^\s*version\s*=\s*"([^"]*)"')

HUNK_HEADER_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(?P<new_start>\d+)(?:,\d+)? @@")

ALLOW_ATTR_RE = re.compile(r"#!?\[allow\(")

# Longest/most-specific alternatives first so a keyword never partially
# matches a longer one (e.g. "close" inside "closed") and needs no backtrack.
CLOSING_KEYWORD_RE = re.compile(
    r"\b(closes|closed|close|fixes|fixed|fix|resolves|resolved|resolve)\b"
    r"\s*(#\d+|https://github\.com/[\w.\-]+/[\w.\-]+/issues/\d+)",
    re.IGNORECASE,
)


class Finding(NamedTuple):
    severity: str  # one of SEVERITY_ORDER
    location: str  # path or commit sha
    message: str


class Commit(NamedTuple):
    sha: str
    subject: str
    body: str


class GitError(RuntimeError):
    pass


# ── parsing (pure) ──────────────────────────────────────────────────────


def parse_name_status(text: str) -> list[tuple[str, str]]:
    """Parse `git diff --name-status` output into (status, path) pairs.

    Renames/copies (R###/C###) carry old and new path; the new path is what
    matters for "does this touch X".
    """
    entries: list[tuple[str, str]] = []
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        status = parts[0]
        path = parts[2] if status.startswith(("R", "C")) else parts[1]
        entries.append((status, path))
    return entries


def parse_added_lines(diff_text: str) -> dict[str, list[tuple[int, str]]]:
    """Parse a `git diff --unified=0` into path -> [(new_lineno, line_text)] for `+` lines."""
    added: dict[str, list[tuple[int, str]]] = {}
    current_path: str | None = None
    new_lineno: int | None = None
    for line in diff_text.splitlines():
        if line.startswith("diff --git "):
            current_path = None
            new_lineno = None
        elif line.startswith("+++ "):
            target = line[4:]
            current_path = None if target == "/dev/null" else target.removeprefix("b/")
        elif line.startswith("@@"):
            match = HUNK_HEADER_RE.match(line)
            new_lineno = int(match.group("new_start")) if match and current_path else None
        elif current_path is not None and new_lineno is not None:
            if line.startswith("+") and not line.startswith("+++"):
                added.setdefault(current_path, []).append((new_lineno, line[1:]))
                new_lineno += 1
            elif line.startswith("-") and not line.startswith("---"):
                pass  # removed line: does not consume a new-file line number
    return added


def parse_commits(log_text: str) -> list[Commit]:
    """Parse `git log --format=%x1e%H%x1f%s%x1f%b` output into commits."""
    commits = []
    for record in log_text.split(RECORD_SEP):
        if not record.strip():
            continue
        fields = record.split(FIELD_SEP)
        sha = fields[0].strip()
        subject = fields[1] if len(fields) > 1 else ""
        body = fields[2].rstrip("\n") if len(fields) > 2 else ""
        commits.append(Commit(sha=sha, subject=subject, body=body))
    return commits


def extract_package_version(toml_text: str | None) -> str | None:
    """Return the `version` under `[package]` only, ignoring every other table."""
    if toml_text is None:
        return None
    in_package = False
    for line in toml_text.splitlines():
        if PACKAGE_SECTION_RE.match(line):
            in_package = True
            continue
        if SECTION_HEADER_RE.match(line):
            in_package = False
            continue
        if in_package:
            match = VERSION_LINE_RE.match(line)
            if match:
                return match.group(1)
    return None


# ── checks (pure) ───────────────────────────────────────────────────────


def check_version_bump(changed_paths: list[str], base_cargo_toml: str | None, head_cargo_toml: str | None) -> list[Finding]:
    def touches_versioned_code(path: str) -> bool:
        return (
            (path.startswith("src/") and path.endswith(".rs"))
            or path == "build.rs"
            or path == "Cargo.toml"
            or path.startswith("vendor/")
        )

    if not any(touches_versioned_code(path) for path in changed_paths):
        return []

    base_version = extract_package_version(base_cargo_toml)
    head_version = extract_package_version(head_cargo_toml)
    if base_version is not None and base_version == head_version:
        return [
            Finding(
                severity="high",
                location="Cargo.toml",
                message=(
                    f"[package].version is unchanged ({head_version!r}) but this diff touches "
                    "src/**/*.rs, build.rs, Cargo.toml, or vendor/**; bump version in the same commit"
                ),
            )
        ]
    return []


def check_generated_paths(changed_paths: list[str]) -> list[Finding]:
    findings = []
    for path in changed_paths:
        is_generated = path.startswith(GENERATED_PATH_PREFIXES) or path in GENERATED_PATH_EXACT
        if is_generated:
            findings.append(
                Finding(
                    severity="critical",
                    location=path,
                    message="hand-edits release-CI-owned/generated output; fix the source (docs/next, scripts/preview.py, the workflow) and let CI regenerate this path",
                )
            )
    return findings


def check_allow_justification(
    added_lines_by_path: dict[str, list[tuple[int, str]]],
    head_lines_by_path: dict[str, list[str]],
) -> list[Finding]:
    findings = []
    for path, added in added_lines_by_path.items():
        if not path.endswith(".rs"):
            continue
        head_lines = head_lines_by_path.get(path, [])
        for lineno, text in added:
            if not ALLOW_ATTR_RE.search(text):
                continue
            if "//" in text:
                continue  # trailing same-line comment
            preceding_index = lineno - 2  # lineno is 1-based; preceding line is lineno-1
            preceding = head_lines[preceding_index] if 0 <= preceding_index < len(head_lines) else ""
            if preceding.strip().startswith("//"):
                continue
            findings.append(
                Finding(
                    severity="medium",
                    location=path,
                    message=(
                        f"line {lineno}: #[allow(...)] added with no justification comment; "
                        "add a `//` comment on the previous line or trailing the attribute explaining why"
                    ),
                )
            )
    return findings


def check_issue_closing_keywords(commits: list[Commit]) -> list[Finding]:
    findings = []
    for commit in commits:
        message = f"{commit.subject}\n{commit.body}"
        match = CLOSING_KEYWORD_RE.search(message)
        if match is None:
            continue
        findings.append(
            Finding(
                severity="high",
                location=commit.sha,
                message=(
                    f"commit message binds closing keyword {match.group(1)!r} to an issue reference; "
                    "master holds unreleased work, so use a bare `refs #<n>` instead and let release CI close it"
                ),
            )
        )
    return findings


# ── report rendering (pure) ─────────────────────────────────────────────


def render_report(findings: list[Finding]) -> str:
    lines = ["# Deterministic Review", ""]
    if not findings:
        lines.append("No findings.")
    else:
        lines.append("## Findings")
        lines.append("")
        for finding in sorted(findings, key=lambda f: SEVERITY_ORDER.index(f.severity)):
            lines.append(f"{finding.severity.upper()} - {finding.location}: {finding.message}")
    counts = collections.Counter(f.severity for f in findings)
    lines.append("")
    lines.append(
        "VERDICT: {critical} critical, {high} high, {medium} medium, {low} low".format(
            critical=counts["critical"], high=counts["high"], medium=counts["medium"], low=counts["low"]
        )
    )
    return "\n".join(lines)


# ── git plumbing (impure; kept out of the checks above) ────────────────


def run_git(args: list[str]) -> str:
    result = subprocess.run(["git", *args], capture_output=True, text=True)
    if result.returncode != 0:
        raise GitError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def read_git_file(sha: str, path: str) -> str | None:
    result = subprocess.run(["git", "show", f"{sha}:{path}"], capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else None


def run_all_checks(base_sha: str, head_sha: str) -> list[Finding]:
    changed_paths = [path for _status, path in parse_name_status(run_git(["diff", "--name-status", f"{base_sha}...{head_sha}"]))]

    findings: list[Finding] = []
    findings += check_version_bump(changed_paths, read_git_file(base_sha, "Cargo.toml"), read_git_file(head_sha, "Cargo.toml"))
    findings += check_generated_paths(changed_paths)

    added_by_path = parse_added_lines(run_git(["diff", "--unified=0", f"{base_sha}...{head_sha}"]))
    rs_added = {path: lines for path, lines in added_by_path.items() if path.endswith(".rs")}
    head_lines_by_path = {path: (read_git_file(head_sha, path) or "").splitlines() for path in rs_added}
    findings += check_allow_justification(rs_added, head_lines_by_path)

    commits = parse_commits(run_git(["log", f"--format={RECORD_SEP}%H{FIELD_SEP}%s{FIELD_SEP}%b", f"{base_sha}..{head_sha}"]))
    findings += check_issue_closing_keywords(commits)

    return findings


def main() -> int:
    base_sha = os.environ.get("BASE_SHA")
    head_sha = os.environ.get("HEAD_SHA")
    if not base_sha or not head_sha:
        print("review_rules: BASE_SHA and HEAD_SHA must both be set", file=sys.stderr)
        return 2

    try:
        findings = run_all_checks(base_sha, head_sha)
    except GitError as exc:
        print(f"review_rules: {exc}", file=sys.stderr)
        return 2

    print(render_report(findings))
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
