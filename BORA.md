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
