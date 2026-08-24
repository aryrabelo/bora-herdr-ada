# Gates: bora-55c.1 — unified command schema: wt [scripts.run.*] survives

Scope: bora reads commands from wt's `.wt/settings.toml [scripts.run.*]` (with
precedence chain), deprecates `.bora.toml [[commands]]` with a warning, and
caches command definitions per repo root with mtime invalidation. Owned paths:
`src/bora_config.rs`, `website/src/data/config-reference.json`, this file.

- [x] G1: wt-declared commands enumerate: a repo with `.wt/settings.toml`
  containing `[scripts.run.*]` entries yields those commands through bora's
  command-loading path, with precedence `.wt/settings.local.toml` >
  `.wt/settings.toml` > `.conductor/settings.toml` > defaults, covered by unit
  tests.
  CHECK: cargo nextest run -E 'test(/command|scripts|wt|precedence/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   1.189s] 106 tests run: 106 passed, 3849 skipped

- [x] G2: `.bora.toml [[commands]]` still parses (deprecation window) AND a
  deprecation warning is emitted (tracing warn or returned warning surfaced to
  the user), with a test pinning both halves.
  CHECK: cargo nextest run -E 'test(/deprecat|bora_config|legacy/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.153s] 106 tests run: 106 passed, 3849 skipped

- [x] G3: definitions are cached per repo root with mtime invalidation — a
  second load within one mtime does NOT re-read/re-stat the file, and an mtime
  bump invalidates. Test pins both directions.
  CHECK: cargo nextest run -E 'test(/cache|mtime/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.639s] 37 tests run: 37 passed, 3918 skipped;
  probe throttle (gap fix 2026-08-24): cache_hit_within_probe_window_skips_stats
  (src/bora_config.rs:814) pins a second load within PROBE_THROTTLE performing
  ZERO stats and zero reads; mtime_bump_invalidates_cache (src/bora_config.rs:854)
  pins stat + invalidate past the window. Lead re-run of the CHECK above pending.

- [x] G4: config reference documents the surviving `[scripts.run.*]` schema and
  the `.bora.toml` migration/deprecation.
  CHECK: grep -c "scripts.run" website/src/data/config-reference.json
  EXPECT: /[1-9]/
  EVIDENCE: 7

- [x] G5: full suite green after the change (lead-run).
  CHECK: cargo nextest run 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.342s] 3954 tests run: 3954 passed, 1 skipped
