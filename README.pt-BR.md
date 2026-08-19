# bora

bora é um fork do [herdr](https://github.com/herdrdev/herdr): acompanha o upstream, adiciona funcionalidades exclusivas do fork por cima, e mantém seu próprio canal de release.

<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  <a href="https://herdr.dev">herdr.dev</a> · <a href="#instalação">instalação</a> · <a href="https://herdr.dev/docs/quick-start/">início rápido</a> · <a href="https://herdr.dev/docs/">documentação</a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · Português (BR)
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

**o runtime onde seus agentes de código vivem.**

- **sempre rodando** — bora é um servidor em background; os terminais vivem dentro dele. feche a tampa, perca a rede ou reinicie a máquina; os agentes continuam trabalhando e as sessões voltam. reconecte de qualquer terminal, ou via ssh.
- **nunca precise caçar o que travou** — cada pane é marcado como working, blocked ou idle. quando um agente para e precisa de uma resposta, bora avisa.
- **agent-native** — agentes controlam o bora pela cli e pela socket api: podem criar panes, se promptar entre si e esperar até que outro agente esteja de fato bloqueado. [agent skill →](https://herdr.dev/docs/agent-skill/)
- **roda o que você já usa** — claude code, codex, cursor, opencode, grok e o resto. bora não os envolve nem os substitui; ele é o dono dos terminais deles.
- **teclado e mouse, os dois de primeira classe** — prefix keys no estilo tmux *e* clique, arraste, split. escolha por momento, não por ferramenta.
- **plugins** — estenda panes e workflows. [veja o marketplace →](https://herdr.dev/plugins/)
- **um binário rust só, sem electron** — roda em qualquer terminal que você já usa.

---

## instalação

recomendado, binário pré-compilado — linux (x86_64, aarch64), macos (x86_64, aarch64) e windows (x86_64):

```bash
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/install.sh | sh
```

instala em `~/.local/bin` (substitua com `HERDR_INSTALL_DIR`); precisa de `curl` e `awk`.

a partir do código-fonte — precisa de um toolchain Rust, [`just`](https://just.systems/) e `python3`:

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # biblioteca estática libghostty-vt pré-compilada — evita o build do zig 0.15.2
cargo build --release
ln -sf "$(pwd)/target/release/bora" ~/.local/bin/bora
ln -sf ~/.local/bin/bora ~/.local/bin/herdr   # opcional: mantém o nome de comando `herdr`
```

depois inicie ele onde o trabalho está:

```bash
bora
```

rode seus agentes, divida panes, e vá embora. `ctrl+b q` desanexa, `bora` reconecta. [início rápido →](https://herdr.dev/docs/quick-start/)

## configuração recomendada

rodamos o bora dentro do [Ghostty](https://ghostty.org/). por padrão, o Ghostty engole vários dos keybinds do bora (Option como Alt, chords de tab/window) a menos que seja configurado — veja [`examples/README.pt-BR.md`](examples/README.pt-BR.md) para a configuração completa de Ghostty + bora + omp.

## documentação

bora compartilha o núcleo com o herdr upstream, então a documentação vive no site do herdr e descreve o comportamento comum aos dois: [herdr.dev/docs](https://herdr.dev/docs/): [início rápido](https://herdr.dev/docs/quick-start/) · [conceitos](https://herdr.dev/docs/concepts/) · [agentes suportados](https://herdr.dev/docs/agents/) · [teclado](https://herdr.dev/docs/keyboard/) · [configuração](https://herdr.dev/docs/configuration/) · [estado da sessão](https://herdr.dev/docs/session-state/) · [remoto](https://herdr.dev/docs/persistence-remote/) · [integrações](https://herdr.dev/docs/integrations/) · [plugins](https://herdr.dev/docs/plugins/) · [socket api](https://herdr.dev/docs/socket-api/)

## agradecimentos

cada sponsor e backer anterior está listado em [SPONSORS.md](./SPONSORS.md) — obrigado 🐑

enterprise / parcerias: hey@herdr.dev (contato do herdr upstream, não deste fork)

## instruções para agentes

se você é um agente de ia ajudando neste repositório, leia [`AGENTS.md`](./AGENTS.md) antes de fazer mudanças e leia [`CONTRIBUTING.md`](./CONTRIBUTING.md) antes de abrir issues ou PRs.

## desenvolvimento

precisa de um toolchain Rust, [`just`](https://just.systems/), `python3`, e [`cargo-nextest`](https://nexte.st/) (`cargo install cargo-nextest --locked`):

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # prebuilt libghostty-vt static lib — skips the zig 0.15.2 build
cargo build --release

just test        # unit tests
just check       # formatting, tests, and maintenance checks
```

leia [`BORA.md`](./BORA.md) e [`AGENTS.md`](./AGENTS.md) antes de contribuir.

## licença

bora é licenciado sob a [Apache License 2.0](LICENSE), a mesma licença do herdr upstream.