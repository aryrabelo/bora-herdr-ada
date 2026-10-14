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
| `binary` | builds `cargo build --release --locked` and copies it to `~/.local/bin/bora` (a copy, not a symlink), then prints `bora --version` | always |
| `plugins` | installs the bora plugins listed in [scripts/install/plugins.txt](scripts/install/plugins.txt) with `bora plugin install <owner/repo> --yes`: `smarzban/herdr-file-viewer` (git-aware file viewer, `prefix+f`) and `qu8n/herdr-automatic-rename` (names tabs after directory, branch, host or running agent) | always |
| `omp` | runs `omp plugin install github:aryrabelo/omp-cmd-z-undo`: Ctrl+Z (and Cmd+Z) undo typing in the omp prompt instead of suspending | `omp` is on `PATH` |
| `ghostty` | writes `~/.config/ghostty/bora.conf` from [examples/opinionated/ghostty.conf](examples/opinionated/ghostty.conf) (rewritten on every install) and adds one line, `config-file = ?bora.conf`, to your Ghostty config | macOS with Ghostty installed |
| `ghostty-menu` | changes macOS user defaults for Ghostty (`defaults write com.mitchellh.ghostty NSUserKeyEquivalents`): moves the menu items "Show Next Tab", "Show Previous Tab" and "Reload Configuration" to `cmd+ctrl+]`, `cmd+ctrl+[` and `cmd+ctrl+shift+,`, because menu shortcuts resolve before Ghostty's keybinds and would otherwise eat `cmd+shift+]`, `cmd+shift+[` and `cmd+shift+,`. The keys are the titles the menu actually shows, so they are resolved for your languages ([scripts/install/ghostty-menu-titles.js](scripts/install/ghostty-menu-titles.js)); if a title can't be resolved, the component fails instead of writing a key the menu ignores. A different existing shortcut prints `replacing <old> -> <new>`. Takes effect after quitting Ghostty (Cmd+Q) and relaunching | macOS with Ghostty installed |
| `config` | creates `~/.config/bora/config.toml` from [examples/opinionated/config.toml](examples/opinionated/config.toml) when it does not exist; an existing config is never edited, the installer only lists the template keys it lacks | always |

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
directories, `~/.omp/plugins`, `~/Library/Preferences/<domain>.plist`. If any
of them is a symlink (e.g. into a dotfiles repo), that step prints
`skipped (<path> is a symlink to <target>; not writing through it)` and the
exact line or command to run yourself. `$HOME` and its ancestors are not
checked. The `binary` component is the exception: it writes cargo's target
directory and `~/.local/bin/bora`.

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
`<owner/repo[/subdir]> [plugin_id]`.

Personal configuration (machine paths, private scripts, wrappers) stays out of
this repo; only generic pieces belong in the templates.

## What bora adds

| Feature | How to use | Docs |
|---|---|---|
| Separate `bora` binary that coexists with `herdr` | `bora` has its own config/state dir `~/.config/bora`; `HERDR_NAMESPACE=<name>` points it at another namespace (e.g. `herdr`) | [BORA.md](BORA.md#coexistence-model) |
| Fork version identity | `bora --version` prints `bora <fork version> (v<upstream release>[<upstream sha>].bora-<n>)` | [examples/README.md](examples/README.md) |
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
