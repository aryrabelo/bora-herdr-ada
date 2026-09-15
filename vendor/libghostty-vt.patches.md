# libghostty-vt local patches

This file tracks intentional local changes applied on top of the vendored
`libghostty-vt` source. Remove a patch only when the vendored source commit
contains the upstream behavior and the listed verification still passes.

## 0001 default lib-vt panes to grapheme clustering

status: active

patch: `vendor/patches/libghostty-vt/0001-default-grapheme-cluster-mode.patch`

herdr issue: https://github.com/herdrdev/herdr/issues/243

upstream discussion: not opened; libghostty-vt currently exposes current mode mutation but no C API for configuring terminal default modes

upstream pr: not opened

vendored base: `c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3`

local files:

- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: Herdr renders terminal cells directly and requires DEC private mode
2027 to store flags, ZWJ emoji, and other multi-codepoint grapheme clusters in
one cell. This patch makes clustering active for new terminals and keeps it as
the reset default so RIS (`ESC c`) does not disable it.

remove when: libghostty-vt exposes a C API for setting default mode 2027, or
upstream makes grapheme clustering the lib-vt default, and the reset-survival
regression passes without this patch.

verification:

```sh
cargo nextest run --locked grapheme_cluster_mode_is_default_and_survives_full_reset
cargo nextest run --locked grapheme_cluster_mode_renders_flag_emoji_in_single_wide_cell
cargo nextest run --locked grapheme_cluster_mode_renders_zwj_family_in_single_wide_cell
```

## 0002 expose modifyOtherKeys mode through terminal data

status: active

patch: `vendor/patches/libghostty-vt/0002-expose-modify-other-keys-mode.patch`

herdr issue: none; fixes the performance regression exposed by
https://github.com/herdrdev/herdr/pull/2303

upstream discussion: not opened

upstream pr: not opened

vendored base: `c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3`

local files:

- `vendor/libghostty-vt/include/ghostty/vt/terminal.h`
- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: Herdr must know whether xterm modifyOtherKeys mode 2 is active to
request printable key releases from the outer terminal. The formatter API can
recover this fact only by formatting the active screen and scrollback. A typed
terminal-data query exposes the authoritative scalar without formatting or
allocation.

remove when: the vendored source exposes an equivalent scalar query for
modifyOtherKeys mode 2 and Herdr can use it without this patch.

verification:

```sh
cargo nextest run --locked modify_other_keys_query_tracks_mode_two
cargo nextest run --locked host_report_all_supplies_printable_releases_for_event_type_only_panes
python3 -m unittest scripts.test_vendor_libghostty_vt scripts.test_ui_hot_path_architecture
```

## 0003 let hosts opt out of reflow on resize

status: active

patch: `vendor/patches/libghostty-vt/0003-reflow-on-resize-opt-out.patch`

herdr issue: none; fixes the padded-transcript corruption measured on
2026-09-15 (pane 91 -> 53 cols emitted one 38-column remainder row after each
full row)

upstream discussion: not opened

upstream pr: not opened

vendored base: `c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3`

local files:

- `vendor/libghostty-vt/include/ghostty/vt/terminal.h`
- `vendor/libghostty-vt/src/terminal/Terminal.zig`
- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: `Terminal.resize` derived reflow purely from the DECAWM wraparound
mode, so narrowing a pane rewrapped rows the program had already painted.
Output padded to the old width was re-split into a content row plus a
trailing-space remainder row, and Herdr had no way to decline. This patch adds
a per-terminal `reflow_on_resize` host flag (default `true`, so an unconfigured
Herdr behaves exactly as before) that gates the reflow decision and reuses the
existing `PageList.resizeWithoutReflow` path, plus a
`GHOSTTY_TERMINAL_OPT_REFLOW_ON_RESIZE` setter on the C ABI. The flag is host
configuration: `fullReset` (RIS) does not clear it and the running program
cannot change it. It backs the `terminal.reflow_on_resize` config key.

remove when: the vendored source exposes an equivalent host-level
reflow-on-resize control through the C ABI and Herdr can set it without this
patch.

verification:

```sh
cargo nextest run --locked reflow_on_resize_default_rewraps_soft_wrapped_row
cargo nextest run --locked reflow_on_resize_disabled_keeps_rows_unwrapped
cargo nextest run --locked reflow_on_resize_default_on_with_opt_out
```

## 0004 render the OSC 66 text-sizing payload instead of dropping it

status: active

patch: `vendor/patches/libghostty-vt/0004-render-osc-66-text-sizing-payload.patch`

herdr issue: none; measured 2026-09-15 in a bora pane, where
`printf '\033]66;s=2;SCALE-TWO\033\\'` printed an empty line

upstream discussion: https://github.com/ghostty-org/ghostty/issues/10333
(`Implement the Text Sizing Protocol (OSC 66)`, open); the parser landed in
https://github.com/ghostty-org/ghostty/pull/10315 without any dispatch

upstream pr: not opened

vendored base: `c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3`

local files:

- `vendor/libghostty-vt/src/terminal/stream.zig`
- `vendor/libghostty-vt/src/terminal/stream_terminal.zig`

reason: The vendored parser is already upstream-complete — OSC 66 is parsed
at `src/terminal/osc/parsers/kitty_text_sizing.zig` and the payload survives
intact as `Command.kitty_text_sizing.text`, with upstream unit tests to prove
it. Only the dispatch is missing: at the pinned base, `Stream.oscDispatch`
grouped `kitty_text_sizing` with eleven other variants into a single arm whose
whole body was `log.debug("unimplemented OSC callback: {}", .{cmd})`, so the
handler was never called and the text was discarded before it could reach the
grid. The Kitty text-sizing protocol requires an implementation that does not
support the sizing attributes to still render the text normally, so this patch
moves `kitty_text_sizing` out of that dead arm and prints its payload
codepoint by codepoint through the normal print action, ignoring
scale/width/valign/halign.

remove when: the vendored source implements the text-sizing protocol itself
(upstream issue 10333) — or at minimum renders the payload — and the
verification below passes without this patch.

verification:

```sh
cargo nextest run --locked osc_66_renders_payload_when_text_sizing_unsupported
(cd vendor/libghostty-vt && zig build test-lib-vt -Dtest-filter="OSC 66")
python3 -m unittest scripts.test_vendor_libghostty_vt
```
