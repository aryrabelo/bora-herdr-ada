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

- `origin` -> upstream `ogulcancelik/herdr` (`master`).
- `fork`   -> our GitHub fork.
- `bora`   -> long-lived branch = upstream `master` + our patches. We merge
  upstream into it from time to time (`scripts/bora update`).

## Build / install

`herdr-bora` is the fork's normal `herdr` binary installed under a different
name and run on its own session, so the two never collide.

```sh
scripts/bora build        # cargo build --release, install as ~/.local/bin/herdr-bora
scripts/bora run          # run it on the dedicated "bora" session (own socket)
```

> [!NOTE]
> On macOS 26 the local native build is currently blocked: the vendored
> `libghostty-vt` requires zig 0.15.2, which cannot link against the macOS 26
> SDK, while zig 0.16 is rejected by the vendored `build.zig`. Until the
> toolchain is sorted, build on CI:
>
> ```sh
> scripts/bora ci-build     # dispatch the GitHub Actions macOS build on the fork
> gh run watch              # wait for it
> scripts/bora ci-install   # download the artifact, install as herdr-bora
> ```

## Keeping current with upstream

```sh
scripts/bora update       # git fetch origin + merge origin/master into bora
scripts/bora build        # or ci-build, then ci-install
```

Resolve any merge conflicts (our patches are small and localized to
`src/detect/*`, `src/config/sound.rs`, `src/terminal/state.rs`), then rebuild.

## Coexistence model

- Distinct binary name (`herdr-bora`) — never overwrites stock `herdr`.
- Dedicated named session (`--session bora`) — its own socket/server, so it runs
  our binary independently of any stock-`herdr` server on the default session.
- Shared `~/.config/herdr/config.toml` (same themes/keys); override with
  `HERDR_CONFIG_PATH` if you want fully separate config.
