# herdr task runner

# Run tests
test:
    cargo nextest run --locked --status-level fail --final-status-level fail --failure-output final --success-output never
    python3 -m unittest scripts.test_agent_detection_manifest_check scripts.test_changelog scripts.test_config_reference_check scripts.test_docs_translation_parity scripts.test_hermes_integration_asset scripts.test_independent_review scripts.test_package_windows_conpty scripts.test_preview scripts.test_review_rules scripts.test_review_rules_e2e scripts.test_unix_installer scripts.test_vendor_libghostty_vt scripts.test_vendor_portable_pty
    just ui-hot-path-architecture-test
    just integration-assets-test
    just plugin-marketplace-test

# Run one nextest filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final --success-output never

# Enforce deterministic UI hot-path architecture boundaries
ui-hot-path-architecture-test:
    python3 -m unittest scripts.test_ui_hot_path_architecture

# Run fast local lint checks
[unix]
lint:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings \
        -A clippy::dbg_macro \
        -A clippy::todo \
        -A clippy::cognitive_complexity \
        -A clippy::too_many_lines
    @gated=$(grep -rlF '#![cfg(not(target_os = "macos"))]' src tests 2>/dev/null || true); \
    if [ -n "$gated" ] && [ "$(uname)" = "Darwin" ]; then \
        echo ""; \
        echo "note: these files are gated off entirely on macOS and were NOT compiled or linted by the clippy run above:"; \
        printf '  %s\n' $gated; \
        echo "verify them on CI's ubuntu-latest leg (or a Linux box) before trusting a green just lint here."; \
    fi

[script("powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File")]
[windows]
lint:
    & .\scripts\windows_check.ps1 -Mode lint

# Run PR CI checks
[unix]
ci filter='all()': lint
    cargo nextest run --locked -E "{{filter}}" --status-level fail --final-status-level slow --failure-output final --success-output never
    just ui-hot-path-architecture-test
    just integration-assets-test
    just plugin-marketplace-test

# Run Windows target lint from Unix/macOS to catch cfg(windows) compile and clippy failures before CI
[unix]
windows-lint:
    rustup target add x86_64-pc-windows-msvc
    LIBGHOSTTY_VT_SIMD=false cargo clippy --bin bora --locked --target x86_64-pc-windows-msvc -- -D warnings \
        -A clippy::dbg_macro \
        -A clippy::todo \
        -A clippy::cognitive_complexity \
        -A clippy::too_many_lines

# Check formatting + run unit tests + maintenance script tests
# Windows target lint is commented out on purpose: this fork does not ship or use
# Windows builds, and `windows-lint` downloads the msvc target and type-checks the
# whole Windows tree on every `just check`. Run `just windows-lint` by hand if a
# change touches src/platform/windows.rs.
[unix]
check: ci
    python3 -m unittest scripts.test_agent_detection_manifest_check scripts.test_changelog scripts.test_config_reference_check scripts.test_docs_translation_parity scripts.test_hermes_integration_asset scripts.test_independent_review scripts.test_package_windows_conpty scripts.test_preview scripts.test_review_rules scripts.test_review_rules_e2e scripts.test_unix_installer scripts.test_vendor_libghostty_vt scripts.test_vendor_portable_pty
    @echo "docs reminder: if this changes user-facing behavior, make sure the relevant release docs are updated or called out before release."

[script("powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File")]
[windows]
check:
    & .\scripts\windows_check.ps1 -Mode check

# Install repo-local git hooks
install-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/pre-commit
    chmod +x .githooks/commit-msg
    @echo "installed git hooks from .githooks"

# Report upstream symbols (fn/mod) absent from this fork, split production vs test.
# Not part of `check`: it needs the `upstream` remote/ref, which CI may lack.
upstream-drift:
    python3 scripts/upstream_drift.py --report

# Build release binary
[unix]
build:
    cargo build --release --locked

[script("powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File")]
[windows]
build:
    cargo build --release --locked

# Non-gating full-render scaling profile for background workspaces and active panes
bench-render-scale:
    cargo test --release --locked --bin bora render_scale_profile -- --ignored --nocapture --test-threads=1

# ~3-5 minute CPU comparison; downloads stable unless HERDR_PERF_BASELINE_BIN is set
bench-release-smoke:
    cargo build --release --locked
    scripts/release_perf_smoke.sh "${CARGO_TARGET_DIR:-target}/release/herdr"

