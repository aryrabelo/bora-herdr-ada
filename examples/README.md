# bora example setup

## 1. What you get

bora is a terminal agent multiplexer — a fork of [herdr](https://github.com/herdrdev/herdr) that tracks upstream closely and layers its own additions on top: agent-to-agent channels with a native chat view (`bora channel`), sidebar repo grouping, a "Programs" launcher band, configurable agent-panel scope/sort, rendering/full-repaint fixes, and an MCP server (`bora mcp serve`) for exposing bora itself to an MCP-client harness. Ary runs [OMP (oh-my-pi)](https://github.com/can1357/oh-my-pi) coding agents inside it day to day, which is what this example setup is built around — but bora detects and drives most terminal coding agents (Claude Code, Codex, OpenCode, and more; see `src/detect/manifests/`), not just OMP. This directory is a working example of the outer-terminal setup bora is built to run inside: [Ghostty](https://ghostty.org) as the terminal, wired so its keybinds and menu don't eat bora's chords, plus example `bora` and OMP configs you can copy as a starting point.

You can stop after step 2 with a working `bora` install. Steps 3–6 build the recommended Ghostty + bora + OMP setup on top of it.

## 2. Install bora

```sh
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/install.sh | sh
```

This is this fork's own installer (`website/install.sh`), not `herdr.dev/install.sh` — that one installs upstream herdr, a different project. The script:

- downloads the release binary matching your platform from this fork's GitHub releases,
- installs it as `bora` at `${HERDR_INSTALL_DIR:-$HOME/.local/bin}` — set `HERDR_INSTALL_DIR` to install elsewhere.

Make sure that directory is on your `PATH`, then confirm:

```sh
bora --version
```

This prints something like:

```
bora 0.24.0 (v0.8.1[a5c69bea].bora-24)
```

The first part (`0.24.0`) is bora's own release version. The parenthesized part is the fork identity: `v0.8.1` is the upstream herdr release this fork's `master` branch is merged up to, `[a5c69bea]` is the upstream commit that merge brought in, and `.bora-24` is this fork's own build number on top of that base.

Prebuilt binaries currently cover Linux and macOS (x86_64 and aarch64). Windows support is landing but is not in every release yet — check [GitHub releases](https://github.com/aryrabelo/bora-herdr-ada/releases) for your platform.

To build from source instead (e.g. to track `main` day to day), see the [root README's install section](../README.md#install) for full details. In short: `git clone` the repo, run `just fetch-libghostty-vt` to pull the prebuilt `libghostty-vt` static lib (needs a Rust toolchain, `just`, and `python3` on your `PATH`), then `cargo build --release` and symlink `target/release/bora` onto your `PATH`. This example setup does not depend on which install method you used.

## 3. Install Ghostty, and why

We recommend [Ghostty](https://ghostty.org) as bora's outer terminal for two concrete reasons, not just taste:

- it speaks the **kitty keyboard protocol**, which is how bora receives modifier chords like `alt+a` or `cmd+shift+]` as distinct key events at all — without it, many of bora's bindings are simply unreachable;
- it's built on the same engine family as bora's own vendored pane renderer, `libghostty-vt` (see `vendor/libghostty-vt/`).

On macOS:

```sh
brew install --cask ghostty
```

(cask name verified with `brew info --cask ghostty`). For other platforms, see [ghostty.org](https://ghostty.org) for install instructions — not independently verified here.

## 4. Wire Ghostty so it stops eating bora's keys

Copy the example config into place (`-i` prompts before overwriting a file that's already there):

```sh
cp -i examples/ghostty/config ~/.config/ghostty/config
```

This is the load-bearing step. Three things in that file matter, in order:

**`macos-option-as-alt = true`** — without this, macOS composes `alt+a` into the literal character `å` instead of delivering an `alt+a` key event, so every `alt+…` bora binding (agent navigation, in Ary's config: `next_agent = alt+a`) silently does nothing. Applies to new windows only — Ghostty needs a full quit and relaunch (not just a config reload) to pick it up.

**The `keybind = cmd+X=unbind` block** — Ghostty binds most `cmd+…` chords to its own tab/window actions (new tab, close window, etc.) by default and consumes them before they ever reach the process running inside it. Each `unbind` line frees one chord so the raw key event reaches bora instead, which then binds it directly in `bora`'s own config (see step 5). Unbinding is per-chord: adding a new `cmd+…` binding to your `bora` config does nothing until the matching `keybind = cmd+X=unbind` line exists here too.

**The AppKit menu layer** — some chords (`cmd+shift+]`, `cmd+shift+[`, and reload-config's `cmd+shift+,`) are additionally owned by Ghostty's own **application menu** (Window → "Show Next Tab", etc.), which macOS resolves *before* Ghostty's own keybind table is ever consulted. `keybind = ...=unbind` cannot reach these — the fix is to remap the menu item itself via macOS's `NSUserKeyEquivalents` preference for the Ghostty bundle:

```sh
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Show Next Tab" "@^]"
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Show Previous Tab" "@^["
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Reload Configuration" "@^\$,"
```

> **Not independently verified.** These `-dict-add` invocations were written from the documented `NSUserKeyEquivalents` mechanism, not run and confirmed here — this repo does not execute `defaults write` against a real user domain to test it. The declarative, verified equivalent is a nix-darwin `system.defaults.CustomUserPreferences` block:
>
> ```nix
> system.defaults.CustomUserPreferences."com.mitchellh.ghostty" = {
>   NSUserKeyEquivalents = {
>     "Show Next Tab" = "@^]";
>     "Show Previous Tab" = "@^[";
>     "Reload Configuration" = "@^\$,";
>   };
> };
> ```
>
> Glyphs: `@` = cmd, `^` = ctrl, `~` = alt/option, `$` = shift. Menu item titles must match **exactly** — a wrong title is silently ignored (no error, override just never fires). Confirm the real titles for your Ghostty version with:
>
> ```sh
> osascript -e 'tell application "System Events" to tell process "Ghostty" to get name of every menu item of menu 1 of menu bar item "Window" of menu bar 1'
> ```
>
> This remap only takes effect for a freshly launched menu — quit Ghostty fully (`Cmd+Q`) and relaunch; a config reload (`Cmd+Shift+,`) is not enough.

**Troubleshooting "my unbind isn't working":** there are two separate layers, and which one owns a chord is not obvious from the symptom. If the chord still does something Ghostty-shaped (switches tabs, opens a new window), check the **menu** layer first (`NSUserKeyEquivalents` above) — `keybind = ...=unbind` only ever touches Ghostty's own keybind table, not the AppKit menu. If the chord does nothing at all (window just flashes), it's the keybind table — confirm the `unbind` line is actually present and that you didn't relaunch since editing it.

The example config also sets `font-family = JetBrainsMono Nerd Font Mono`. That's a personal choice requiring the [Nerd Font](https://www.nerdfonts.com) variant installed — if you don't have it, delete that line (Ghostty falls back to its default) or point it at any font you have.

## 5. bora's own config

Copy the example into place (`-i` again, so an existing config isn't silently clobbered):

```sh
cp -i examples/bora/config.toml ~/.config/bora/config.toml
```

Notable choices in that file:

- **Prefix:** the shipped default is `ctrl+b` (`[keys] prefix`); the example sets `ctrl+space`. Pick whichever doesn't collide with your shell or other tools.
- **Triple-bound actions:** most actions are bound to three chords at once — `prefix+X`, `cmd+X`, and `ctrl+alt+X`. That's deliberate, not redundancy for its own sake: which chord actually reaches bora depends on the terminal and how much of the menu-vs-keybind unbinding from step 4 you've done. `ctrl+alt+X` is the safe fallback that transmits even without a modern keyboard protocol.
- **`[[keys.command]]` entries** bind chords to plugin actions (`type = "plugin_action"`) or shell commands (`type = "shell"`). Several reference plugins (e.g. a file-viewer, a review pane) or `examples/bora/helix-tab.sh` (a script that opens or focuses a "helix" tab and launches `hx .` in it — requires `bora` on your PATH and `jq` installed). What happens when the target is missing depends on the binding type: `type = "shell"` bindings (`cmd+shift+e`, `prefix+a` in the example) fail **silently** — pressing the chord does nothing, no error. `type = "plugin_action"` bindings (`prefix+f`, `cmd+shift+r`, `prefix+shift+b`) surface a visible **"custom command failed"** toast naming the missing plugin — `src/app/input/navigate.rs`'s `launch_custom_command` catches the `plugin_action_not_found` error from `find_plugin_action` and raises the toast. Delete the entries you don't want, or install what they point to.

## 6. OMP (oh-my-pi) configuration

OMP is the coding-agent harness Ary runs inside bora panes. Its config lives under `~/.omp/agent/`:

```sh
mkdir -p ~/.omp/agent/rules
cp -i examples/omp/config.yml ~/.omp/agent/config.yml
cp -i examples/omp/mcp.json ~/.omp/agent/mcp.json
cp -i examples/omp/rules/*.md ~/.omp/agent/rules/
```

- **`examples/omp/config.yml`** is a **trimmed** version of a real config. The full version carries a personal, multi-hundred-line `modelRoles` ladder mapping every model tier across several paid subscriptions and fallback chains — useful for one person's specific quota juggling, not for a newcomer. The example keeps the generally useful top-level settings and a minimal `modelRoles` so the file is valid and legible.
- **`examples/omp/mcp.json`** demonstrates the no-literal-secrets pattern: an MCP server's `Authorization` header value can start with `!` followed by a shell command, which OMP runs at request time to produce the header instead of reading a static token out of the file. That command might call a 1Password wrapper (`op://<vault>/<item>/<field>` style references), a local script, or `gh auth token` — the point is the secret never sits in the config file itself. It also sets `disabledServers`, a list of bundled MCP server names to turn off without removing them from `mcpServers` — JSON has no comment syntax, so the example keeps just one illustrative entry (`cmux`); delete it or list your own server names to disable.
- **`examples/omp/rules/*.md`** are global rules injected into every OMP session (this exact prompt's own system context loads three of these same files, referenced as `rule://no-tmp-writes`, `rule://worker-safety`, `rule://omp-token-economy`). Format is YAML frontmatter followed by a markdown body. Two frontmatter shapes are used here: most rules carry a one-line `description` (OMP matches this against the current task to decide when to surface the rule), while `next-action.md` carries `alwaysApply: true` instead — no matching needed, it's injected into every response unconditionally (see this doc's own "NEXT ACTION" protocol, which comes from exactly that file):

  ```markdown
  ---
  description: "One line: what this rule covers and when to read it."
  ---

  # Rule title

  Body in plain markdown.
  ```

  They live at `~/.omp/agent/rules/<name>.md`.

Secrets are **never** stored in any of these files. They're fetched at runtime — via a 1Password CLI wrapper, `gh auth token`, or similar — and any machine-local values (hostnames, ports, local paths) belong in a file kept outside version control, not in the committed config.

## 7. How Ary keeps this reproducible

Ary's real `~/.config/ghostty/config`, `~/.config/bora/config.toml`, and `~/.omp/agent/*` are symlinks into a nix-darwin + home-manager flake (a separate, private dotfiles repo), so editing the live file edits the repo directly — no copy-and-forget drift. You don't need Nix, or that repo, to use this example setup: plain `cp` (as shown above) gets you the same files. Nix just makes Ary's own copy self-reinstalling on a fresh machine; it's an implementation detail of his workstation, not a requirement of bora or OMP.

## 8. Verify the whole thing

```sh
bora --version                 # confirm the binary and its fork identity
bora                            # start bora
```

Once it's running:

- press your prefix (e.g. `ctrl+space` then a bound key) — confirms the prefix chord is being received at all;
- press `prefix+s` to open Settings — the header shows the same fork version string from `bora --version`, right-aligned next to the title;
- press an `alt+…` binding (e.g. `alt+a` for next-agent, if bound) — if it works, `macos-option-as-alt` is active and macOS isn't swallowing it into a composed character;
- press a `cmd+…` binding (e.g. `cmd+t` for new tab) — if it works, Ghostty's `unbind` block is freeing that chord instead of consuming it into a native Ghostty tab.

If the last two don't fire, go back to step 4 — that's a Ghostty config issue, not a bora one.

---

**Unverified claims in this document:** the `defaults write ... -dict-add` commands in step 4 were derived from the documented `NSUserKeyEquivalents` preference key but not executed against a real domain to confirm they work non-destructively; the declarative nix-darwin block next to them is the verified path. Everything else — the installer behavior, the version string format, the Ghostty config semantics, the `bora` config bindings, and the OMP rule-file format — was checked directly against this repo's source or the referenced config files before being written here.
