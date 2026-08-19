---
description: "Safety preamble for dispatching subagents, workers, or orchestration units. Read before any fan-out, worktree dispatch, or delegated write work."
---

# Worker safety preamble

Bake these into EVERY dispatched unit's brief (they must be in-context for the worker, not in a file it might skip):

- Explicit staging only: NEVER `git add -A` / `git add .`.
- NEVER `--amend`, NEVER `--no-verify`, NEVER delete worktrees.
- Push only where the brief explicitly authorizes.
- Stay inside the owned paths declared in the brief; never touch the trunk branch, shared plan files, or another unit's paths.
- Scope is IN/OUT explicit: the brief lists what is in scope AND what is out; out-of-scope discoveries are reported, not fixed.
- Gates are echoed: run the unit's verification commands and paste their real output into the report/handoff.
- Handoff artifact: end with decisions made, gates PASS/FAIL, and anything the verifier must re-run.
