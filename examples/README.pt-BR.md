# Configuração de exemplo do bora

<p align="center">
  <a href="README.md">English</a> · Português (BR)
</p>

## 1. O que você tem

bora é um multiplexador de agentes de terminal — um fork do [herdr](https://github.com/herdrdev/herdr) que acompanha o upstream de perto e adiciona camadas próprias em cima: canais agente-a-agente com uma view de chat nativa (`bora channel`), agrupamento de repos na sidebar, uma faixa lançadora "Programs", escopo/ordenação configuráveis do painel de agentes, correções de renderização/full-repaint, e um servidor MCP (`bora mcp serve`) para expor o próprio bora a um harness cliente de MCP. Ary roda agentes de código [OMP (oh-my-pi)](https://github.com/can1357/oh-my-pi) dentro dele no dia a dia, o que é em torno do que essa configuração de exemplo foi construída — mas o bora detecta e dirige a maioria dos agentes de código de terminal (Claude Code, Codex, OpenCode, e mais; veja `src/detect/manifests/`), não só OMP. Este diretório é um exemplo funcional da configuração de terminal externo que o bora é feito para rodar dentro: [Ghostty](https://ghostty.org) como o terminal, configurado para que seus keybinds e menu não engulam os chords do bora, mais configs de exemplo do `bora` e do OMP que você pode copiar como ponto de partida.

Você pode parar depois do passo 2 com uma instalação funcional do `bora`. Os passos 3–6 constroem a configuração recomendada de Ghostty + bora + OMP em cima disso.

## 2. Instale o bora

```sh
curl -fsSL https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/install.sh | sh
```

Esse é o instalador próprio deste fork (`website/install.sh`), não `herdr.dev/install.sh` — esse aí instala o herdr upstream, um projeto diferente. O script:

- baixa o binário de release correspondente à sua plataforma a partir dos GitHub releases deste fork,
- instala como `bora` em `${HERDR_INSTALL_DIR:-$HOME/.local/bin}` — defina `HERDR_INSTALL_DIR` para instalar em outro lugar.

Confirme que esse diretório está no seu `PATH`, depois confirme:

```sh
bora --version
```

Isso imprime algo como:

```
bora 0.24.0 (v0.8.1[a5c69bea].bora-24)
```

A primeira parte (`0.24.0`) é a versão de release do próprio bora. A parte entre parênteses é a identidade do fork: `v0.8.1` é o release do herdr upstream até o qual a branch `master` deste fork está mergeada, `[a5c69bea]` é o commit upstream que esse merge trouxe, e `.bora-24` é o número de build deste fork em cima dessa base.

Binários pré-compilados atualmente cobrem Linux e macOS (x86_64 e aarch64). O suporte a Windows está a caminho mas ainda não está em todo release — confira os [GitHub releases](https://github.com/aryrabelo/bora-herdr-ada/releases) para sua plataforma.

Para compilar a partir do código-fonte (ex.: para acompanhar `main` no dia a dia), veja a [seção de instalação do README raiz](../README.md#install) para todos os detalhes. Resumindo: `git clone` o repo, rode `just fetch-libghostty-vt` para baixar a lib estática pré-compilada do `libghostty-vt` (precisa de um toolchain Rust, `just`, e `python3` no seu `PATH`), depois `cargo build --release` e faça um symlink de `target/release/bora` no seu `PATH`. Essa configuração de exemplo não depende de qual método de instalação você usou.

## 3. Instale o Ghostty, e por quê

Recomendamos o [Ghostty](https://ghostty.org) como o terminal externo do bora por duas razões concretas, não só gosto:

- ele fala o **protocolo de teclado do kitty**, que é como o bora recebe chords com modificador como `alt+a` ou `cmd+shift+]` como eventos de tecla distintos — sem isso, muitos dos bindings do bora simplesmente ficam inalcançáveis;
- ele é construído sobre a mesma família de engine do renderer de pane vendorizado do próprio bora, `libghostty-vt` (veja `vendor/libghostty-vt/`).

No macOS:

```sh
brew install --cask ghostty
```

(nome do cask verificado com `brew info --cask ghostty`). Para outras plataformas, veja [ghostty.org](https://ghostty.org) para instruções de instalação — não verificado independentemente aqui.

## 4. Configure o Ghostty para parar de engolir as teclas do bora

Copie a config de exemplo para o lugar (`-i` pergunta antes de sobrescrever um arquivo que já existe):

```sh
cp -i examples/ghostty/config ~/.config/ghostty/config
```

Esse é o passo load-bearing. Três coisas nesse arquivo importam, nessa ordem:

**`macos-option-as-alt = true`** — sem isso, o macOS compõe `alt+a` no caractere literal `å` em vez de entregar um evento de tecla `alt+a`, então todo binding `alt+…` do bora (navegação entre agentes, na config do Ary: `next_agent = alt+a`) simplesmente não faz nada, silenciosamente. Aplica-se só a janelas novas — o Ghostty precisa de um quit completo e relançamento (não só um reload de config) para pegar essa mudança.

**O bloco `keybind = cmd+X=unbind`** — o Ghostty vincula a maioria dos chords `cmd+…` às suas próprias ações de tab/janela (nova aba, fechar janela, etc.) por padrão, e os consome antes que cheguem ao processo rodando dentro dele. Cada linha `unbind` libera um chord para que o evento de tecla bruto chegue ao bora em vez disso, que então o vincula diretamente na própria config do `bora` (veja o passo 5). O unbind é por chord: adicionar um novo binding `cmd+…` na sua config do `bora` não faz nada até que a linha `keybind = cmd+X=unbind` correspondente também exista aqui.

**A camada de menu do AppKit** — alguns chords (`cmd+shift+]`, `cmd+shift+[`, e o `cmd+shift+,` do reload de config) são adicionalmente donos do próprio **menu de aplicativo** do Ghostty (Window → "Show Next Tab", etc.), que o macOS resolve *antes* que a própria tabela de keybinds do Ghostty seja sequer consultada. `keybind = ...=unbind` não alcança isso — a correção é remapear o próprio item de menu via a preferência `NSUserKeyEquivalents` do macOS para o bundle do Ghostty:

```sh
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Show Next Tab" "@^]"
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Show Previous Tab" "@^["
defaults write com.mitchellh.ghostty NSUserKeyEquivalents -dict-add "Reload Configuration" "@^\$,"
```

> **Não verificado independentemente.** Essas invocações de `-dict-add` foram escritas a partir do mecanismo documentado de `NSUserKeyEquivalents`, mas não foram rodadas e confirmadas aqui — este repo não executa `defaults write` contra um domínio de usuário real para testar. O equivalente declarativo e verificado é um bloco `system.defaults.CustomUserPreferences` do nix-darwin:
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
> Glifos: `@` = cmd, `^` = ctrl, `~` = alt/option, `$` = shift. Os títulos dos itens de menu precisam bater **exatamente** — um título errado é ignorado silenciosamente (sem erro, o override simplesmente nunca dispara). Confirme os títulos reais para sua versão do Ghostty com:
>
> ```sh
> osascript -e 'tell application "System Events" to tell process "Ghostty" to get name of every menu item of menu 1 of menu bar item "Window" of menu bar 1'
> ```
>
> Esse remap só tem efeito para um menu recém-lançado — feche o Ghostty completamente (`Cmd+Q`) e relance; um reload de config (`Cmd+Shift+,`) não basta.

**Troubleshooting "meu unbind não está funcionando":** existem duas camadas separadas, e qual delas é dona de um chord não é óbvio a partir do sintoma. Se o chord ainda faz algo com cara de Ghostty (troca de aba, abre uma janela nova), confira a camada de **menu** primeiro (`NSUserKeyEquivalents` acima) — `keybind = ...=unbind` só toca a tabela de keybinds do próprio Ghostty, não o menu do AppKit. Se o chord não faz absolutamente nada (a janela só pisca), é a tabela de keybinds — confirme que a linha `unbind` está de fato presente e que você não relançou desde que a editou.

A config de exemplo também define `font-family = JetBrainsMono Nerd Font Mono`. Essa é uma escolha pessoal que exige a variante [Nerd Font](https://www.nerdfonts.com) instalada — se você não a tem, apague essa linha (o Ghostty cai de volta no padrão dele) ou aponte para qualquer fonte que você tenha.

## 5. A config do próprio bora

Copie o exemplo para o lugar (`-i` de novo, para não sobrescrever silenciosamente uma config existente):

```sh
cp -i examples/bora/config.toml ~/.config/bora/config.toml
```

Escolhas notáveis nesse arquivo:

- **Prefix:** o padrão de fábrica é `ctrl+b` (`[keys] prefix`); o exemplo define `ctrl+space`. Escolha o que não colidir com seu shell ou outras ferramentas.
- **Ações triplamente vinculadas:** a maioria das ações é vinculada a três chords ao mesmo tempo — `prefix+X`, `cmd+X`, e `ctrl+alt+X`. Isso é deliberado, não redundância por redundância: qual chord de fato chega ao bora depende do terminal e de quanto do unbind menu-vs-keybind do passo 4 você já fez. `ctrl+alt+X` é o fallback seguro que transmite mesmo sem um protocolo de teclado moderno.
- **Entradas `[[keys.command]]`** vinculam chords a ações de plugin (`type = "plugin_action"`) ou comandos de shell (`type = "shell"`). Várias referenciam plugins (ex.: um visualizador de arquivos, um pane de review) ou `examples/bora/helix-tab.sh` (um script que abre ou foca uma tab "helix" e lança `hx .` nela — precisa do `bora` no seu PATH e do `jq` instalado). O que acontece quando o alvo está faltando depende do tipo do binding: bindings `type = "shell"` (`cmd+shift+e`, `prefix+a` no exemplo) falham **silenciosamente** — apertar o chord não faz nada, sem erro. Bindings `type = "plugin_action"` (`prefix+f`, `cmd+shift+r`, `prefix+shift+b`) mostram um toast visível de **"custom command failed"** nomeando o plugin faltante — o `launch_custom_command` de `src/app/input/navigate.rs` captura o erro `plugin_action_not_found` de `find_plugin_action` e dispara o toast. Apague as entradas que você não quer, ou instale o que elas apontam.

**Exemplo de plugin empacotado — `examples/bora/plugins/gitui`:** um manifesto de plugin mínimo, já no repo, que abre o [gitui](https://github.com/gitui-org/gitui) na própria tab. Instale com:

```sh
bora plugin link examples/bora/plugins/gitui
```

Precisa do `gitui` no seu `PATH` (ex.: `brew install gitui`). Diferente dos bindings vindos da config acima, os hooks de evento `worktree.created`/`worktree.opened` abrem a tab automaticamente sempre que um worktree é criado ou aberto — sem precisar de nenhuma entrada `[[keys.command]]`, embora as ações `toggle`/`open`/`close` também estejam disponíveis caso queira vincular uma.

## 6. Configuração do OMP (oh-my-pi)

OMP é o harness de agente de código que o Ary roda dentro dos panes do bora. A config dele fica em `~/.omp/agent/`:

```sh
mkdir -p ~/.omp/agent/rules
cp -i examples/omp/config.yml ~/.omp/agent/config.yml
cp -i examples/omp/mcp.json ~/.omp/agent/mcp.json
cp -i examples/omp/rules/*.md ~/.omp/agent/rules/
```

- **`examples/omp/config.yml`** é uma versão **reduzida** de uma config real. A versão completa carrega uma escada `modelRoles` pessoal de várias centenas de linhas mapeando cada tier de modelo em várias assinaturas pagas e cadeias de fallback — útil para o quota juggling específico de uma pessoa, não para quem está começando. O exemplo mantém as configs de nível superior geralmente úteis e um `modelRoles` mínimo para que o arquivo seja válido e legível.
- **`examples/omp/mcp.json`** demonstra o padrão de nenhum segredo literal: o valor do header `Authorization` de um servidor MCP pode começar com `!` seguido de um comando de shell, que o OMP roda no momento da requisição para produzir o header em vez de ler um token estático do arquivo. Esse comando pode chamar um wrapper do 1Password (referências no estilo `op://<vault>/<item>/<field>`), um script local, ou `gh auth token` — o ponto é que o segredo nunca fica no próprio arquivo de config. Ele também define `disabledServers`, uma lista de nomes de servidores MCP empacotados para desligar sem removê-los de `mcpServers` — JSON não tem sintaxe de comentário, então o exemplo mantém só uma entrada ilustrativa (`cmux`); apague-a ou liste os nomes dos seus próprios servidores para desligar.
- **`examples/omp/rules/*.md`** são regras globais injetadas em toda sessão do OMP (o contexto de sistema deste próprio prompt carrega três desses mesmos arquivos, referenciados como `rule://no-tmp-writes`, `rule://worker-safety`, `rule://omp-token-economy`). O formato é frontmatter YAML seguido de um corpo em markdown. Duas formas de frontmatter são usadas aqui: a maioria das regras carrega uma `description` de uma linha (o OMP compara isso com a tarefa atual para decidir quando mostrar a regra), enquanto `next-action.md` carrega `alwaysApply: true` em vez disso — não precisa de match nenhum, é injetada em toda resposta incondicionalmente (veja o próprio protocolo "NEXT ACTION" deste doc, que vem exatamente desse arquivo):

  ```markdown
  ---
  description: "One line: what this rule covers and when to read it."
  ---

  # Rule title

  Body in plain markdown.
  ```

  Eles ficam em `~/.omp/agent/rules/<name>.md`.

Segredos **nunca** são armazenados em nenhum desses arquivos. Eles são buscados em tempo de execução — via um wrapper de CLI do 1Password, `gh auth token`, ou similar — e quaisquer valores locais à máquina (hostnames, portas, caminhos locais) pertencem a um arquivo mantido fora do controle de versão, não na config commitada.

## 7. Como o Ary mantém isso reproduzível

A `~/.config/ghostty/config`, `~/.config/bora/config.toml`, e `~/.omp/agent/*` reais do Ary são symlinks para dentro de um flake nix-darwin + home-manager (um repo de dotfiles separado e privado), então editar o arquivo ao vivo edita o repo diretamente — sem drift de copiar-e-esquecer. Você não precisa do Nix, nem daquele repo, para usar essa configuração de exemplo: um `cp` simples (como mostrado acima) te dá os mesmos arquivos. O Nix só faz a própria cópia do Ary se reinstalar sozinha numa máquina nova; é um detalhe de implementação da estação de trabalho dele, não um requisito do bora ou do OMP.

## 8. Verifique tudo

```sh
bora --version                 # confirm the binary and its fork identity
bora                            # start bora
```

Uma vez rodando:

- aperte seu prefix (ex.: `ctrl+space` e depois uma tecla vinculada) — confirma que o chord de prefix está sendo recebido de fato;
- aperte `prefix+s` para abrir Settings — o header mostra a mesma string de versão do fork de `bora --version`, alinhada à direita ao lado do título;
- aperte um binding `alt+…` (ex.: `alt+a` para next-agent, se vinculado) — se funcionar, `macos-option-as-alt` está ativo e o macOS não está engolindo em um caractere composto;
- aperte um binding `cmd+…` (ex.: `cmd+t` para nova aba) — se funcionar, o bloco `unbind` do Ghostty está liberando esse chord em vez de consumi-lo numa aba nativa do Ghostty.

Se os dois últimos não dispararem, volte ao passo 4 — isso é um problema de config do Ghostty, não do bora.

---

**Alegações não verificadas neste documento:** os comandos `defaults write ... -dict-add` no passo 4 foram derivados da preferência documentada `NSUserKeyEquivalents`, mas não executados contra um domínio real para confirmar que funcionam de forma não destrutiva; o bloco declarativo do nix-darwin ao lado deles é o caminho verificado. Todo o resto — o comportamento do instalador, o formato da string de versão, a semântica da config do Ghostty, os bindings da config do `bora`, e o formato dos arquivos de regra do OMP — foi conferido diretamente contra o código-fonte deste repo ou os arquivos de config referenciados antes de ser escrito aqui.