# Build the website and documentation
website-build:
    cd website && bun install --frozen-lockfile && bun run build

# Test bundled agent integration assets
integration-assets-test:
    bun test src/integration/assets/herdr-agent-state.test.ts
    bun test src/integration/assets/opencode/herdr-agent-state.test.ts
    bun test src/integration/assets/opencode/herdr-tui-session.test.ts
# Run plugin marketplace Worker tests
plugin-marketplace-test:
    cd workers/plugin-marketplace && bun install --frozen-lockfile && bun test


# Build the vendored libghostty-vt source dist
build-libghostty-vt:
    scripts/build_vendored_libghostty_vt.sh

# Fetch the prebuilt libghostty-vt static lib (+ .vendor-hash stamp) for this
# host into prebuilt/ (fallback when zig 0.15.2 cannot build locally — remove
# when upstream zig-0.16 port lands)
fetch-libghostty-vt:
    scripts/fetch_libghostty_vt_prebuilt.sh

# Cross-build prebuilt/libghostty-vt-<target>.a + its .vendor-hash stamp
# locally in a Linux container (zig 0.15.2), no GitHub Actions. macOS 26 fast
# dev loop; remove with the rest of the prebuilt fallback when upstream
# zig-0.16 port lands.
build-libghostty-vt-prebuilt:
    scripts/build_libghostty_vt_prebuilt.sh

# Alias: regenerate the local prebuilt/libghostty-vt-<target>.a shortcut
# (from-source cross-build + matching .vendor-hash stamp) so build.rs's
# staleness guard accepts it again.
prebuilt-ghostty: build-libghostty-vt-prebuilt

