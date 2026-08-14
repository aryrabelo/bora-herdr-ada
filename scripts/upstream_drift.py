#!/usr/bin/env python3
"""Repo-wide symbol drift between upstream/master and this fork.

Compares SETS of `fn NAME` / `mod NAME;` symbol names across the whole
source tree (not file-by-file: a symbol that merely moved files is not a
loss, and per-file comparison produced false positives on `recent_text`
and `state_icon`). A symbol upstream has that we don't, anywhere, is
either a deliberate fork divergence (belongs in the baseline) or an
accidental regression (should get fixed or explicitly accepted).

Modes:
    --report (default)  print upstream symbols absent from our fork,
                         split production vs #[test].
    --check              exit 1 if any absent symbol is NOT in the baseline.
    --update             rewrite the baseline to the current absent set.

If the `upstream` remote/ref is unavailable, prints a skip message and
exits 0 for every mode -- this script has no meaning without it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BASELINE_PATH = REPO_ROOT / "scripts" / "upstream_drift_baseline.txt"
BASELINE_HEADER = (
    "# Symbols present in upstream/master but absent from this fork's src tree.\n"
    "# Entries here are ACCEPTED divergences (deliberate fork renames/removals),\n"
    "# not a wishlist or a to-do list. Regenerate with:\n"
    "#     python3 scripts/upstream_drift.py --update\n"
)

FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)")
MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")
MOD_BLOCK_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*\{")
TEST_ATTR_RE = re.compile(r"^\s*#\[(?:test|tokio::test|.*::test)\]")
CFG_TEST_MOD_RE = re.compile(r"^\s*#\[cfg\(test\)\]")


class Symbol:
    __slots__ = ("kind", "name")

    def __init__(self, kind: str, name: str):
        self.kind = kind
        self.name = name

    def key(self) -> str:
        return f"{self.kind}:{self.name}"

    def __eq__(self, other):
        return isinstance(other, Symbol) and self.key() == other.key()

    def __hash__(self):
        return hash(self.key())


def extract_symbols(text: str) -> tuple[set, set]:
    """Return (production_symbols, test_symbols) found in one file's text."""
    production: set[Symbol] = set()
    test: set[Symbol] = set()

    depth = 0
    # Depth at which we entered a `#[cfg(test)] mod X { ... }` block; every
    # symbol found while depth stays >= this is test-scoped.
    cfg_test_mod_depth: int | None = None
    # Attribute/doc-comment lines seen since the last real item, cleared
    # once consumed by the next non-attribute/non-doc/non-blank line.
    pending_attrs: list[str] = []

    for line in text.splitlines():
        stripped = line.strip()

        if stripped.startswith("#[") or stripped.startswith("///") or stripped.startswith("//!"):
            pending_attrs.append(line)
            continue
        if stripped == "":
            continue

        in_test_scope = cfg_test_mod_depth is not None
        has_test_attr = any(TEST_ATTR_RE.match(a) for a in pending_attrs)
        has_cfg_test_attr = any(CFG_TEST_MOD_RE.match(a) for a in pending_attrs)

        fn_match = FN_RE.match(line)
        mod_decl = MOD_RE.match(line)
        mod_block = MOD_BLOCK_RE.match(line)

        if fn_match:
            is_test = in_test_scope or has_test_attr
            (test if is_test else production).add(Symbol("fn", fn_match.group(1)))
        elif mod_decl:
            is_test = in_test_scope or has_test_attr
            (test if is_test else production).add(Symbol("mod", mod_decl.group(1)))
        elif mod_block:
            is_test = in_test_scope or has_cfg_test_attr
            (test if is_test else production).add(Symbol("mod", mod_block.group(1)))
            if has_cfg_test_attr and cfg_test_mod_depth is None:
                cfg_test_mod_depth = depth  # depth BEFORE this line's own braces are counted

        pending_attrs = []
        depth += line.count("{") - line.count("}")
        if cfg_test_mod_depth is not None and depth <= cfg_test_mod_depth:
            cfg_test_mod_depth = None

    return production, test


def collect_local_symbols() -> tuple[set, set]:
    production: set[Symbol] = set()
    test: set[Symbol] = set()
    for path in (REPO_ROOT / "src").rglob("*.rs"):
        text = path.read_text(encoding="utf-8", errors="replace")
        p, t = extract_symbols(text)
        production |= p
        test |= t
    return production, test


def upstream_ref_available(ref: str) -> bool:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", ref],
        cwd=REPO_ROOT,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return result.returncode == 0


def collect_upstream_symbols(ref: str) -> tuple[set, set]:
    ls_tree = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", ref, "--", "src"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    production: set[Symbol] = set()
    test: set[Symbol] = set()
    for rel_path in ls_tree.stdout.splitlines():
        if not rel_path.endswith(".rs"):
            continue
        show = subprocess.run(
            ["git", "show", f"{ref}:{rel_path}"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )
        if show.returncode != 0:
            continue
        p, t = extract_symbols(show.stdout)
        production |= p
        test |= t
    return production, test


def load_baseline() -> set[str]:
    if not BASELINE_PATH.exists():
        return set()
    keys = set()
    for line in BASELINE_PATH.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        keys.add(line)
    return keys


def write_baseline(keys: set[str]) -> None:
    body = BASELINE_HEADER + "\n".join(sorted(keys)) + ("\n" if keys else "")
    BASELINE_PATH.write_text(body, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--report", action="store_true", help="print absent symbols (default)")
    mode.add_argument("--check", action="store_true", help="exit 1 on un-baselined absent symbols")
    mode.add_argument("--update", action="store_true", help="rewrite the baseline")
    parser.add_argument(
        "--upstream-ref",
        default="upstream/master",
        help="ref to diff against (default: upstream/master; override only for testing the skip path)",
    )
    args = parser.parse_args()

    if not upstream_ref_available(args.upstream_ref):
        print(
            f"SKIP: ref '{args.upstream_ref}' is not available (missing remote or unfetched branch). "
            "Nothing to compare against, so this script has no meaning here."
        )
        return 0

    upstream_prod, upstream_test = collect_upstream_symbols(args.upstream_ref)
    local_prod, local_test = collect_local_symbols()
    local_all = local_prod | local_test

    absent_prod = sorted(s.key() for s in upstream_prod if s not in local_all)
    absent_test = sorted(s.key() for s in upstream_test if s not in local_all)
    absent_all = set(absent_prod) | set(absent_test)

    if args.update:
        write_baseline(absent_all)
        print(f"wrote {len(absent_all)} entries to {BASELINE_PATH}")
        return 0

    if args.check:
        baseline = load_baseline()
        unbaselined = sorted(absent_all - baseline)
        if unbaselined:
            print(f"{len(unbaselined)} upstream symbol(s) absent from the fork and NOT in the baseline:")
            for key in unbaselined:
                print(f"  {key}")
            print(f"\nrun `python3 {Path(__file__).name} --update` if this is a deliberate divergence.")
            return 1
        print(f"OK: {len(absent_all)} absent symbol(s), all present in baseline ({BASELINE_PATH.name}).")
        return 0

    # default: --report
    print(
        f"upstream symbols absent from this fork: {len(absent_all)} total "
        f"({len(absent_prod)} production, {len(absent_test)} test)"
    )
    if absent_prod:
        print("\nproduction:")
        for key in absent_prod:
            print(f"  {key}")
    if absent_test:
        print("\ntest:")
        for key in absent_test:
            print(f"  {key}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
