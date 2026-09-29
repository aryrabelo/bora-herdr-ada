import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import autogroup  # noqa: E402
from autogroup import Repo, Rule  # noqa: E402

GIT_ENV = dict(
    os.environ,
    GIT_AUTHOR_NAME="t",
    GIT_AUTHOR_EMAIL="t@example.com",
    GIT_COMMITTER_NAME="t",
    GIT_COMMITTER_EMAIL="t@example.com",
)


def git(cwd, *args):
    subprocess.run(["git", "-C", cwd] + list(args), check=True, env=GIT_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def rule(pattern, group="g"):
    kind = "path" if ("/" in pattern or pattern.startswith("~")) else "name"
    return Rule(pattern, group, kind, 1)


class ParseRulesTest(unittest.TestCase):
    def test_comments_blanks_and_spacing(self):
        text = "\n# a comment\n   \n  postpilot*   =   pp  \n\t# indented comment\nother = o\n"
        rules, problems = autogroup.parse_rules(text)
        self.assertEqual([(r.pattern, r.group, r.line_no) for r in rules], [("postpilot*", "pp", 4), ("other", "o", 6)])
        self.assertEqual(problems, [])

    def test_equals_inside_group_text_is_kept(self):
        rules, problems = autogroup.parse_rules("x = a = b/c d\n")
        self.assertEqual(rules[0].group, "a = b/c d")
        self.assertEqual(problems, [])

    def test_malformed_lines_are_skipped_and_reported_not_fatal(self):
        text = "good = ok\nno equals here\n = nopattern\nnogroup =   \nalso-good = fine\n"
        rules, problems = autogroup.parse_rules(text)
        self.assertEqual([r.pattern for r in rules], ["good", "also-good"])
        self.assertEqual([p[0] for p in problems], [2, 3, 4])

    def test_kind_name_vs_path(self):
        rules, _ = autogroup.parse_rules("a* = 1\n~/x = 2\n/abs/* = 3\n*/mid/* = 4\n~bare = 5\n")
        self.assertEqual([r.kind for r in rules], ["name", "path", "path", "path", "path"])


class MatchTest(unittest.TestCase):
    REPO = Repo("PostPilot-Web", "/Users/me/Sites/postpilot/PostPilot-Web")

    def test_name_glob_matches_name_only(self):
        self.assertTrue(autogroup.rule_matches(rule("postpilot*"), self.REPO))
        # a name rule must not see the path components
        self.assertFalse(autogroup.rule_matches(rule("sites"), self.REPO))
        self.assertFalse(autogroup.rule_matches(rule("postpilot"), self.REPO))

    def test_case_insensitive_both_kinds(self):
        self.assertTrue(autogroup.rule_matches(rule("POSTPILOT-web"), self.REPO))
        self.assertTrue(autogroup.rule_matches(rule("/USERS/ME/sites/*"), self.REPO))

    def test_path_glob_tilde_expansion_and_trailing_slash(self):
        with tempfile.TemporaryDirectory() as home:
            with mock.patch.dict(os.environ, {"HOME": home}):
                repo = Repo("proj", os.path.join(home, "Sites", "proj"))
                self.assertTrue(autogroup.rule_matches(rule("~/Sites/proj"), repo))
                self.assertTrue(autogroup.rule_matches(rule("~/Sites/proj/"), repo))
                self.assertTrue(autogroup.rule_matches(rule("~/Sites/*"), repo))
                self.assertFalse(autogroup.rule_matches(rule("~/Other/*"), repo))
                # trailing slash on the repo root side is ignored too
                self.assertTrue(autogroup.rule_matches(rule("~/Sites/proj"), Repo("proj", repo.root + "/")))

    def test_path_rule_needs_star_to_cover_children(self):
        repo = Repo("r", "/a/b/r")
        self.assertFalse(autogroup.rule_matches(rule("/a/b"), repo))
        self.assertTrue(autogroup.rule_matches(rule("/a/b/*"), repo))

    def test_first_match_wins(self):
        rules = [rule("post*", "first"), rule("postpilot-web", "second"), rule("*", "third")]
        self.assertEqual(autogroup.first_match(rules, self.REPO).group, "first")
        self.assertEqual(autogroup.first_match(rules[1:], self.REPO).group, "second")
        self.assertEqual(autogroup.first_match(rules, Repo("zzz", "/z")).group, "third")
        self.assertIsNone(autogroup.first_match(rules[:2], Repo("zzz", "/z")))


class DecideTest(unittest.TestCase):
    R = Rule("p*", "pp", "name", 7)

    def test_ungrouped_is_assigned_including_empty_and_blank(self):
        for current in (None, "", "   "):
            d = autogroup.decide(current, self.R, False)
            self.assertTrue(d.assign, current)
            self.assertEqual(d.group, "pp")

    def test_grouped_is_untouched_without_force(self):
        d = autogroup.decide("mine", self.R, False)
        self.assertFalse(d.assign)
        self.assertIn("mine", d.reason)

    def test_force_regroups_but_is_idempotent(self):
        self.assertTrue(autogroup.decide("mine", self.R, True).assign)
        self.assertFalse(autogroup.decide("pp", self.R, True).assign)

    def test_no_rule_skips_even_with_force(self):
        for current in (None, "x"):
            self.assertFalse(autogroup.decide(current, None, True).assign)
            self.assertFalse(autogroup.decide(current, None, False).assign)


class RepoFromGitTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        base = os.path.realpath(cls.tmp.name)
        cls.main = os.path.join(base, "Main-Repo")
        os.makedirs(cls.main)
        git(cls.main, "init", "-q", ".")
        git(cls.main, "commit", "-q", "--allow-empty", "-m", "init")
        cls.sub = os.path.join(cls.main, "src", "deep")
        os.makedirs(cls.sub)
        cls.linked = os.path.join(base, "elsewhere", "feature-wt")
        os.makedirs(os.path.dirname(cls.linked))
        git(cls.main, "worktree", "add", "-q", cls.linked, "-b", "feature")
        cls.bare = os.path.join(base, "bare-thing.git")
        subprocess.run(["git", "init", "-q", "--bare", cls.bare], check=True, env=GIT_ENV)
        cls.plain = os.path.join(base, "not-a-repo")
        os.makedirs(cls.plain)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_plain_repo_root_and_subdirectory(self):
        for cwd in (self.main, self.sub):
            self.assertEqual(autogroup.repo_from_git(cwd), Repo("Main-Repo", self.main))

    def test_linked_worktree_resolves_to_main_repo(self):
        self.assertEqual(autogroup.repo_from_git(self.linked), Repo("Main-Repo", self.main))
        # and a group rule for the main repo therefore follows the worktree
        repo = autogroup.repo_from_git(self.linked)
        self.assertEqual(autogroup.first_match([rule("main-*", "grp")], repo).group, "grp")

    def test_bare_repo_uses_the_git_dir_itself(self):
        self.assertEqual(autogroup.repo_from_git(self.bare), Repo("bare-thing.git", self.bare))

    def test_non_git_and_missing_dir_are_none(self):
        self.assertIsNone(autogroup.repo_from_git(self.plain))
        self.assertIsNone(autogroup.repo_from_git(os.path.join(self.plain, "missing")))
        self.assertIsNone(autogroup.repo_from_git(None))


class ProcessWorkspaceTest(unittest.TestCase):
    """Behavior of the assign/skip flow against a recording set_group."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.repo = os.path.join(os.path.realpath(cls.tmp.name), "postpilot-x")
        os.makedirs(cls.repo)
        git(cls.repo, "init", "-q", ".")
        git(cls.repo, "commit", "-q", "--allow-empty", "-m", "init")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    RULES = [Rule("postpilot*", "pp", "name", 1)]

    def run_process(self, info, force=False, dry_run=False, panes=()):
        calls = []
        with mock.patch.object(autogroup.bora_api, "workspace_set_group", lambda ws, g: calls.append((ws, g))), mock.patch.object(
            autogroup.bora_api, "pane_list", lambda ws: list(panes)
        ):
            with contextlib.redirect_stdout(io.StringIO()):
                ok = autogroup.process_workspace(info, self.RULES, force, dry_run)
        return ok, calls

    def api_info(self, group=None, name="postpilot-x"):
        info = {"workspace_id": "w1", "label": name, "worktree": {"repo_name": name, "repo_root": "/r/" + name}}
        if group is not None:
            info["visual_group"] = group
        return info

    def test_ungrouped_match_is_assigned(self):
        self.assertEqual(self.run_process(self.api_info())[1], [("w1", "pp")])
        self.assertEqual(self.run_process(self.api_info(group=""))[1], [("w1", "pp")])

    def test_hand_chosen_group_is_never_overridden(self):
        self.assertEqual(self.run_process(self.api_info(group="mine"))[1], [])

    def test_force_regroups_only_when_different(self):
        self.assertEqual(self.run_process(self.api_info(group="mine"), force=True)[1], [("w1", "pp")])
        self.assertEqual(self.run_process(self.api_info(group="pp"), force=True)[1], [])

    def test_dry_run_calls_nothing(self):
        self.assertEqual(self.run_process(self.api_info(), dry_run=True)[1], [])
        self.assertEqual(self.run_process(self.api_info(group="mine"), force=True, dry_run=True)[1], [])

    def test_unmatched_repo_is_skipped(self):
        self.assertEqual(self.run_process(self.api_info(name="other-y"))[1], [])

    def test_git_fallback_when_api_has_no_worktree(self):
        info = {"workspace_id": "w2", "label": "x"}
        panes = [{"cwd": "/does/not/exist", "focused": False}, {"cwd": self.repo, "focused": False}]
        self.assertEqual(self.run_process(info, panes=panes)[1], [("w2", "pp")])

    def test_no_worktree_and_no_git_cwd_is_skipped(self):
        info = {"workspace_id": "w3", "label": "x"}
        self.assertEqual(self.run_process(info, panes=[{"cwd": self.tmp.name}])[1], [])
        self.assertEqual(self.run_process(info, panes=[])[1], [])

    def test_set_group_failure_is_reported_as_failure(self):
        def boom(ws, group):
            raise autogroup.bora_api.ApiError("workspace_not_found", "gone")

        with mock.patch.object(autogroup.bora_api, "workspace_set_group", boom), contextlib.redirect_stderr(io.StringIO()):
            self.assertFalse(autogroup.process_workspace(self.api_info(), self.RULES, False, False))


class EventWorkspaceTest(unittest.TestCase):
    def test_workspace_id_and_worktree_come_from_data_workspace(self):
        wt = {"repo_name": "r", "repo_root": "/r"}
        event = {"event": "worktree.created", "data": {"workspace": {"workspace_id": "w4", "worktree": wt}, "worktree": {"path": "/p"}}}
        self.assertEqual(autogroup._event_workspace(event), ("w4", wt))
        self.assertEqual(autogroup._event_workspace({"data": {}}), (None, None))


if __name__ == "__main__":
    unittest.main()
