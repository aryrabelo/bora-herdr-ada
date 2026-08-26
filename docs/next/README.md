# bora

a fork of [herdr](https://github.com/herdrdev/herdr): tracks upstream, layers fork-specific features on top, and ships its own release channel.

<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  <a href="https://herdr.dev">herdr.dev</a> · <a href="#install">install</a> · <a href="https://herdr.dev/docs/quick-start/">quick start</a> · <a href="https://herdr.dev/docs/">docs</a>
</p>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a> · <a href="README.pt-BR.md">Português (BR)</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/aryrabelo/bora-herdr-ada/releases"><img src="https://img.shields.io/github/downloads/aryrabelo/bora-herdr-ada/total?labelColor=333333&color=666666" alt="total GitHub release downloads" /></a>
  <a href="https://github.com/herdrdev/herdr/stargazers"><img src="https://img.shields.io/github/stars/herdrdev/herdr?labelColor=333333&color=666666&logo=github" alt="GitHub stars" /></a>
  <a href="https://github.com/aryrabelo/bora-herdr-ada/releases/latest"><img src="https://img.shields.io/github/v/release/aryrabelo/bora-herdr-ada?label=release&labelColor=333333&color=666666" alt="latest stable release" /></a>
  <a href="https://x.com/herdrdev"><img src="https://img.shields.io/badge/follow-%40herdrdev-000000?logo=x&logoColor=white" alt="follow @herdrdev on X" /></a>
</p>

---

https://github.com/user-attachments/assets/043ec09f-4bdd-41d5-aee0-8fda6b83e267

**the runtime your coding agents live on.**

- **always running** — bora is a background server; the terminals live inside it. close the lid, drop the network, or restart the machine; agents keep working and sessions come back. reattach from any terminal, or over ssh.
- **never hunt for the stuck one** — every pane is marked working, blocked, or idle. when an agent stops and needs an answer, bora says so.
- **agent-native** — agents drive bora through the cli and socket api: they can spawn panes, prompt each other, and wait until another agent is genuinely blocked. [agent skill →](https://herdr.dev/docs/agent-skill/)
- **runs what you already run** — claude code, codex, cursor, opencode, grok and the rest. bora doesn't wrap or replace them; it owns their terminals.
- **keyboard and mouse, both first-class** — tmux-style prefix keys *and* click, drag, split. pick per moment, not per tool.
- **plugins** — extend panes and workflows. [browse the marketplace →](https://herdr.dev/plugins/)
- **one rust binary, no electron** — runs in whatever terminal you already use.

---

## install

recommended, prebuilt binary — linux (x86_64, aarch64), macos (x86_64, aarch64), and windows (x86_64):

```bash
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/install.sh | sh
```

installs to `~/.local/bin` (override with `HERDR_INSTALL_DIR`); needs `curl` and `awk`.

from source — needs a Rust toolchain, [`just`](https://just.systems/), and `python3`:

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # prebuilt libghostty-vt static lib — skips the zig 0.15.2 build
cargo build --release
ln -sf "$(pwd)/target/release/bora" ~/.local/bin/bora
ln -sf ~/.local/bin/bora ~/.local/bin/herdr   # optional: keep the `herdr` command name
```

then start it where the work lives:

```bash
bora
```

run your agents, split panes, walk away. `ctrl+b q` detaches, `bora` reattaches. [quick start →](https://herdr.dev/docs/quick-start/)

## recommended setup

we run bora inside [Ghostty](https://ghostty.org/). Ghostty eats several of bora's keybindings by default (Option-as-Alt, tab/window chords) unless configured — see [`examples/README.md`](examples/README.md) for the full worked Ghostty + bora + omp setup.

## agent dispatch

two fork-only commands turn "run an agent on this" into one shot:

```bash
bora agent --new "review the plan and list the risks"
# creates a workspace on the current directory, starts the default agent
# on its root pane, injects the prompt, and prints one json result.

bora agent my-key prompt "first task"   # creates agent "my-key"
bora agent my-key prompt "second task"  # same agent, new prompt
```

`--new` always creates; `agent <name> prompt` is get-or-create — the name is the idempotency key, so re-running the same dispatch prompts the existing agent instead of duplicating it. the kind resolves `--kind` over `[agents] default` in config.toml over a hardcoded `omp` fallback. see `bora agent help` for the full surface.

## docs

bora shares its core with upstream herdr, so the docs live on herdr's site and describe behavior common to both: [herdr.dev/docs](https://herdr.dev/docs/): [quick start](https://herdr.dev/docs/quick-start/) · [concepts](https://herdr.dev/docs/concepts/) · [supported agents](https://herdr.dev/docs/agents/) · [keyboard](https://herdr.dev/docs/keyboard/) · [configuration](https://herdr.dev/docs/configuration/) · [session state](https://herdr.dev/docs/session-state/) · [remote](https://herdr.dev/docs/persistence-remote/) · [integrations](https://herdr.dev/docs/integrations/) · [plugins](https://herdr.dev/docs/plugins/) · [socket api](https://herdr.dev/docs/socket-api/)

## thanks

every past sponsor and backer is listed in [SPONSORS.md](./SPONSORS.md) — thank you 🐑

enterprise / partnership: hey@herdr.dev (upstream herdr's contact, not this fork)

## agent instructions

if you are an ai agent helping with this repository, read [`AGENTS.md`](./AGENTS.md) before making changes and read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before opening issues or PRs.

## development

needs a Rust toolchain, [`just`](https://just.systems/), `python3`, and [`cargo-nextest`](https://nexte.st/) (`cargo install cargo-nextest --locked`):

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # prebuilt libghostty-vt static lib — skips the zig 0.15.2 build
cargo build --release

just test        # unit tests
just check       # formatting, tests, and maintenance checks
```

read [`BORA.md`](./BORA.md) and [`AGENTS.md`](./AGENTS.md) before contributing.

## license

bora is licensed under the [Apache License 2.0](LICENSE), the same license as upstream herdr.
