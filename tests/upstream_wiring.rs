//! Guards against the class of bug that killed the sidebar token feature:
//! upstream sync commit f485af71 (2026-08-01) deleted `mod tokens;` from
//! src/ui/sidebar.rs while leaving src/ui/sidebar/tokens.rs on disk, so the
//! configurable sidebar metadata-token feature silently stopped rendering.
//!
//! Rather than reimplementing rustc's module resolution with a `mod`-line
//! regex (which was tried and falsely accused src/terminal/metadata.rs and
//! src/server/headless/tests/pane_graphics.rs, both wired via `#[path]`),
//! this asks the resolver directly: cargo's dep-info (`.d`) files list every
//! source file that actually entered a compilation unit. A file present on
//! disk but absent from every dep-info union is not compiled, `mod` line or
//! not.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// src/ files legitimately not compiled on this host. Each entry is checked
/// two ways: it must still exist, and it must still be UNREACHED by the dep
/// info union -- an allowlisted file that starts compiling is a stale (lying)
/// entry, not a pass.
///
/// The list is host-specific: CI runs this suite on both macOS and Linux, and
/// each platform backend only compiles on its own OS, so a file that is
/// allowlisted on one host is a live module on the other.
#[cfg(target_os = "macos")]
const PLATFORM_GATED_ALLOWLIST: &[(&str, &str)] = &[
    (
        "src/platform/windows.rs",
        "windows-only platform backend, not compiled on macOS",
    ),
    (
        "src/platform/linux.rs",
        "linux-only platform backend, not compiled on macOS",
    ),
    (
        "src/platform/fallback.rs",
        "non-windows/linux/macos fallback backend, not compiled on macOS",
    ),
    (
        "src/platform/windows/clipboard_image.rs",
        "windows-only clipboard support, not compiled on macOS",
    ),
    (
        "src/pane/terminal/windows_recent_fallback.rs",
        "windows-only pty fallback, not compiled on macOS",
    ),
];

