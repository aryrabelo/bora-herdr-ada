from __future__ import annotations

import os
import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = REPO_ROOT / ".github/workflows/independent-review.yml"
SCRIPT = REPO_ROOT / "scripts/independent_review.sh"

# The workflow decides "did a review actually happen?" by grepping the reviewer's
# output for this line. The predicate is extracted from the workflow itself
# rather than restated, so these tests cannot drift from what CI runs.
GATE_LINE = re.compile(
    r"if ! grep -qE '(?P<pattern>[^']+)' review-output\.md; then"
)


def gate_pattern() -> re.Pattern[str]:
    match = GATE_LINE.search(WORKFLOW.read_text(encoding="utf-8"))
    if match is None:
        raise AssertionError(
            "independent-review.yml no longer contains the VERDICT grep these "
            "tests assert against; update both together."
        )
    return re.compile(match.group("pattern"), re.MULTILINE)


class IndependentReviewWiringTests(unittest.TestCase):
    def test_workflow_calls_the_script_instead_of_inlining_the_review(self) -> None:
        # A gate whose logic lives only in YAML cannot be run locally, so it is
        # never tested before it fails on a PR.
        self.assertTrue(SCRIPT.exists(), "scripts/independent_review.sh is missing")
        self.assertTrue(
            os.access(SCRIPT, os.X_OK), "scripts/independent_review.sh is not executable"
        )
        self.assertIn("scripts/independent_review.sh", WORKFLOW.read_text(encoding="utf-8"))

    def test_workflow_reviews_the_pushed_commit_not_a_merge_preview(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("ref: ${{ github.event.pull_request.head.sha }}", text)
        self.assertIn("HEAD_SHA: ${{ github.event.pull_request.head.sha }}", text)
        self.assertIn("BASE_SHA: ${{ github.event.pull_request.base.sha }}", text)

    def test_workflow_runs_on_every_push_to_the_pull_request(self) -> None:
        import yaml

        spec = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
        # PyYAML parses a bare `on:` key as the boolean True.
        triggers = spec.get("on", spec.get(True))
        # Assert on the parsed trigger, not on raw text: the file mentions
        # `pull_request_target` in a comment explaining why it is not used.
        self.assertIn("pull_request", triggers)
        self.assertNotIn("pull_request_target", triggers)
        # `synchronize` is the push-to-an-open-PR event; without it the gate
        # would only ever see the first commit.
        self.assertEqual(
            triggers["pull_request"]["types"], ["opened", "synchronize", "reopened"]
        )

    def test_pipefail_is_set_before_piping_the_review_into_tee(self) -> None:
        # Actions' default shell is `bash -e` with no pipefail, so `script | tee`
        # would report tee's exit status and swallow a crashing reviewer.
        #
        # Look at command lines only. An earlier version of this test matched
        # the whole file and passed on a comment that merely mentions
        # `set -euo pipefail`, so deleting the actual command went unnoticed.
        commands = [
            line.strip()
            for line in WORKFLOW.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.strip().startswith("#")
        ]
        self.assertIn("set -euo pipefail", commands, "the review step must set -o pipefail")
        tee = next(i for i, line in enumerate(commands) if "| tee review-output.md" in line)
        self.assertLess(
            commands.index("set -euo pipefail"),
            tee,
            "pipefail must be set before the review is piped into tee",
        )


class VerdictGateTests(unittest.TestCase):
    """The gate must accept a real review and reject every 'no review happened'.

    The motivating case is measured, not hypothetical: on 2026-08-17 `claude -p`
    with no credential printed `Not logged in - Please run /login` and exited
    **0**, so an exit-code-only gate would have posted that as the review and
    passed green.
    """

    def setUp(self) -> None:
        self.pattern = gate_pattern()

    def assert_accepted(self, output: str, why: str) -> None:
        self.assertIsNotNone(self.pattern.search(output), why)

    def assert_rejected(self, output: str, why: str) -> None:
        self.assertIsNone(self.pattern.search(output), why)

    def test_a_genuine_review_passes(self) -> None:
        self.assert_accepted(
            "MEDIUM - `src/app/worktrees.rs:1499`: branch-only matching.\n\n"
            "VERDICT: 0 critical, 0 high, 1 medium, 0 low\n",
            "a real review must pass the gate",
        )

    def test_findings_never_veto_the_merge(self) -> None:
        # The gate enforces that verification happened, not that the model
        # approved. A model opinion must not block a merge.
        self.assert_accepted(
            "CRITICAL - foo.rs:1: bad\n\nVERDICT: 3 critical, 2 high, 1 medium, 4 low\n",
            "a review reporting critical findings still counts as a review",
        )

    def test_unauthenticated_cli_that_exits_zero_is_rejected(self) -> None:
        self.assert_rejected(
            "Not logged in \u00b7 Please run /login\n",
            "the measured exit-0 failure mode must not pass as a review",
        )

    def test_empty_and_errored_output_are_rejected(self) -> None:
        self.assert_rejected("", "empty output is not a review")
        self.assert_rejected("Error: rate limited (429)\n", "an API error is not a review")

    def test_unformatted_or_inline_verdicts_are_rejected(self) -> None:
        self.assert_rejected("Looks good to me!\n", "prose without a verdict is not a review")
        self.assert_rejected(
            "VERDICT: some critical, some high\n", "a non-numeric verdict is not a review"
        )
        self.assert_rejected(
            "the VERDICT: 0 critical, 0 high, 0 medium, 0 low\n",
            "the verdict must start its own line",
        )


if __name__ == "__main__":
    unittest.main()
