# bora

bora 是 [herdr](https://github.com/herdrdev/herdr) 的一个分支（fork）：跟随上游更新，叠加分支专属的功能，并使用自己的发布渠道。

<p align="center">
  <img src="assets/logo.png" alt="herdr" width="100" />
</p>

<p align="center">
  <a href="https://herdr.dev">herdr.dev</a> · <a href="#安装">安装</a> · <a href="https://herdr.dev/zh-cn/docs/quick-start/">快速开始</a> · <a href="https://herdr.dev/zh-cn/docs/">文档</a></p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
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

**智能体复用器，住在你的终端里。**

- **每个智能体一目了然**——`blocked`、`working`、`done`。真实的终端视图，而不是包装过的转述。
- **分离后智能体继续运行**——从任意终端重新连接，或通过 ssh。会话在重启后依然保留。
- **智能体也能使用 bora**——纯 socket api：智能体可以创建窗格、读取输出、互相等待。[智能体技能 →](https://herdr.dev/zh-cn/docs/agent-skill/)
- **键盘和鼠标都是一等公民**——tmux 风格的前缀键，*以及*点击、拖动、分割。按当下的场景选择，而不是被工具锁死。
- **插件**——扩展窗格和工作流。[浏览插件市场 →](https://herdr.dev/plugins/)
- **单个 rust 二进制，没有 electron**——运行在你已经在用的任何终端里。

---

## 安装

推荐使用预编译二进制——linux（x86_64、aarch64）和 macos（x86_64、aarch64）；windows 将在下一个版本中提供：

```bash
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/install.sh | sh
```

安装到 `~/.local/bin`（可通过 `HERDR_INSTALL_DIR` 覆盖）；需要 `curl` 和 `awk`。

从源码构建（需要 Rust 工具链、[`just`](https://just.systems/) 和 `python3`）：

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # prebuilt libghostty-vt static lib — skips the zig 0.15.2 build
cargo build --release
ln -sf "$(pwd)/target/release/bora" ~/.local/bin/bora
ln -sf ~/.local/bin/bora ~/.local/bin/herdr   # optional: keep the `herdr` command name
```

然后在工作所在的目录启动它：

```bash
bora
```

运行你的智能体、分割窗格，然后安心离开。`ctrl+b q` 分离，`bora` 重新连接。[快速开始 →](https://herdr.dev/zh-cn/docs/quick-start/)

## 推荐配置

我们在 [Ghostty](https://ghostty.org/) 里运行 bora。Ghostty 默认会吞掉 bora 的部分按键绑定（Option 作为 Alt、标签/窗口快捷键），需要额外配置——完整的 Ghostty + bora + omp 配置示例见 [`examples/README.md`](examples/README.md)。

## 文档

bora 与上游 herdr 共享核心，文档都发布在 herdr 的网站上，描述的是两者共有的行为：[herdr.dev/docs](https://herdr.dev/zh-cn/docs/)：[快速开始](https://herdr.dev/zh-cn/docs/quick-start/) · [核心概念](https://herdr.dev/zh-cn/docs/concepts/) · [受支持的智能体](https://herdr.dev/zh-cn/docs/agents/) · [键盘](https://herdr.dev/zh-cn/docs/keyboard/) · [配置](https://herdr.dev/zh-cn/docs/configuration/) · [会话状态](https://herdr.dev/zh-cn/docs/session-state/) · [远程访问](https://herdr.dev/zh-cn/docs/persistence-remote/) · [集成](https://herdr.dev/zh-cn/docs/integrations/) · [插件](https://herdr.dev/zh-cn/docs/plugins/) · [socket api](https://herdr.dev/zh-cn/docs/socket-api/)

## 致谢

<a href="https://terminaltrove.com/"><img src="assets/sponsors/terminal-trove.png" alt="Terminal Trove" width="200" /></a>

[Terminal Trove](https://terminaltrove.com/) 以及 [SPONSORS.md](./SPONSORS.md) 中列出的每一位支持者——谢谢 🐑

企业/合作：hey@herdr.dev（这是上游 herdr 的联系方式，不代表本分支）

## 智能体须知

如果你是协助本仓库的 AI 智能体：在改动代码前阅读 [`AGENTS.md`](./AGENTS.md)，在创建 issue 或 PR 前阅读 [`CONTRIBUTING.md`](./CONTRIBUTING.md)。

## 开发

需要 Rust 工具链、`just`、`python3`，以及 [`cargo-nextest`](https://nexte.st/)（`cargo install cargo-nextest --locked`）：

```bash
git clone https://github.com/aryrabelo/bora-herdr-ada
cd bora-herdr-ada
just fetch-libghostty-vt   # prebuilt libghostty-vt static lib — skips the zig 0.15.2 build
cargo build --release

just test        # 单元测试
just check       # 格式检查、测试和维护性检查
```

阅读 [`BORA.md`](./BORA.md) 和 [`AGENTS.md`](./AGENTS.md) 后再贡献代码。

## 许可证

bora 基于 [Apache License 2.0](LICENSE) 许可证发布，与上游 herdr 一致。
