# ary.auto-group

Puts each workspace into a sidebar group according to its git repository, using
a small rules file. Linked worktrees follow their **main** repo, so a worktree
of `postpilot-web` lands in the same group as `postpilot-web` itself.

It **never overrides a group you chose**: a workspace that already has a group
is left untouched. The only exception is `organize --force`, which you run by
hand.

Needs `python3` (3.9+, stdlib only) and `git`. Verified on bora 0.50.2.

## Install

```sh
bora plugin link examples/bora/plugins/auto-group
mkdir -p "$(bora plugin config-dir ary.auto-group)"
cp examples/bora/plugins/auto-group/rules.example.conf \
   "$(bora plugin config-dir ary.auto-group)/rules.conf"
```

Edit `rules.conf`, then check it:

```sh
python3 examples/bora/plugins/auto-group/autogroup.py check
```

(`check` needs `HERDR_BIN_PATH` or `HERDR_PLUGIN_CONFIG_DIR` in the
environment; both are set inside bora panes.) With no `rules.conf`, or no
matching rule, the plugin does nothing.

## Rules

One rule per line, `<pattern> = <group>`. Text after the first `=` is the group
path (spaces allowed, `/` nests). Blank lines and `#` lines are ignored; a
malformed line is skipped and reported. Matching is case-insensitive and the
**first** matching rule wins.

- No `/` in the pattern and not starting with `~`: `fnmatch` glob against the repo
  **name** (folder name of the main repo).
- Otherwise: `fnmatch` glob against the repo **root path** (`~` expanded,
  trailing `/` ignored; `*` also matches `/`).

```conf
# name glob
postpilot*              = pp
# path glob
~/Sites/bora-team/*     = bora team
# nested group: folder "docs" inside "pp"
~/Sites/postpilot/docs* = pp/docs
```

Comments must be on their own line; text after the group is part of the group.
Put specific rules above general ones.

## When it runs

- Automatically on `workspace.created`, `worktree.created` and `worktree.opened`,
  for that one workspace (if it has no group yet).
- Action `ary.auto-group.organize`: applies the rules to every ungrouped workspace.
- From a shell:

```sh
python3 autogroup.py organize --dry-run   # print the plan, change nothing
python3 autogroup.py organize             # same as the action
python3 autogroup.py organize --force     # ALSO regroup workspaces that already have a group
```

`--force` exists only on the command line; no action or hook uses it. Running
`organize` twice is a no-op the second time. Hook and action output is in
`bora plugin log list --plugin ary.auto-group`.

## Optional key binding

Add to your `config.toml` (this plugin does not edit it):

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "ary.auto-group.organize"
description = "auto-group workspaces"
```