/// Linux CI host: same contract as the macOS list above -- windows-only
/// backends and the macOS backend are legitimately unreached here.
#[cfg(not(target_os = "macos"))]
const PLATFORM_GATED_ALLOWLIST: &[(&str, &str)] = &[
    (
        "src/platform/windows.rs",
        "windows-only platform backend, not compiled on Linux",
    ),
    (
        "src/platform/macos.rs",
        "macos-only platform backend, not compiled on Linux",
    ),
    (
        "src/platform/fallback.rs",
        "non-windows/linux/macos fallback backend, not compiled on Linux",
    ),
    (
        "src/platform/windows/clipboard_image.rs",
        "windows-only clipboard support, not compiled on Linux",
    ),
    (
        "src/pane/terminal/windows_recent_fallback.rs",
        "windows-only pty fallback, not compiled on Linux",
    ),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Split dep-info deps on whitespace, honoring `\` line continuations and
/// `\ ` escaped spaces inside a single dependency path.
fn split_dep_targets(raw: &str) -> Vec<String> {
    // Join continuation lines (trailing unescaped `\` at end of line) into one logical line.
    let mut joined = String::new();
    for line in raw.lines() {
        if let Some(stripped) = line.strip_suffix('\\') {
            joined.push_str(stripped);
        } else {
            joined.push_str(line);
            joined.push('\n');
        }
    }

    let mut deps = Vec::new();
    for logical_line in joined.split('\n') {
        // `target: dep dep dep` -- drop the `target:` prefix.
        let Some(colon) = find_unescaped_colon(logical_line) else {
            continue;
        };
        let rest = &logical_line[colon + 1..];

        let mut current = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' && chars.peek() == Some(&' ') {
                current.push(' ');
                chars.next();
            } else if c.is_whitespace() {
                if !current.is_empty() {
                    deps.push(std::mem::take(&mut current));
                }
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            deps.push(current);
        }
    }
    deps
}

/// Windows drive-letter colons (`C:\...`) aren't relevant on this host, but
/// guard against matching an escaped colon anyway; dep-info targets don't
/// escape `:` in practice, so the first unescaped one is the separator.
fn find_unescaped_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && (i == 0 || bytes[i - 1] != b'\\') {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Union every `.d` file under the four dep-info locations cargo writes to,
/// normalized to repo-root-relative `src/...` forward-slash paths.
fn reached_src_files() -> (HashSet<String>, usize) {
    let root = repo_root();
    let mut dep_files = Vec::new();
    for rel_dir in [
        "target/debug/deps",
        "target/release/deps",
        "target/debug",
        "target/release",
    ] {
        let dir = root.join(rel_dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("d") {
                dep_files.push(path);
            }
        }
    }

    let mut reached = HashSet::new();
    for dep_file in &dep_files {
        let Ok(raw) = fs::read_to_string(dep_file) else {
            continue;
        };
        for dep in split_dep_targets(&raw) {
            let dep_path = Path::new(&dep);
            let rel = if let Ok(stripped) = dep_path.strip_prefix(&root) {
                stripped.to_path_buf()
            } else if dep_path.is_absolute() {
                continue; // absolute path outside the repo (e.g. cargo registry sources)
            } else {
                dep_path.to_path_buf()
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str.starts_with("src/") {
                reached.insert(rel_str);
            }
        }
    }
    (reached, dep_files.len())
}

fn all_src_rs_files() -> Vec<String> {
    let root = repo_root();
    let mut out = Vec::new();
    walk_rs(&root.join("src"), &root, &mut out);
    out.sort();
    out
}

fn walk_rs(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, root, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn sidebar_token_rendering_stays_wired() {
    let (reached, dep_file_count) = reached_src_files();
    if dep_file_count == 0 {
        eprintln!(
            "SKIP sidebar_token_rendering_stays_wired: no dep-info (.d) files found under target/ \
             -- run `cargo build --bin bora` (or `cargo build --tests`) first. A green result from \
             a missing instrument is exactly the failure mode this test exists to prevent, so this \
             is a loud skip, not a silent pass."
        );
        return;
    }

    assert!(
        reached.contains("src/ui/sidebar/tokens.rs"),
        "src/ui/sidebar/tokens.rs is on disk but not reachable from any compiled crate root. \
         This is the exact failure class introduced by upstream sync commit f485af71 \
         (2026-08-01, \"finalize upstream sync\"): it deleted `mod tokens;` from \
         src/ui/sidebar.rs while leaving the file on disk, silently killing the configurable \
         sidebar metadata-token feature (ui.sidebar.agents.rows in config.toml) for two weeks \
         with no compiler error, because orphaned files don't fail a build."
    );
}

#[test]
fn no_source_file_is_orphaned_from_the_build() {
    let (reached, dep_file_count) = reached_src_files();
    if dep_file_count == 0 {
        eprintln!(
            "SKIP no_source_file_is_orphaned_from_the_build: no dep-info (.d) files found under \
             target/ -- run `cargo build --bin bora` (or `cargo build --tests`) first."
        );
        return;
    }

    let root = repo_root();
    let mut failures = Vec::new();

    for (allowed_path, reason) in PLATFORM_GATED_ALLOWLIST {
        let abs = root.join(allowed_path);
        if !abs.exists() {
            failures.push(format!(
                "allowlisted file no longer exists, remove it from PLATFORM_GATED_ALLOWLIST: {allowed_path} ({reason})"
            ));
        } else if reached.contains(*allowed_path) {
            failures.push(format!(
                "stale allowlist entry: {allowed_path} is now reachable from the build \
                 (reason on file was: \"{reason}\"), remove it from PLATFORM_GATED_ALLOWLIST -- \
                 an allowlist entry that no longer matches reality is a lie"
            ));
        }
    }

    let allowlisted: HashSet<&str> = PLATFORM_GATED_ALLOWLIST.iter().map(|(p, _)| *p).collect();
    for src_file in all_src_rs_files() {
        if allowlisted.contains(src_file.as_str()) {
            continue;
        }
        if !reached.contains(&src_file) {
            failures.push(format!(
                "{src_file} exists on disk but is not reachable from any compiled crate root. \
                 This is the same failure class that killed the sidebar token feature (upstream \
                 sync f485af71 deleted `mod tokens;` while leaving the file on disk): either wire \
                 it in (mod/use/#[path]) or add it to PLATFORM_GATED_ALLOWLIST with a reason if it \
                 is legitimately platform-gated."
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "orphaned or misdeclared src/ files:\n{}",
        failures.join("\n")
    );
}
