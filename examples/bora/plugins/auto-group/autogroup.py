#!/usr/bin/env python3
"""ary.auto-group: put workspaces into sidebar groups according to their repository.

  autogroup.py hook                         event hook (workspace.created,
                                            worktree.created, worktree.opened)
  autogroup.py organize [--dry-run] [--force]
                                            apply the rules to every workspace;
                                            without --force only ungrouped ones
  autogroup.py check                        parse the rules file and print it

The plugin never fights a manual choice: a workspace that already has a group is
left alone unless `organize --force` is run by hand. Python 3.9, stdlib only.
"""

import argparse
import collections
import fnmatch
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bora_api  # noqa: E402

PLUGIN_ID = "ary.auto-group"
RULES_FILE = "rules.conf"

Rule = collections.namedtuple("Rule", "pattern group kind line_no")
Repo = collections.namedtuple("Repo", "name root")
Decision = collections.namedtuple("Decision", "assign group reason")


# ---------------------------------------------------------------- rules file


def parse_rules(text):
    """Parse a rules file. Returns (rules, problems); problems = [(line_no, line, reason)]."""
    rules = []
    problems = []
    for line_no, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            problems.append((line_no, raw, "missing '=' (expected '<pattern> = <group>')"))
            continue
        pattern, group = line.split("=", 1)
        pattern = pattern.strip()
        group = group.strip()
        if not pattern:
            problems.append((line_no, raw, "empty pattern before '='"))
            continue
        if not group:
            problems.append((line_no, raw, "empty group after '='"))
            continue
        kind = "path" if ("/" in pattern or pattern.startswith("~")) else "name"
        rules.append(Rule(pattern, group, kind, line_no))
    return rules, problems


def _norm_path(path):
    trimmed = path.rstrip("/")
    return (trimmed or "/").lower()


def rule_matches(rule, repo):
    """Name rules glob the repo name; path rules glob the repo root (`~` expanded)."""
    if rule.kind == "name":
        return fnmatch.fnmatchcase(repo.name.lower(), rule.pattern.lower())
    pattern = _norm_path(os.path.expanduser(rule.pattern))
    return fnmatch.fnmatchcase(_norm_path(repo.root), pattern)


def first_match(rules, repo):
    for rule in rules:
        if rule_matches(rule, repo):
            return rule
    return None


def decide(current_group, matched_rule, force):
    """Pure decision: should a workspace currently in `current_group` be assigned?"""
    current = (current_group or "").strip()
    if matched_rule is None:
        return Decision(False, None, "no rule matches")
    target = matched_rule.group
    if current and not force:
        return Decision(False, None, "already grouped in '%s'" % current)
    if current == target:
        return Decision(False, None, "already in '%s'" % target)
    return Decision(True, target, "rule line %d: %s" % (matched_rule.line_no, matched_rule.pattern))


# ------------------------------------------------------------- repo identity


def repo_from_worktree_info(worktree):
    """`WorkspaceInfo.worktree` already names the MAIN repo, also for linked worktrees."""
    if not worktree:
        return None
    root = worktree.get("repo_root")
    if not root:
        return None
    return Repo(worktree.get("repo_name") or os.path.basename(root.rstrip("/")), root)


def _git_env():
    env = dict(os.environ)
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"):
        env.pop(key, None)
    env["PATH"] = env.get("PATH", "") + os.pathsep + "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin"
    return env


