# NEXT_PROMPT — `.bora/settings.toml`: Conductor-style workspace isolation

## Goal

Add per-project workspace configuration to bora, modeled on Conductor's `.conductor/settings.toml`, under the name **`.bora/settings.toml`** at the repo root. Today `worktree create` only copies files listed in `.worktreeinclude` (see `src/worktree.rs::copy_worktree_includes`). Extend worktree/workspace creation with the following, keeping `.worktreeinclude` working as a fallback.

## Requirements

### 1. Isolation contract (document + enforce)

Each workspace gives an agent a separate place to work:
- Code changes stay on that workspace's branch.
- File edits happen in that workspace's working tree.
- Setup and run scripts execute from that workspace directory.
- App processes can use workspace-specific environment variables such as `BORA_PORT`.
- Notes and handoffs live in the workspace's `.context/` folder without being committed (auto-create it, ensure it is git-ignored via the shared `.git/info/exclude`).

### 2. `[scripts]` section

```toml
# .bora/settings.toml
[scripts]
setup = """
pnpm install
cp "$BORA_ROOT_PATH/.env" .env
pnpm run build
"""
run = "pnpm dev --port $BORA_PORT"
run_mode = "concurrent"   # or "exclusive"
```

- `setup`: runs once in the new worktree right after creation, before any agent is launched in it. Non-zero exit → surface failure in `worktree create --json` result (do not silently continue).
- `run`: the project's dev command; `run_mode = "concurrent"` means every workspace may run it simultaneously (each on its own `$BORA_PORT`); `"exclusive"` means starting it in one workspace stops it elsewhere.
- Env vars injected into both scripts:
  - `BORA_ROOT_PATH` — main repo checkout path
  - `BORA_WORKSPACE_PATH` — this worktree path
  - `BORA_WORKSPACE_ID`, `BORA_BRANCH`
  - `BORA_PORT` — stable per-workspace port allocated from a configurable range (collision-free across workspaces)

### 3. `[files]` section — copy vs symlink

```toml
[files]
copy = ["CLAUDE.md", ".env.example"]          # snapshot at creation (today's .worktreeinclude behavior)
symlink = [".claude", ".claude-plugin", "docs", "harness.toml", "hooks", "Plans.md"]  # live view of the root repo
```

- `symlink` entries: create relative or absolute symlinks pointing into the root checkout — macOS primary target, plain `std::os::unix::fs::symlink`.
- Precedence: `.bora/settings.toml [files]` > `.worktreeinclude` (legacy, treated as `copy`).
- Never overwrite an existing path in the worktree; log and skip.
- Symlinked/copied names that are excluded in the shared `.git/info/exclude` stay excluded (worktrees share it) — verify and document.

### 4. CLI surface

- `bora worktree create` gains `--no-setup` (skip setup script) and reports `{setup: "ok"|"failed"|"skipped"}` in `--json`.
- `bora workspace run` (new): execute the `run` script in a workspace respecting `run_mode` + `$BORA_PORT`.

## Context that motivated this

Orchestrating N omp agents in parallel worktrees (campaign-pipeline project): git-excluded meta files (CLAUDE.md, .claude/, docs/, harness.toml, Plans.md) did not exist in worktrees; workaround was a hand-rolled `link-meta.sh` + absolute paths in every agent prompt. Copies also go stale mid-run (SPRD edited in root not visible in worktree copies) — hence `symlink` as a first-class option.

## Definition of done

- `.bora/settings.toml` parsed (serde + toml, same style as existing config).
- setup/run scripts + env vars working; `.context/` auto-created and excluded.
- copy + symlink lists working with the precedence rules above.
- Unit tests mirroring `copy_worktree_includes_copies_listed_files` for: symlinks created, setup script env, port allocation stability, legacy `.worktreeinclude` fallback.
- Docs: section in BORA.md.
