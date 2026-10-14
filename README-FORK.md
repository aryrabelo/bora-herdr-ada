# bora: what this fork adds to herdr

bora is a fork of [herdr](https://github.com/herdrdev/herdr). It tracks
upstream closely (`master` mirrors upstream, `main` = `master` + fork commits,
see [BORA.md](BORA.md)) and ships as a separate `bora` binary. This page lists
what bora has that upstream herdr does not, how to use each piece, and what
the fork deliberately removed. Everything upstream documents still applies;
upstream's docs live in [docs/next/website/src/content/docs/](docs/next/website/src/content/docs/).

## Install

Prerequisites: a Rust toolchain (`cargo`), [just](https://github.com/casey/just),
`git`, and `python3`. The vendored `libghostty-vt` needs zig 0.16.0, or the
prebuilt static lib (`just fetch-libghostty-vt`); [BORA.md](BORA.md#build--install)
explains when you need which.

```sh
git clone https://github.com/aryrabelo/bora-herdr-ada.git bora
cd bora
just fetch-libghostty-vt   # unless you build libghostty-vt with zig 0.16.0
just install
```

On a terminal, `just install` asks one question first:

```
Install the suggested setup? [Y/n/customize] (n = binary only):
```

- **Y** installs the binary plus every suggested component available on this
  machine (the list is printed above the question).
- **n** installs only the binary: a plain fork, nothing opinionated.
- **customize** opens a checklist with the suggested components ticked;
  toggle by number, Enter installs.

Nothing beyond the binary is applied without that choice. Every component is
idempotent and prints `<thing>: installed | already present | skipped (<reason>)`.
A component you selected that cannot run (no `cargo`, no `bora` for
`plugins`, a build error) prints `failed (<reason>)` and makes `just install`
exit non-zero.

| Component | What it changes on disk | Suggested when |
|---|---|---|
| `binary` | builds `cargo build --release --locked` and copies it to `~/.local/bin/bora` (a copy, not a symlink), then prints `bora --version`; also links `~/.local/bin/herdr -> bora` when no `herdr` is there, because plugins written for upstream (gh-pr) call `herdr` by name and fail silently without it. An existing `herdr` is never replaced | always |
| `plugins` | installs the bora plugins listed in [scripts/install/plugins.txt](scripts/install/plugins.txt) with `bora plugin install <owner/repo> --yes`: file viewer, automatic rename, Collie and gh-pr (see [Bundled plugins](#bundled-plugins)). A plugin whose `requires=` command is missing (Bun, `gh`) is skipped with the reason, not failed | always |
| `omp` | runs `omp plugin install` for each entry in [scripts/install/omp-plugins.txt](scripts/install/omp-plugins.txt): today `omp-cmd-z-undo` (Ctrl+Z and Cmd+Z undo typing in the omp prompt instead of suspending) and `omp-mermaid` (collapsible ASCII render of mermaid blocks, see [Bundled plugins](#bundled-plugins) for its costs) | `omp` is on `PATH` |
| `ghostty` | writes `~/.config/ghostty/bora.conf` from [examples/opinionated/ghostty.conf](examples/opinionated/ghostty.conf) (rewritten on every install) and adds one line, `config-file = ?bora.conf`, to your Ghostty config | macOS with Ghostty installed |
| `ghostty-menu` | changes macOS user defaults for Ghostty (`defaults write com.mitchellh.ghostty NSUserKeyEquivalents`): moves the menu items "Show Next Tab", "Show Previous Tab" and "Reload Configuration" to `cmd+ctrl+]`, `cmd+ctrl+[` and `cmd+ctrl+shift+,`, because menu shortcuts resolve before Ghostty's keybinds and would otherwise eat `cmd+shift+]`, `cmd+shift+[` and `cmd+shift+,`. The keys are the titles the menu actually shows, so they are resolved for your languages ([scripts/install/ghostty-menu-titles.js](scripts/install/ghostty-menu-titles.js)); if a title can't be resolved, the component fails instead of writing a key the menu ignores. A different existing shortcut prints `replacing <old> -> <new>`. Takes effect after quitting Ghostty (Cmd+Q) and relaunching | macOS with Ghostty installed |
| `config` | creates `~/.config/bora/config.toml` from [examples/opinionated/config.toml](examples/opinionated/config.toml) when it does not exist; an existing config is never edited, the installer only lists the template keys it lacks | always |
| `autoupdate` | schedules [scripts/autoupdate.sh](scripts/autoupdate.sh) from this checkout daily at 05:00 local time (see [Updating](#updating)): on macOS writes the user LaunchAgent `~/Library/LaunchAgents/dev.bora.autoupdate.plist` (`RunAtLoad` false, `PATH` captured from the shell that ran `just install`) and loads it with `launchctl bootstrap gui/<uid>`; on Linux writes `bora-autoupdate.service` and `bora-autoupdate.timer` to `~/.config/systemd/user/` and runs `systemctl --user enable --now bora-autoupdate.timer`. Rewritten and reloaded only when the content changed. Writes nothing when nix-darwin already loads `org.nixos.bora-autoupdate` | `cargo` on `PATH`, and macOS with `launchctl` or Linux with `systemctl --user`, unless `org.nixos.bora-autoupdate` is loaded |

The split between the three layers: **Ghostty** stops consuming Cmd chords
(`keybind = cmd+t=unbind`, ...), turns Option into Alt and maps Cmd+Z to
Ctrl+_; **bora's config** binds those chords (`cmd+t` new tab, `cmd+d` split,
`cmd+shift+]` next workspace, `alt+a` next agent, `prefix+f` file viewer);
**omp** gets the undo extension that Ctrl+_ / Ctrl+Z land on. The macOS menu
remap (`ghostty-menu`) is the part a config file cannot do; the manual
commands are in [examples/README.md](examples/README.md) step 4.

Nothing is written through a symlink. Before every write or `mkdir`, and
before running `bora plugin install` or `omp plugin install`, the component
checks the target path and each parent directory below `$HOME`: the file
itself, `~/.config`, `~/.config/ghostty`, bora's config and state
directories, `~/.omp/plugins`, `~/Library/Preferences/<domain>.plist`,
`~/Library/LaunchAgents`, `~/.config/systemd/user`. If any
of them is a symlink (e.g. into a dotfiles repo), that step prints
`skipped (<path> is a symlink to <target>; not writing through it)` and the
exact line or command to run yourself. `$HOME` and its ancestors are not
checked. The `binary` component is the exception: it writes cargo's target
directory, `~/.local/bin/bora` and, when no `herdr` is there, the
`~/.local/bin/herdr` link.

### Non-interactive use

With no flags and stdin or stdout not a terminal (agents, pipes, CI),
`just install` installs **only the binary** without prompting, like the old
recipe. If it cannot (no `cargo`, build error) it exits non-zero, so
`just install && bora server live-handoff` never hands the server to a stale
binary.

```sh
just install --list            # components, availability, suggestion; changes nothing
just install --yes             # the suggested setup, no prompt
just install --all             # every component available here
just install --only plugins,config
just install --yes --without ghostty-menu
just bootstrap                 # = --yes --without binary
```

Paths follow bora's own resolution: `$XDG_CONFIG_HOME` (else `~/.config`),
`$HERDR_NAMESPACE` (else `bora`) and `$HERDR_CONFIG_PATH`.

### Adding an install component

Drop one file in [scripts/install/components/](scripts/install/components/),
named `<NN>-<name>.sh`; files run in filename order, and nothing else needs
registering. The file sets `NAME` and `DESCRIPTION` (one line: what it changes
on disk) and defines `install_component`. Optionally define `available`
(print the reason and return non-zero when it cannot run on this machine) and
`default_on` (return non-zero when it is not part of the suggested setup).
Each function runs in its own subshell with
[scripts/install/lib.sh](scripts/install/lib.sh) loaded (`say`, `note`,
`bora_bin`, `bora_config_dir`, `bora_config_file`, `bora_state_dir`,
`ghostty_config_dir`, `is_macos`, `resolve_link`, `symlinked_component`,
`refuse_symlink`), `$ROOT` set to the repo, and `set -e` on. Guard every
write with `refuse_symlink "<thing>" <path>`. The
function is `install_component`, not `install`, so it does not shadow
`install(1)`. Adding a bora plugin is one line in
[scripts/install/plugins.txt](scripts/install/plugins.txt):
`<owner/repo[/subdir]> [plugin_id] [requires=<cmd>,...]`; an omp plugin is one
line in [scripts/install/omp-plugins.txt](scripts/install/omp-plugins.txt):
`<source> <package-name> [linked=<path under $HOME>]`. Either way, add its row
to [Bundled plugins](#bundled-plugins) in the same change.

### Bundled plugins

An enabled bora plugin receives `HERDR_SOCKET_PATH`, which is the reach of the
whole CLI (prompt agents, send input, close workspaces), and its startup and
event hooks run with nobody watching. An omp plugin runs inside the omp
process with omp's permissions. Bundling one installs that on the machine of
everyone who runs `just install`, so every entry carries this review
(checked 2026-10-14 against the installed checkout):

| Plugin | Maintainer | Runs | Reaches |
|---|---|---|---|
| `herdr-file-viewer` | smarzban | install: downloads its prebuilt binary from the repo's GitHub releases, else builds with `cargo`. After that, panes and actions only, on `prefix+f` | files under the chosen root, read-only; `git` |
| `herdr-automatic-rename` | qu8n | a startup hook plus hooks on ~20 workspace, worktree, tab and pane events, all `bash automatic-rename.sh` | the bora socket (renames tabs and workspaces); `git` in the pane's cwd |
| `herdr.collie` | AltanS (MIT, ~1.3k★) | install: a Bun build. After that, nothing until the "Start Collie" action, which starts a background service (launchd/systemd `--user`) | when started: a local HTTP server published on your tailnet with `tailscale serve`. **Anyone on that tailnet can read your agents and send them text.** "Uninstall Collie" removes the service and the serve |
| `gh-pr` | wyattjoh (no licence file) | `bun bin/update-pr-status.ts` on every pane focus and worktree open/create | `git` in the pane's cwd; `gh pr view`/`gh pr checks` with your GitHub auth; `herdr pane report-metadata` |
| `omp-cmd-z-undo` | aryrabelo | an omp extension: at TUI session start it wraps the prompt editor and rebinds Ctrl+Z and Cmd+Z to undo, in memory only (your keybindings file is never written). Ctrl+Z stops suspending omp, and is also taken off `app.tools.expand`, `app.editor.external` and `app.message.copy` wherever those actions are checked | omp's in-memory keybindings and editor component; no file writes, no network, no subprocess |
| `omp-mermaid` | aryrabelo, fork of Gurpartap/pi-mermaid (MIT) | an omp extension: on every user input and at the end of every agent turn it scans for ```` ```mermaid ```` blocks and adds a collapsible top-down render (`ctrl+o` expands it, with the source) | in-process: the work runs in the `mermaid` parser and `beautiful-mermaid`; its own code (`index.ts`) makes no network call and spawns nothing |

Two costs of `omp-mermaid`, measured 2026-10-14 on omp 18.10.4. omp already
draws mermaid blocks natively in user and assistant messages, so with the
plugin each diagram shows twice: the native render inline, then the plugin's
box. And the install pulls its `@oh-my-pi/*` peer packages as real copies,
about 1.1 GB under `~/.omp/plugins`. It is the fork rather than upstream
`pi-mermaid` because upstream also loads under omp but its expand hint loses
the key (`35 more lines,  to expand`, omp #2998) and its peers are a second,
standalone pi stack (369 MB).

Personal configuration (machine paths, private scripts, wrappers) stays out of
this repo; only generic pieces belong in the templates.

## Updating

**Source installs** (a clone plus `cargo`, installed with `just install`):
update from git and rebuild, by hand

```sh
git pull && just install && bora server live-handoff
```

or every day with the `autoupdate` component (`just install --only autoupdate`),
which runs [scripts/autoupdate.sh](scripts/autoupdate.sh) from that clone at
05:00. On `main` with no uncommitted changes, tracking a remote's `main`
(e.g. `origin/main`), it fetches and fast-forwards; when `HEAD` did not move
and `~/.local/bin/bora` exists it logs `up to date` and builds nothing;
otherwise it runs `scripts/install.sh --only binary` and, if
`bora status server` reports a running server, `bora server live-handoff`.
Another branch, uncommitted changes, an upstream that is not a remote's
`main`, or local commits not in upstream: it logs why and touches nothing. A
failed build keeps the old binary, and a failed build or handoff is retried
by the next run. A clone without `cargo` (or `git`) on the job's `PATH`
fails loudly and never falls back to `bora update`; the component is
unavailable when the shell running `just install` has no `cargo`, because
that `PATH` is the one the job gets. Every run is logged to
`~/.local/state/bora/autoupdate.log` (`$XDG_STATE_HOME/bora/` when set); run
`scripts/autoupdate.sh` by hand anytime, two runs never overlap.

**Release installs** (no `cargo`): the install script downloads the latest
release binary to `${HERDR_INSTALL_DIR:-$HOME/.local/bin}/bora` (first
install, or a clean reinstall), and `bora update` replaces it in place:

```sh
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/distribution/install.sh | sh
bora channel set stable
bora update --handoff
```

`--handoff` moves a running server onto the new binary. `scripts/autoupdate.sh`
runs exactly `bora update --handoff` only when it is not inside a git
checkout.

The built-in check does not install anything: a running server checks every
30 minutes and only tells you a new version is available
(`[update] version_check`, default `true`). `[update] manifest_check` (default
`true`) refreshes agent-detection manifests on the same interval; it never
touches the binary. Set either to `false` to turn it off.

Never run `bora update` on a source build on the preview channel
(`bora channel show` prints yours). The preview channel compares build IDs,
not versions, and a locally built binary has none, so it is always
"outdated" and `bora update` replaces it with the latest preview release: a
downgrade (measured 2026-09-15: a 0.47.0 source build was offered
0.46.8-preview). That is why `scripts/autoupdate.sh` never calls
`bora update` when it builds from source.

## What bora adds

| Feature | How to use | Docs |
|---|---|---|
| Separate `bora` binary that coexists with `herdr` | `bora` has its own config/state dir `~/.config/bora`; `HERDR_NAMESPACE=<name>` points it at another namespace (e.g. `herdr`) | [BORA.md](BORA.md#coexistence-model) |
| Fork version identity | `bora --version` prints `bora <upstream herdr release>+<bora version> (herdr <upstream sha>)`, e.g. `bora 0.9.5+0.52.11 (herdr 2f3432c6)` | [examples/README.md](examples/README.md) |
| omp (oh-my-pi) as a first-class agent | upstream identifies omp only while its integration is live; bora also detects it from its `π  /` status bar (`src/detect/manifests/omp.toml`). `bora integration install omp` adds hook-reported state; `bora agent start <name> --kind omp --pane <id>` | [agents.mdx](docs/next/website/src/content/docs/agents.mdx) |
| Opinionated setup | `just install` (see Install above) | this file |
| Folders sidebar view | `[ui] view_mode = "folders"`, or cycle Flat → Folders → Repo with `prefix+shift+v`; drag a workspace onto a folder to join it; folder right-click menu renames and nests; `bora workspace set-group <workspace_id> [name]` | [docs/next/CHANGELOG.md](docs/next/CHANGELOG.md) |
| `#channels` between agents | `bora channel create`, `bora channel send <name> <text> [--to <nick>] [--when-idle]`, `bora channel ask <name> <nick> <text>` (blocks for the reply), `bora channel note <name> <text>` (no bell), `bora channel reply <name> <seq> <text>`, `bora channel tail` | [agent-automation.mdx](docs/next/website/src/content/docs/agent-automation.mdx), [cli-reference.mdx](docs/next/website/src/content/docs/cli-reference.mdx) |
| Chat view and passive human-seat delivery | `[ui] chat_view = true`, `chat_name = "<you>"`; open with `prefix+i`. Messages to you never switch your view: the channel row gets a `chat_unread` token and unanswered asks count in the `N waiting` badge | [agent-automation.mdx](docs/next/website/src/content/docs/agent-automation.mdx) (Chat view) |
| When-idle prompt queue | `bora agent prompt <target> <text> --when-idle` queues server-side until the agent stops working | [agent-automation.mdx](docs/next/website/src/content/docs/agent-automation.mdx) |
| One-command dispatch | `bora worktree create --branch <name> --agent <kind> --prompt <text>` creates worktree, workspace, folder, agent and prompt in one go | [cli-reference.mdx](docs/next/website/src/content/docs/cli-reference.mdx) |
| Worktree `.context/` | every worktree bora creates gets a git-excluded `.context/` for notes and handoffs | [BORA.md](BORA.md#worktree-creation) |
| Manual status pin | pane right-click menu → Status: Working / Blocked / Idle / Done / Auto; socket method `pane.set_status` | [socket-api.mdx](docs/next/website/src/content/docs/socket-api.mdx) |
| Blocked hold | a pane that was blocked keeps reading blocked (pane list, rollups, `N waiting`) until its agent works again, so a dismissed prompt does not look finished | no user doc yet; contract in [AGENTS.md](AGENTS.md) (manual pane status note) |
| Per-pane done | a finished pane in a background split keeps its Done mark until that pane itself is focused; `bora pane mark-unseen <pane_id>... [--all]` marks panes unread again | [cli-reference.mdx](docs/next/website/src/content/docs/cli-reference.mdx) |
| Idle tiers and agent panel order | `[ui] idle_fresh_minutes` (default 15) and `idle_stale_minutes` (default 120) color idle agents and sink stale ones; `agent_panel_sort = "spaces" \| "priority" \| "recent"`, cycled with `prefix+a` | no user doc yet; key docs in `src/config/model.rs` |
| Copy pane reference | pane right-click menu → Copy reference copies `<workspace label> <pane_id>` for another agent's prompt | [docs/next/CHANGELOG.md](docs/next/CHANGELOG.md) |
| MCP server | `bora mcp serve [--channels <a,b>] [--nick <name>] [--allow-prompt]` exposes channel and agent tools over stdio | [examples/README.md](examples/README.md) |
| Pane process info | `bora pane process-info --pane <id>` (foreground process and tty) | [cli-reference.mdx](docs/next/website/src/content/docs/cli-reference.mdx) |
| No-reflow resize | `[terminal] reflow_on_resize = false` truncates instead of re-wrapping what a program drew | [configuration.mdx](docs/next/website/src/content/docs/configuration.mdx) |
| Example plugins | `bora plugin link examples/bora/plugins/<name>` for `gitui`, `pane-timer`, `auto-group`, `windhover-push` | [examples/bora/plugins/AGENTS.md](examples/bora/plugins/AGENTS.md) |

## Deliberately removed

These shipped in bora once and were taken out on purpose; the reasons and the
conditions for revisiting each are in [AGENTS.md](AGENTS.md) ("Removed — do not
reintroduce") and [docs/next/CHANGELOG.md](docs/next/CHANGELOG.md).

- `ui.chat_open_on_mention` (chat view auto-opening on a mention): replaced by passive delivery.
- Project view (`view_mode = "project"`, `projects.yml`, `project.*` verbs).
- Sandbox orchestrator, shared todo/scratchpad stores and their MCP tools.
- Right panel (Changes/Checks/Issues/PRs tabs) and its `[github]`/`[checks]`/`[flow]` polling.
- Per-pane channel scope (`channel join --scope-write/--scope-read`).
- `.bora.toml` / `.bora/settings.toml` per-repo command menu and worktree provisioning (now an external plugin).
- PR/issue/kanban board in core: it stays an external plugin.
