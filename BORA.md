# bora

Our fork of [herdr](https://github.com/ogulcancelik/herdr) with local
improvements, shipped as a `bora` binary that coexists with a
stock `herdr` install (brew/mise) instead of overwriting it.

## What's different from upstream

- **omp (oh-my-pi) is a first-class detected agent.** Upstream only recognizes
  `omp` while its lifecycle integration is live; with no live integration it
  could not identify omp panes (no `Agent::Omp`, no detection manifest) and they
  fell back to a wrong/stale agent label, leaving the sidebar state stuck. The
  fork adds `Agent::Omp`, process/label recognition, and an `omp.toml` detection
  manifest keyed on the `π  /` status bar (working = braille spinner / `⟦esc⟧`).
  See commit `fix(detect): recognize omp (oh-my-pi) as a first-class agent`.

- **Worktree creation auto-provisions `.context/`.** Every worktree bora
  creates gets a `.context/` scratch dir for notes and handoffs, git-excluded
  centrally rather than per worktree. See "Worktree creation" below.

## Branch layout

- `upstream` -> upstream `herdrdev/herdr` (the org was renamed from
  `ogulcancelik/herdr`; the old path still redirects).
- `origin`   -> our repo `aryrabelo/bora-herdr-ada`.
- `master` -> 1:1 mirror of upstream `master`. Never carries fork commits;
  only fast-forwarded (`scripts/bora sync`).
- `main`   -> default branch = `master` + our features. Kept current by
  merging `master` in (never rebasing — see "Keeping current with upstream").

### Worktree location (learned 2026-08-11, binding)

Task worktrees for this repo live at `~/Sites/worktrees/bora/<slug>/` — the
machine-wide convention is `~/Sites/worktrees/{repo}/{slug}/`. This overrides
root `AGENTS.md`'s upstream `../herdr-worktrees/<task-slug>` layout, which does
not apply on this workstation. Never create sibling worktrees next to the
checkout (e.g. `~/Sites/bora-<something>`).

```sh
git worktree add ~/Sites/worktrees/bora/<slug> <ref>
# when done:
git worktree remove ~/Sites/worktrees/bora/<slug>
```

## Build / install

`bora` is the fork's normal `herdr` binary installed under a different
name and run on its own session, so the two never collide.

```sh
scripts/bora build        # cargo build --release, install as ~/.local/bin/bora
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
> scripts/bora ci-install   # download the artifact, install as bora
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

`master` is a pristine mirror; `main` carries the fork's work and takes
`master` in by **merge**, never rebase — `main` already holds merge commits
from prior syncs, so `git merge-base` finds the true delta and only genuinely
new upstream commits need conflict resolution. A rebase replays `main`'s entire
unique history instead and explodes into hundreds of conflicts. (learned
2026-08-05, binding; enforced in `scripts/bora sync`.)

```sh
scripts/bora sync   # fast-forward master to upstream, then merge it into main
# resolve conflicts, run `just check`, then publish:
git push origin main
```

Conflicts cluster in the fork's own surfaces (`src/detect/*`,
`src/config/sound.rs`, `src/terminal/state.rs`, the worktree-action files,
`src/ui/sidebar.rs`, `src/ui/dialogs.rs`, and the rebrand string changes —
including `bora-*` release asset names and `CARGO_BIN_EXE_bora`). Resolve,
commit the merge, then rebuild (`scripts/bora build`).

## Releases

Releases are tag-driven off `main`. Tag a `main` commit `vX.Y.Z` and push the
tag; `.github/workflows/release.yml` builds the four `bora-*` binaries
(`bora-linux-x86_64`, `bora-linux-aarch64`, `bora-macos-x86_64`,
`bora-macos-aarch64`), creates the GitHub release, and updates
`website/latest.json`.

## Coexistence model

- Distinct binary name (`bora`) — never overwrites stock `herdr`.
- Dedicated named session (`--session bora`) — its own socket/server, so it runs
  our binary independently of any stock-`herdr` server on the default session.
- Own config namespace: config/state/sessions/sockets live under `bora` /
  `bora-dev` (`app_dir_name`), so a bora install never clobbers a stock
  `herdr`. The `HERDR_*` env var names (including `HERDR_CONFIG_PATH`) are
  unchanged for plugin/agent compatibility.

## Worktree creation

Two provisioning steps run on every worktree bora creates. Both are
unconditional — no per-repo config file is involved (`src/worktree.rs`).

- **`.worktreeinclude` file copy.** When the source repo root holds a
  `.worktreeinclude`, every non-empty, non-`#` line is read as a path relative
  to that root and copied into the new checkout, creating parent directories as
  needed. A line whose source is not a regular file is skipped, and a failed
  copy is logged without failing creation. It is a copy, not a link, so later
  edits to the root file are not visible in the worktree.
- **`.context/` scratch dir.** Created in the new checkout, and excluded from
  git by appending a `.context/` line to `<git common dir>/info/exclude`.
  Repeated calls add nothing; two worktree creations racing in the same repo
  can leave a duplicate line, which git ignores just the same.

> [!NOTE]
> Worktrees created off a shared `.git` all read the **same** `info/exclude`.
> Anything excluded there stays excluded in every worktree, which is why
> `.context/` is excluded once, centrally, instead of through a per-worktree
> `.gitignore`.

Per-project setup scripts, copy/symlink file provisioning, per-workspace port
allocation and the `[[commands]]` menu are not core surface. They live in the
plugin [`aryrabelo/bora-local-commands`](https://github.com/aryrabelo/bora-local-commands),
which hooks `worktree.created` for provisioning and contributes the command
menu. `docs/next/CHANGELOG.md` records what left core and how to migrate.
