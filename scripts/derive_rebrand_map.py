#!/usr/bin/env python3
"""Derives the fork's rebrand map from a git diff.

The fork renames the product `herdr` to `bora` in user-facing text only.
Hand-editing those literals means every upstream merge conflicts on lines a
human has to re-judge. Instead the mapping lives in `scripts/rebrand.json` and
is replayed mechanically: on an upstream merge conflict in branded text, take
upstream's side and run `scripts/rebrand.py`.

This tool builds that mapping from a diff so the map is derived from real
edits rather than typed twice. It only accepts hunks where the old and new
line differ purely by a herdr -> bora substitution; anything else is reported
for a human to look at, never guessed.

Usage:
    scripts/derive_rebrand_map.py [<git-diff-args>...] > scripts/rebrand.json
    scripts/derive_rebrand_map.py --review    # print rejected pairs only
"""

from __future__ import annotations

import json
import re
import subprocess
import sys


def brand_only_change(old: str, new: str) -> bool:
    """True when `new` is `old` with herdr swapped for bora and nothing else."""
    if old == new:
        return False
    canonical = new
    for herdr, bora in (
        ("Herdr", "Bora"),
        ("herdr", "bora"),
        ("HERDR", "BORA"),
    ):
        canonical = canonical.replace(bora, herdr)
    return canonical == old


def paired_lines(diff: str):
    """Yields (path, old_line, new_line) for 1:1 replacements inside hunks."""
    path = None
    removed: list[str] = []
    added: list[str] = []

    def flush():
        # Only a balanced hunk maps unambiguously; report the rest.
        if len(removed) == len(added):
            for old, new in zip(removed, added):
                yield path, old, new
        else:
            for old in removed:
                yield path, old, None
        removed.clear()
        added.clear()

    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            yield from flush()
            path = line[6:]
        elif line.startswith("@@"):
            yield from flush()
        elif line.startswith("-") and not line.startswith("---"):
            removed.append(line[1:])
        elif line.startswith("+") and not line.startswith("+++"):
            added.append(line[1:])
        else:
            yield from flush()
    yield from flush()


def literals(old: str, new: str) -> list[tuple[str, str]]:
    """Narrows a changed line to the smallest quoted literals that differ.

    Replacing whole lines would make the map brittle: any unrelated upstream
    edit to the same line would stop matching. Quoted string contents are the
    real unit of branding.
    """
    pairs = []
    old_strings = re.findall(r'"([^"]*)"', old)
    new_strings = re.findall(r'"([^"]*)"', new)
    if len(old_strings) == len(new_strings):
        for o, n in zip(old_strings, new_strings):
            if o != n and brand_only_change(o, n):
                pairs.append((o, n))
    if pairs:
        return pairs
    # No usable quoted literal: fall back to the trimmed line so the edit is
    # still reproducible, at the cost of being sensitive to nearby changes.
    return [(old.strip(), new.strip())]


def main() -> int:
    review = "--review" in sys.argv
    args = [a for a in sys.argv[1:] if a != "--review"]
    if not args:
        args = ["HEAD"]
    diff = subprocess.run(
        ["git", "diff", "-U0", *args],
        capture_output=True,
        text=True,
        check=True,
    ).stdout

    mapping: dict[str, list[dict[str, str]]] = {}
    rejected: list[tuple[str, str]] = []
    for path, old, new in paired_lines(diff):
        if path is None or old is None:
            if old is not None:
                rejected.append((path or "?", old))
            continue
        if not brand_only_change(old, new):
            rejected.append((path, old))
            continue
        for o, n in literals(old, new):
            entry = {"from": o, "to": n}
            bucket = mapping.setdefault(path, [])
            if entry not in bucket:
                bucket.append(entry)

    if review:
        for path, line in rejected:
            print(f"{path}: {line.strip()}")
        print(f"\n{len(rejected)} line(s) are not pure rebrands; review by hand.")
        return 0

    json.dump(
        {"replacements": mapping},
        sys.stdout,
        indent=2,
        ensure_ascii=False,
        sort_keys=True,
    )
    print()
    if rejected:
        print(
            f"note: skipped {len(rejected)} non-rebrand line(s); "
            "rerun with --review to inspect them",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