# Check that release docs and changelog have been finalized from docs/next before release
release-docs-check:
    python3 scripts/agent_detection_manifest_check.py --require-website
    python3 scripts/config_reference_check.py
    node website/scripts/docs-versions.mjs check
    node website/scripts/docs-preview.mjs check
    @test -f docs/next/README.md
    @test -f docs/next/README.zh-CN.md
    @test -f docs/next/README.pt-BR.md
    @python3 scripts/changelog.py check-history-sync || { \
        echo "run this before releasing: reconcile CHANGELOG.md and docs/next/CHANGELOG.md (the staging file)"; \
        exit 1; \
    }
    @for file in CONFIGURATION.md INTEGRATIONS.md SOCKET_API.md; do \
        if [ -e "$file" ]; then \
            echo "error: $file was replaced by website docs; remove the root copy"; \
            exit 1; \
        fi; \
    done
    @test -d docs/next/website/src/content/docs
    @for file in docs/next/website/src/content/docs/*.mdx; do \
        for locale in ja zh-cn; do \
            translated="docs/next/website/src/content/docs/$locale/$(basename "$file")"; \
            if [ ! -f "$translated" ]; then \
                echo "error: $translated is missing; translate next docs before releasing"; \
                exit 1; \
            fi; \
        done; \
    done
    @for file in docs/next/website/src/content/docs/ja/*.mdx docs/next/website/src/content/docs/zh-cn/*.mdx; do \
        staged="docs/next/website/src/content/docs/$(basename "$file")"; \
        if [ ! -f "$staged" ]; then \
            echo "error: $file has no matching english doc; remove the stale translation"; \
            exit 1; \
        fi; \
    done
    python3 scripts/docs_translation_parity.py --docs-root docs/next/website/src/content/docs
    just website-build
    cd website && bun run build:draft

# Validate release docs, render scaling, and end-to-end CPU before release preparation
pre-release-check:
    just release-docs-check
    just bench-render-scale
    just bench-release-smoke
    @echo "release review required: investigate material render-scaling regressions before publishing."
    @echo "release review required: update skills/herdr/SKILL.md for this stable release so it matches the current CLI, IDs, agent lifecycle semantics, and safety guidance."
    @echo "release policy: do not update skills/herdr/SKILL.md between stable releases; preview builds keep the latest stable skill."

# Prepare the release commit without tagging or pushing (usage: just release-prepare 0.1.1)
release-prepare version:
    @printf '%s\n' '{{version}}' | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { \
        echo "error: version must look like 0.6.6 without a v prefix"; \
        exit 1; \
    }
    @if ! git diff --quiet -- . ':(exclude)skills/herdr/SKILL.md' || \
        ! git diff --cached --quiet -- . ':(exclude)skills/herdr/SKILL.md' || \
        [ -n "$(git ls-files --others --exclude-standard)" ]; then \
        echo "error: commit all changes except skills/herdr/SKILL.md first"; \
        exit 1; \
    fi
    @git fetch origin main --tags
    @if git rev-parse "v{{version}}" >/dev/null 2>&1; then \
        echo "error: tag v{{version}} already exists"; \
        exit 1; \
    fi
    just pre-release-check
    python3 scripts/changelog.py prepare --version {{version}} --path docs/next/CHANGELOG.md
    cp docs/next/CHANGELOG.md CHANGELOG.md
    sed -i.bak 's/^version = ".*"/version = "{{version}}"/' Cargo.toml && rm -f Cargo.toml.bak
    cargo update -p bora --offline
    just check
    git add CHANGELOG.md docs/next/CHANGELOG.md Cargo.toml Cargo.lock skills/herdr/SKILL.md
    git diff --cached --quiet || git commit -m "release: v{{version}}"
    @echo "v{{version}} release commit prepared. Review it, then run: just release-publish {{version}}"

# Tag and push an already-prepared release commit (usage: just release-publish 0.1.1)
release-publish version:
    @printf '%s\n' '{{version}}' | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { \
        echo "error: version must look like 0.6.6 without a v prefix"; \
        exit 1; \
    }
    @if [ -n "$(git status --porcelain)" ]; then \
        echo "error: working tree must be clean before publishing"; \
        exit 1; \
    fi
    @branch="$(git branch --show-current)"; \
    if [ "$branch" != "main" ]; then \
        echo "error: release-publish must run from main, got $branch"; \
        exit 1; \
    fi
    @git fetch origin main --tags
    @if git rev-parse "v{{version}}" >/dev/null 2>&1; then \
        echo "error: tag v{{version}} already exists"; \
        exit 1; \
    fi
    @cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"; \
    if [ "$cargo_version" != "{{version}}" ]; then \
        echo "error: Cargo.toml version $cargo_version does not match {{version}}"; \
        exit 1; \
    fi
    just release-docs-check
    python3 scripts/changelog.py extract --version {{version}} --output /tmp/herdr-release-notes-check.md
    rm -f /tmp/herdr-release-notes-check.md
    @local_head="$(git rev-parse HEAD)"; \
    remote_head="$(git rev-parse origin/main)"; \
    if ! git merge-base --is-ancestor "$remote_head" "$local_head"; then \
        echo "error: origin/main is not an ancestor of HEAD; pull or rebase before publishing"; \
        exit 1; \
    fi; \
    if [ "$local_head" != "$remote_head" ]; then \
        echo "pushing release commit to origin/main"; \
        git push origin HEAD:main; \
    fi
    git tag -a v{{version}} -m "v{{version}}"
    git push origin v{{version}}
    @echo "v{{version}} released — GitHub Actions building binaries and updating website/latest.json"

# Prepare, verify, tag, push, and trigger the GitHub Release workflow (usage: just release 0.1.1)
release version:
    just release-prepare {{version}}
    just release-publish {{version}}

# Print default config
default-config:
    cargo run --release --locked -- --default-config

# Install the fork's default plugins + keybind (idempotent). Run once per machine after installing bora.
bootstrap:
    #!/usr/bin/env bash
    set -euo pipefail
    reg="${XDG_CONFIG_HOME:-$HOME/.config}/bora/plugins.json"
    if [ -f "$reg" ] && grep -q herdr-file-viewer "$reg"; then
        echo "plugin herdr-file-viewer: already installed"
    else
        bora plugin install smarzban/herdr-file-viewer --yes
    fi
    cfg="${XDG_CONFIG_HOME:-$HOME/.config}/bora/config.toml"
    if [ -f "$cfg" ] && grep -q 'command = "open-file-viewer"' "$cfg"; then
        echo "keybind prefix+f: already set"
    else
        mkdir -p "$(dirname "$cfg")"
        printf '\n[[keys.command]]\nkey = "prefix+f"\ntype = "plugin_action"\ncommand = "open-file-viewer"\n' >> "$cfg"
        echo "keybind prefix+f -> open-file-viewer (right split): added to $cfg"
    fi
