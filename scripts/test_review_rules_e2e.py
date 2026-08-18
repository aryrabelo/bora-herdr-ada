"""End-to-end checks for scripts/review_rules.py against real git repositories.

test_review_rules.py exercises the pure check functions with synthetic input.
This module drives the actual git plumbing instead — `--name-status` parsing,
`--unified=0` line-number tracking, and the record-separated `git log` format —
because that is where a parser silently measures the wrong thing. It earns its
runtime: it caught a finding that named the offending keyword but not the issue
reference the author had to go rewrite.

Every check is asserted in BOTH directions. A checker that only ever fires is
noise, and a checker that never fires is decoration.
"""

from __future__ import annotations

import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CHECKER = REPO_ROOT / "scripts/review_rules.py"

BASE_CARGO = '[package]\nname = "bora"\nversion = "0.20.0"\n\n[dependencies]\nserde = { version = "1.0" }\n'
VERDICT_RE = re.compile(
    r"^VERDICT: \d+ critical, \d+ high, \d+ medium, \d+ low$", re.MULTILINE
)

FINDINGS = 1  # the checker's exit code when rules were violated
CLEAN = 0
OPERATIONAL_FAILURE = 2


class ReviewRulesEndToEnd(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        (self.repo / "src").mkdir()
        (self.repo / "docs/versions").mkdir(parents=True)
        self.write("Cargo.toml", BASE_CARGO)
        self.write("src/main.rs", "fn main() {}\n")
        self.write("docs/versions/0.19.md", "published\n")
        self.git("init", "-q")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "t")
        self.commit("base")

    def git(self, *args: str) -> str:
        return subprocess.run(
            ["git", *args],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout

    def write(self, path: str, content: str) -> None:
        (self.repo / path).write_text(content, encoding="utf-8")

    def append(self, path: str, content: str) -> None:
        with (self.repo / path).open("a", encoding="utf-8") as handle:
            handle.write(content)

    def commit(self, message: str) -> None:
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def bump_version(self) -> None:
        self.write("Cargo.toml", BASE_CARGO.replace('version = "0.20.0"', 'version = "0.20.1"'))

    def review(self, base: str = "HEAD~1", head: str = "HEAD") -> tuple[int, str]:
        env = {**os.environ, "BASE_SHA": base, "HEAD_SHA": head}
        done = subprocess.run(
            [str(CHECKER)], cwd=self.repo, env=env, capture_output=True, text=True
        )
        output = done.stdout + done.stderr
        if done.returncode in (CLEAN, FINDINGS):
            # CI greps for this line to decide whether a review happened at all,
            # so it must survive every reviewed path, clean or not.
            self.assertRegex(output, VERDICT_RE, "every review must end in a VERDICT line")
        return done.returncode, output

    def assert_flags(self, expected_text: str) -> None:
        status, output = self.review()
        self.assertEqual(status, FINDINGS, f"expected a finding, got:\n{output}")
        self.assertIn(expected_text, output)

    def assert_clean(self) -> None:
        status, output = self.review()
        self.assertEqual(status, CLEAN, f"expected no findings, got:\n{output}")

    # ── version bump ────────────────────────────────────────────────────

    def test_rust_change_without_a_version_bump_is_flagged(self) -> None:
        self.write("src/main.rs", 'fn main() { println!("x"); }\n')
        self.commit("feat: change behaviour")
        self.assert_flags("Cargo.toml")

    def test_rust_change_with_a_version_bump_is_clean(self) -> None:
        self.write("src/main.rs", 'fn main() { println!("x"); }\n')
        self.bump_version()
        self.commit("feat: change behaviour")
        self.assert_clean()

    def test_docs_only_change_needs_no_version_bump(self) -> None:
        self.write("README.md", "hello\n")
        self.commit("docs: readme")
        self.assert_clean()

    def test_dependency_version_is_not_mistaken_for_the_package_version(self) -> None:
        # Bumping a dependency still touches Cargo.toml, so a naive `version =`
        # match would call this a bump and wave the change through.
        self.write("Cargo.toml", BASE_CARGO.replace('version = "1.0"', 'version = "1.1"'))
        self.commit("chore: bump dep")
        self.assert_flags("Cargo.toml")

    # ── generated and published paths ───────────────────────────────────

    def test_hand_edit_of_published_docs_is_flagged(self) -> None:
        self.write("docs/versions/0.19.md", "edited by hand\n")
        self.bump_version()
        self.commit("docs: tweak")
        self.assert_flags("docs/versions/0.19.md")

    # ── #[allow] justification ──────────────────────────────────────────

    def test_bare_allow_attribute_is_flagged(self) -> None:
        self.append("src/main.rs", "#[allow(dead_code)]\nfn unused() {}\n")
        self.bump_version()
        self.commit("feat: add helper")
        self.assert_flags("allow")

    def test_allow_attribute_with_a_justifying_comment_is_clean(self) -> None:
        self.append(
            "src/main.rs",
            "// upstream API returns dead variants we must keep\n#[allow(dead_code)]\nfn unused() {}\n",
        )
        self.bump_version()
        self.commit("feat: add helper")
        self.assert_clean()

    # ── issue closing keywords ──────────────────────────────────────────

    def test_closing_keyword_in_a_commit_body_is_flagged_and_quoted(self) -> None:
        self.write("README.md", "x\n")
        self.commit("docs: tidy\n\nfixes #12")
        # Naming the offending reference matters: the author has to find and
        # rewrite this exact string, and a body may carry several references.
        self.assert_flags("#12")

    def test_bare_refs_reference_is_clean(self) -> None:
        self.write("README.md", "x\n")
        self.commit("docs: tidy\n\nrefs #12")
        self.assert_clean()

    def test_conventional_fix_subject_is_not_a_closing_keyword(self) -> None:
        self.write("README.md", "x\n")
        self.commit("fix: handle pane focus")
        self.assert_clean()

    # ── operational failure must never look like success ────────────────

    def test_missing_environment_exits_two(self) -> None:
        done = subprocess.run(
            [str(CHECKER)],
            cwd=self.repo,
            env={k: v for k, v in os.environ.items() if k not in ("BASE_SHA", "HEAD_SHA")},
            capture_output=True,
            text=True,
        )
        self.assertEqual(done.returncode, OPERATIONAL_FAILURE)

    def test_unknown_revision_exits_two(self) -> None:
        status, _ = self.review(base="nope123")
        self.assertEqual(status, OPERATIONAL_FAILURE)


if __name__ == "__main__":
    unittest.main()
