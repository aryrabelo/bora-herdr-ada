# libghostty-vt local patches

This file tracks intentional local changes applied on top of the vendored
`libghostty-vt` source. Remove a patch only when the vendored source commit
contains the upstream behavior and the listed verification still passes.

## 0002 expose modifyOtherKeys mode through terminal data

status: active

patch: `vendor/patches/libghostty-vt/0002-expose-modify-other-keys-mode.patch`

herdr issue: none; fixes the performance regression exposed by
https://github.com/herdrdev/herdr/pull/2303

upstream discussion: not opened

upstream pr: not opened

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

local files:

- `vendor/libghostty-vt/include/ghostty/vt/terminal.h`
- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: Herdr must know whether xterm modifyOtherKeys mode 2 is active to
request printable key releases from the outer terminal. The formatter API can
recover this fact only by formatting the active screen and scrollback. A typed
terminal-data query exposes the authoritative scalar without formatting or
allocation. The local query uses value 41; upstream now owns the previous
local value 33 for VT processing errors.

remove when: the vendored source exposes an equivalent scalar query for
modifyOtherKeys mode 2 and Herdr can use it without this patch.

verification:

```sh
just test-one modify_other_keys
just test-one host_report_all_supplies_printable_releases_for_event_type_only_panes
just maintenance-test
just ui-hot-path-architecture-test
```

The former grapheme-default patch is replaced by upstream's public
`GHOSTTY_TERMINAL_OPT_MODE_DEFAULT` API. Herdr configures mode 2027 through that
API and tests that RIS restores it after a child disables it. The Wuffs C-only
mirror fix from Ghostty PR 13789 is also included in this vendored base.

## 0004 fix hosted Wuffs builds

status: active

patch: `vendor/patches/libghostty-vt/0004-fix-hosted-wuffs-builds.patch`

herdr issue: none; preserves Windows cross-compilation and non-SIMD hosted builds

upstream discussion: not opened

upstream pr: not opened

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

local files:

- `vendor/libghostty-vt/pkg/wuffs/build.zig`
- `vendor/libghostty-vt/pkg/wuffs/src/main.zig`

reason: Wuffs now needs MSVC libc headers when targeting Windows. Zig's
`--libc` configuration reaches C compilation, but the translate-c dependency
requires its own explicit configuration. Forward the same file so cross-builds
can use an actual Windows SDK instead of changing the target ABI or skipping
compilation. Native builds without a libc override are unchanged.

The no-libc Wuffs module also exports hidden weak calloc/free stubs. On hosted
Linux with SIMD disabled, those definitions override the Rust executable's
libc allocator, causing immediate allocation failures. Limit the stubs to
freestanding targets; hosted embedders resolve these symbols through libc.

remove when: upstream forwards the build's libc configuration to the Wuffs
translator and prevents hosted allocator interposition, and both Windows
cross-compilation and non-SIMD native tests pass without this patch.

verification:

```sh
LIBGHOSTTY_VT_WINDOWS_LIBC=/path/to/windows-libc.txt just windows-lint
LIBGHOSTTY_VT_SIMD=false just test-one ghostty
just maintenance-test
```

## 0005 bounded word selection for wrapped link activation

status: active

patch: `vendor/patches/libghostty-vt/0005-bounded-word-selection.patch`

herdr issue: https://github.com/herdrdev/herdr/issues/1282

upstream discussion: not opened

upstream pr: not opened; related merged PR https://github.com/ghostty-org/ghostty/pull/10132
implements URL selection in the application layer, not the libghostty C API.

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

local files:

- `vendor/libghostty-vt/include/ghostty/vt/selection.h`
- `vendor/libghostty-vt/src/lib_vt.zig`
- `vendor/libghostty-vt/src/terminal/Screen.zig`
- `vendor/libghostty-vt/src/terminal/c/main.zig`
- `vendor/libghostty-vt/src/terminal/c/selection.zig`

reason: Ctrl+click must resolve a wrapped token beyond the visible viewport
without scanning an arbitrarily long logical line. The new, opt-in API shares
one cell-inspection budget across both directions and returns no selection on
exhaustion, never a truncated link. Its scan skips wide-character spacer cells.
The existing word-selection functions and option layouts remain unchanged;
only Herdr's link activation uses the new function.

remove when: upstream provides an equivalent bounded, wrap-aware selection API
that handles wide-character spacers, and Herdr passes the tests below using it
without this patch.

verification:

```sh
just test-one link_target
just test-one link_activation
just test-one ctrl_click
just check
```

## 0003 let hosts opt out of reflow on resize

status: active

patch: `vendor/patches/libghostty-vt/0003-reflow-on-resize-opt-out.patch`

herdr issue: none; fixes the padded-transcript corruption measured on
2026-09-15 (pane 91 -> 53 cols emitted one 38-column remainder row after each
full row)

upstream discussion: not opened

upstream pr: not opened

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

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

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

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

## 0006 clear screen while preserving the cursor line

status: active

patch: `vendor/patches/libghostty-vt/0006-clear-screen-preserving-cursor-line.patch`

herdr issue: none; requested in https://github.com/herdrdev/herdr/discussions/545

upstream discussion: not opened

upstream pr: not opened

vendored base: `44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51`

local files:

- `vendor/libghostty-vt/include/ghostty/vt/terminal.h`
- `vendor/libghostty-vt/src/lib_vt.zig`
- `vendor/libghostty-vt/src/terminal/c/main.zig`
- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: Herdr needs an explicit screen/history clear that preserves the cursor's
visible soft-wrapped line without writing to the child or interrupting a partial
VT sequence. The new C function operates directly on the screen, leaves alternate
screens untouched, clears image placements, and marks the result dirty.

remove when: the vendored C API provides an equivalent parser-independent clear
operation preserving the visible cursor line, and Herdr passes the checks below
using it without this patch.

verification:

```sh
just test-one clear_pane
just maintenance-test
just check
```
