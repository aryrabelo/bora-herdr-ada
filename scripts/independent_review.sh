#!/usr/bin/env bash
# Independent review: a model that did not write the code reviews the pushed commit.
#
# Why this is a script and not inline workflow YAML: a gate you cannot run
# locally is a gate nobody tests. Run it by hand with
#   BASE_SHA=main HEAD_SHA=HEAD REVIEW_CMD='omp -p --model <spec>' scripts/independent_review.sh
#
# Why it reads SHAs and not the working tree: the working tree is the author's
# directory, not the thing being merged. AGENTS.md records an incident where a
# gate run against the tree reported green while the commit was red. This diffs
# BASE...HEAD so it measures the commit, exactly like CI does.
set -euo pipefail

BASE_SHA="${BASE_SHA:?BASE_SHA is required (the PR base commit)}"
HEAD_SHA="${HEAD_SHA:?HEAD_SHA is required (the PR head commit)}"
# The reviewer must not be the author. This default is the spec this script was
# actually exercised against (a different family from the Claude models that
# write code here); CI overrides it via REVIEW_MODEL / REVIEW_CMD.
REVIEW_MODEL="${REVIEW_MODEL:-openai-codex/gpt-5.6-terra}"
REVIEW_CMD="${REVIEW_CMD:-omp -p --no-extensions --model ${REVIEW_MODEL}}"

# Model context is finite; an unbounded diff silently degrades the review into
# a review of whatever fit. Truncate loudly instead.
MAX_DIFF_BYTES="${MAX_DIFF_BYTES:-180000}"

diff_text="$(git diff --no-color "${BASE_SHA}...${HEAD_SHA}")"

if [ -z "$diff_text" ]; then
	echo "No changes between ${BASE_SHA} and ${HEAD_SHA}; nothing to review."
	exit 0
fi

diff_bytes=${#diff_text}
truncated_note=""
if [ "$diff_bytes" -gt "$MAX_DIFF_BYTES" ]; then
	diff_text="${diff_text:0:$MAX_DIFF_BYTES}"
	truncated_note="

NOTE: the diff was truncated at ${MAX_DIFF_BYTES} of ${diff_bytes} bytes. Review
only what is shown, and say plainly at the top that the review is partial."
fi

prompt="You are reviewing a pull request. You did NOT write this code and have no
stake in it. Your job is to find defects, not to praise the change.

Rules for your review:
- Cite every finding as \`path:line\`, taken from the diff hunk headers.
- Rank each finding: CRITICAL (data loss, security, corruption), HIGH (wrong
  behavior users hit), MEDIUM (edge case, missing error path), LOW (clarity).
- If the diff does not give you enough context to judge something, say
  \"cannot verify from the diff\" and name what you would need. Never guess and
  never invent a line number.
- Ignore formatting and style; linters already cover those.
- If you find nothing of substance, say so in one line. A short honest review
  beats a padded one.
- End with a line of exactly: VERDICT: <n> critical, <n> high, <n> medium, <n> low

Diff (${BASE_SHA}...${HEAD_SHA}):

\`\`\`diff
${diff_text}
\`\`\`${truncated_note}"

# Why no pipefail-hiding pipe here: the review command's own exit status is the
# signal that the verification ran at all, and it must not be masked.
printf '%s' "$prompt" | ${REVIEW_CMD}