def _git(cwd, *args):
    try:
        proc = subprocess.run(
            ["git", "-C", cwd] + list(args),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=_git_env(),
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.decode("utf-8", "replace").splitlines()


def repo_from_git(cwd):
    """Main repo of the checkout at `cwd` via `git rev-parse --git-common-dir`, or None."""
    if not cwd or not os.path.isdir(cwd):
        return None
    lines = _git(cwd, "rev-parse", "--git-common-dir", "--is-bare-repository")
    if not lines or len(lines) < 2:
        return None
    common = lines[0]
    if not os.path.isabs(common):
        common = os.path.join(os.path.realpath(cwd), common)
    common = os.path.normpath(common)
    if lines[1].strip() == "true":
        root = common
    elif os.path.basename(common) == ".git":
        root = os.path.dirname(common)
    else:
        # A submodule's git dir lives outside `.git`; its work tree is the repo.
        top = _git(cwd, "rev-parse", "--show-toplevel")
        if not top or not top[0]:
            return None
        root = os.path.normpath(top[0])
    return Repo(os.path.basename(root), root)


# --------------------------------------------------------------- config / IO


def config_dir():
    env = os.environ.get("HERDR_PLUGIN_CONFIG_DIR", "").strip()
    if env:
        return env
    binary = os.environ.get("HERDR_BIN_PATH", "").strip()
    if not binary:
        raise SystemExit(
            "cannot locate the config dir: HERDR_PLUGIN_CONFIG_DIR and HERDR_BIN_PATH are both unset "
            "(run inside bora, or set HERDR_PLUGIN_CONFIG_DIR)"
        )
    try:
        proc = subprocess.run(
            [binary, "plugin", "config-dir", PLUGIN_ID],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise SystemExit("`%s plugin config-dir %s` failed: %s" % (binary, PLUGIN_ID, exc))
    out = proc.stdout.decode("utf-8", "replace").strip()
    if proc.returncode != 0 or not out:
        raise SystemExit(
            "`%s plugin config-dir %s` failed: %s"
            % (binary, PLUGIN_ID, proc.stderr.decode("utf-8", "replace").strip() or "empty output")
        )
    return out.splitlines()[-1].strip()


def rules_path():
    return os.path.join(config_dir(), RULES_FILE)


def load_rules(path):
    """Returns (rules or None if the file is absent, problems)."""
    try:
        with open(path, "r", encoding="utf-8") as handle:
            text = handle.read()
    except FileNotFoundError:
        return None, []
    return parse_rules(text)


def report_problems(problems):
    for line_no, raw, reason in problems:
        print("warning: rules line %d skipped: %s: %s" % (line_no, reason, raw.strip()), file=sys.stderr)


# ------------------------------------------------------------------ workspace


def _is_grouped(info):
    return bool((info.get("visual_group") or "").strip())


def _candidate_cwds(workspace_id, hints):
    cwds = [c for c in hints if c]
    try:
        panes = bora_api.pane_list(workspace_id)
    except bora_api.ApiError:
        panes = []
    panes = sorted(panes, key=lambda pane: not pane.get("focused"))
    for pane in panes:
        cwds.extend(c for c in (pane.get("cwd"), pane.get("foreground_cwd")) if c)
    return cwds


def resolve_repo(info, extra_worktree=None, cwd_hints=()):
    """API worktree info first (main repo, no subprocess); else git on the workspace cwd."""
    repo = repo_from_worktree_info(info.get("worktree")) or repo_from_worktree_info(extra_worktree)
    if repo:
        return repo
    for cwd in _candidate_cwds(info["workspace_id"], cwd_hints):
        repo = repo_from_git(cwd)
        if repo:
            return repo
    return None


def process_workspace(info, rules, force, dry_run, extra_worktree=None, cwd_hints=()):
    """Decide and (unless dry-run) apply for one workspace. Prints one line. Returns True on success."""
    workspace_id = info["workspace_id"]
    label = info.get("label", "")
    tag = "%s (%s)" % (workspace_id, label)
    current = info.get("visual_group")
    if _is_grouped(info) and not force:
        print("skip %s: already grouped in '%s'" % (tag, current.strip()))
        return True
    repo = resolve_repo(info, extra_worktree, cwd_hints)
    if repo is None:
        print("skip %s: not in a git repository" % tag)
        return True
    decision = decide(current, first_match(rules, repo), force)
    if not decision.assign:
        print("skip %s: %s (repo %s at %s)" % (tag, decision.reason, repo.name, repo.root))
        return True
    if dry_run:
        print("would group %s -> %s (%s)" % (tag, decision.group, decision.reason))
        return True
    try:
        bora_api.workspace_set_group(workspace_id, decision.group)
    except bora_api.ApiError as exc:
        print("error %s: workspace.set_group failed: %s" % (tag, exc), file=sys.stderr)
        return False
    print("grouped %s -> %s (%s)" % (tag, decision.group, decision.reason))
    return True


def _load_or_explain(command):
    """Rules for hook/organize, or None after logging why there is nothing to do."""
    path = rules_path()
    rules, problems = load_rules(path)
    if rules is None:
        print("%s: no rules file at %s; nothing to do" % (command, path))
        return None
    report_problems(problems)
    if not rules:
        print("%s: %s has no rules; nothing to do" % (command, path))
        return None
    return rules


# ------------------------------------------------------------------- commands


def cmd_organize(args):
    rules = _load_or_explain("organize")
    if rules is None:
        return 0
    ok = True
    for info in bora_api.workspace_list():
        ok = process_workspace(info, rules, args.force, args.dry_run) and ok
    return 0 if ok else 1


def _event_workspace(event):
    data = event.get("data") or {}
    workspace = data.get("workspace") or {}
    workspace_id = workspace.get("workspace_id") or data.get("workspace_id")
    return workspace_id, workspace.get("worktree")


def cmd_hook(args):
    raw = os.environ.get("HERDR_PLUGIN_EVENT_JSON", "")
    if not raw:
        print("hook: HERDR_PLUGIN_EVENT_JSON is not set (this command is an event hook)", file=sys.stderr)
        return 2
    try:
        event = json.loads(raw)
    except ValueError as exc:
        print("hook: unparseable HERDR_PLUGIN_EVENT_JSON: %s" % exc, file=sys.stderr)
        return 2
    workspace_id, event_worktree = _event_workspace(event)
    workspace_id = workspace_id or os.environ.get("HERDR_WORKSPACE_ID")
    if not workspace_id:
        print("hook: event %s carries no workspace id" % event.get("event"), file=sys.stderr)
        return 2
    rules = _load_or_explain("hook")
    if rules is None:
        return 0
    try:
        # Re-read the workspace: the event is a snapshot, and a group set by hand
        # since then must win.
        info = bora_api.workspace_get(workspace_id)
    except bora_api.ApiError as exc:
        if exc.code == "workspace_not_found":
            print("skip %s: workspace no longer exists" % workspace_id)
            return 0
        raise
    hints = []
    try:
        context = json.loads(os.environ.get("HERDR_PLUGIN_CONTEXT_JSON") or "{}")
    except ValueError:
        context = {}
    if context.get("workspace_id") == workspace_id:
        hints = [context.get("workspace_cwd"), context.get("focused_pane_cwd")]
    ok = process_workspace(info, rules, False, False, event_worktree, hints)
    return 0 if ok else 1


def cmd_check(args):
    path = rules_path()
    rules, problems = load_rules(path)
    if rules is None:
        print("no rules file at %s (copy rules.example.conf there)" % path)
        return 1
    print("rules file: %s" % path)
    for rule in rules:
        target = "repo name" if rule.kind == "name" else "repo path"
        print("line %d: %s glob '%s' -> %s" % (rule.line_no, target, rule.pattern, rule.group))
    for line_no, raw, reason in problems:
        print("line %d: MALFORMED (%s): %s" % (line_no, reason, raw.strip()))
    print("%d rule(s), %d malformed line(s)" % (len(rules), len(problems)))
    return 1 if problems else 0


def build_parser():
    parser = argparse.ArgumentParser(prog="autogroup.py", description="Group bora workspaces by repository.")
    sub = parser.add_subparsers(dest="command")
    sub.required = True
    organize = sub.add_parser("organize", help="apply the rules to all workspaces")
    organize.add_argument("--dry-run", action="store_true", help="print the plan; call nothing that changes state")
    organize.add_argument("--force", action="store_true", help="also regroup workspaces that already have a group")
    organize.set_defaults(func=cmd_organize)
    hook = sub.add_parser("hook", help="event hook entry (reads HERDR_PLUGIN_EVENT_JSON)")
    hook.set_defaults(func=cmd_hook)
    check = sub.add_parser("check", help="parse the rules file and print it")
    check.set_defaults(func=cmd_check)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except bora_api.ApiError as exc:
        print("auto-group: %s" % exc, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
