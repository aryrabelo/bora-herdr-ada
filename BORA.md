# herdr-bora

Our fork of [herdr](https://github.com/ogulcancelik/herdr) with local
improvements, shipped as a separate `herdr-bora` binary that coexists with a
stock `herdr` install (brew/mise) instead of overwriting it.

## What's different from upstream

- **omp (oh-my-pi) is a first-class detected agent.** Upstream only recognizes
  `omp` while its lifecycle integration is live; with no live integration it
  could not identify omp panes (no `Agent::Omp`, no detection manifest) and they
  fell back to a wrong/stale agent label, leaving the sidebar state stuck. The
  fork adds `Agent::Omp`, process/label recognition, and an `omp.toml` detection
  manifest keyed on the `π  /` status bar (working = braille spinner / `⟦esc⟧`).
  See commit `fix(detect): recognize omp (oh-my-pi) as a first-class agent`.

## Branch layout

- `upstream` -> upstream `ogulcancelik/herdr`.
- `origin`   -> our repo `aryrabelo/bora-herdr-ada`.
- `master` -> 1:1 mirror of upstream `master`. Never carries fork commits;
  only fast-forwarded (`scripts/bora sync`).
- `main`   -> default branch = `master` + our features, each landed as a
  squash-merged PR. Kept current by rebasing onto `master`.

## Build / install

`herdr-bora` is the fork's normal `herdr` binary installed under a different
name and run on its own session, so the two never collide.

```sh
scripts/bora build        # cargo build --release, install as ~/.local/bin/herdr-bora
scripts/bora run          # run it on the dedicated "bora" session (own socket)
```

> [!NOTE]
> **macOS 26 — prebuilt fallback required.** The vendored `libghostty-vt`
> requires zig 0.15.2, which cannot link against the macOS 26 SDK. zig 0.16 is
> rejected by the vendored `build.zig`. Upstream Ghostty's zig-0.16 migration
> ([PR #12726](https://github.com/ghostty-org/ghostty/pull/12726)) is still
> open/WIP, so a vendor-update does not help yet.
>
> **Local builds now work via a prebuilt static lib:**
>
> ```sh
> just fetch-libghostty-vt   # downloads prebuilt/libghostty-vt-<target>.a (gitignored)
> scripts/bora build         # build.rs auto-detects the prebuilt; cargo build --release
> ```
>
> `LIBGHOSTTY_VT_PREBUILT=<absolute path to .a>` overrides the cache path (highest priority).
>
> If you prefer a CI-built binary instead:
>
> ```sh
> scripts/bora ci-build     # dispatch the GitHub Actions macOS build on the fork
> gh run watch              # wait for it
> scripts/bora ci-install   # download the artifact, install as herdr-bora
> ```

### Refreshing / producing prebuilts

**Local, no-CI (fast dev loop here).** `just build-libghostty-vt-prebuilt`
cross-builds `prebuilt/libghostty-vt-<target>.a` for the macOS host target
inside a Linux container with zig 0.15.2 (zig 0.15.2 cannot link its own build
runner on macOS 26, but cross-builds the macOS `.a` fine from Linux). Requires
Docker; no GitHub Actions, no network beyond the zig download. After a vendor
update, just re-run it.

```sh
just build-libghostty-vt-prebuilt   # or: scripts/build_libghostty_vt_prebuilt.sh
scripts/bora build                  # build.rs auto-detects the prebuilt
```

**CI (other contributors / non-macOS targets).** The `libghostty-vt-prebuilts`
GitHub Actions workflow cross-builds `libghostty-vt.a` for each target using zig
0.15.2 and publishes them as assets on the `libghostty-vt-prebuilts` release,
keyed by the vendored commit (8-char prefix). After a vendor update, re-run that
workflow, then run `just fetch-libghostty-vt`.

**Removal condition:** when upstream's zig-0.16 port lands and we vendor-update to a
0.16-capable commit, delete the prebuilt fallback in `build.rs`, the
`fetch-libghostty-vt` and `build-libghostty-vt-prebuilt` just recipes,
`scripts/build_libghostty_vt_prebuilt.sh`, and the `libghostty-vt-prebuilts`
workflow, and return to a pure from-source build.

## Keeping current with upstream

`master` is a pristine mirror; `main` is rebased onto it.

```sh
scripts/bora sync   # fast-forward master to upstream, rebase main onto it
# review the rebase, then publish:
git push --force-with-lease origin main
```

Rebase conflicts are localized to the feature commits (`src/detect/*`,
`src/config/sound.rs`, `src/terminal/state.rs`, the worktree-action files,
`src/ui/sidebar.rs`, and the rebrand string changes). Resolve, finish the
rebase, then rebuild (`scripts/bora build`).

## Releases

Releases are tag-driven off `main`. Tag a `main` commit `vX.Y.Z` and push the
tag; `.github/workflows/release.yml` builds the four `bora-*` binaries
(`bora-linux-x86_64`, `bora-linux-aarch64`, `bora-macos-x86_64`,
`bora-macos-aarch64`), creates the GitHub release, and updates
`website/latest.json`.

## Coexistence model

- Distinct binary name (`herdr-bora`) — never overwrites stock `herdr`.
- Dedicated named session (`--session bora`) — its own socket/server, so it runs
  our binary independently of any stock-`herdr` server on the default session.
- Own config namespace: config/state/sessions/sockets live under `bora` /
  `bora-dev` (`app_dir_name`), so a bora install never clobbers a stock
  `herdr`. The `HERDR_*` env var names (including `HERDR_CONFIG_PATH`) are
  unchanged for plugin/agent compatibility.

## Workspace configuration — `.bora/settings.toml`

Per-project workspace configuration, modeled on Conductor's
`.conductor/settings.toml`. Lives at the repo root in `.bora/settings.toml` and
is read when a worktree/workspace is created. It supersedes the legacy
`.worktreeinclude` (which still works as a fallback when the file is absent or
has no `[files]` section). Unrelated to the separate `.bora.toml` config.

### Isolation contract

Each workspace is an isolated place for one agent to work:

- Code changes stay on that workspace's branch (one branch per workspace).
- File edits happen in that workspace's own working tree.
- Setup and run scripts execute from the workspace directory, with
  workspace-specific environment variables (see the env table below).
- App processes can bind a stable, workspace-specific `BORA_PORT`.
- Notes and handoffs live in the workspace's `.context/` folder, auto-created on
  provisioning and never committed. It is git-ignored by appending a `.context/`
  line to the shared `.git/info/exclude`.

> [!NOTE]
> Worktrees created off a shared `.git` all read the **same**
> `.git/info/exclude`. Any copied or symlinked meta file whose name is excluded
> there stays excluded in every worktree — no per-worktree `.gitignore`
> juggling. This is why `.context/` is excluded once, centrally.

### `[scripts]`

```toml
# .bora/settings.toml
[scripts]
# Runs once in the new worktree right after creation, before any agent is
# launched in it. A non-zero exit is surfaced as setup: "failed" in the
# `worktree create --json` result — creation does not silently continue.
setup = """
pnpm install
cp "$BORA_ROOT_PATH/.env" .env
pnpm run build
"""

# The project's dev command, executed by `bora workspace run`.
run = "pnpm dev --port $BORA_PORT"

# concurrent (default): every workspace may run `run` simultaneously, each on
#                       its own $BORA_PORT.
# exclusive:            starting `run` in one workspace stops the previous run
#                       (tracked via a pidfile at .git/info/bora-run.pid).
run_mode = "concurrent"
```

### `[files]` — copy vs symlink

```toml
[files]
# copy: snapshot taken at creation time (the legacy .worktreeinclude behavior).
#       Edits made to the root copy afterwards are NOT reflected in the worktree.
copy = ["CLAUDE.md", ".env.example"]

# symlink: a live view into the root checkout. The worktree entry is a symlink
#          whose target is the absolute path in the root repo, so later edits in
#          the root are visible immediately in every workspace.
symlink = [".claude", ".claude-plugin", "docs", "harness.toml", "hooks", "Plans.md"]
```

Semantics:

- **copy** = snapshot at creation. **symlink** = live view into the root
  checkout (target is an absolute path into `BORA_ROOT_PATH`).
- Existing paths in the worktree are **never overwritten** — a conflicting entry
  is logged and skipped. Missing sources are skipped too.
- **Precedence:** `.bora/settings.toml [files]` wins over `.worktreeinclude`.
  When `[files]` is present it fully governs file provisioning; when it is
  absent, `.worktreeinclude` is used and every listed entry is treated as
  `copy`. `.worktreeinclude` continues to work unchanged for projects without a
  settings file.

### Injected environment variables

Injected into both `setup` and `run` scripts:

| Variable | Value |
|---|---|
| `BORA_ROOT_PATH` | Main repo checkout path |
| `BORA_WORKSPACE_PATH` | This worktree's path |
| `BORA_WORKSPACE_ID` | The worktree directory name |
| `BORA_BRANCH` | The workspace branch (empty if unknown) |
| `BORA_PORT` | Stable per-branch port (only set when allocatable) |

`BORA_PORT` is resolved through a single function, so a workspace always sees one
port regardless of which surface asks — the `[[commands]]` UI, the `setup`/`run`
scripts, and `bora workspace run` all agree.

Resolution precedence:

1. If `.bora/settings.toml` defines `[ports]` (`base`/`max`, default
   `4100`–`4199`), the **stable persisted allocator** is used: a port is
   allocated once per branch and kept forever. New workspaces take the lowest
   free port in the range; when the range is exhausted `BORA_PORT` is not set.
   Assignments persist in `.git/info/bora-ports.json` (`{"<branch>": <port>}`).

   ```toml
   [ports]
   base = 4100
   max  = 4199
   ```

2. Otherwise, if legacy `.bora.toml` defines `[ports]`, its **index-based
   scheme** is used for compatibility: `base + index * per_worktree`, where
   `index` is the workspace's position among branch-sorted worktrees.

3. Otherwise, if `.bora/settings.toml` exists but has no `[ports]`, the
   persisted allocator is used with the default `4100`–`4199` range. With
   neither config present, `BORA_PORT` is not set.

### CLI

- `bora worktree create` gains `--no-setup` to skip the setup script. The
  `--json` output includes a `setup` field: `"ok"`, `"failed"`, or `"skipped"`
  (skipped when `--no-setup` is passed or no setup script is configured).
- `bora workspace run [--cwd PATH]` executes the `run` script in a workspace,
  respecting `run_mode`. With `run_mode = "exclusive"`, starting a run stops the
  previous one, tracked via a pidfile at `.git/info/bora-run.pid`.
