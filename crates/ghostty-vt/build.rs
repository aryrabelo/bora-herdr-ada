//! Build-time helpers. The FNV/vendored-source hash machinery below is
//! exercised by a subset of platforms and CI feature toggles; clippy's
//! host-target pass sees it unused, so treat it as conditionally dead.
#![allow(dead_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Deterministic (not cryptographic — integrity only) content hash over
/// (relative_path, bytes) pairs, NUL-framed so path/content boundaries can't
/// shift into a collision. Entries are sorted by path first, so the result
/// does not depend on filesystem iteration order.
fn hash_entries(entries: &mut [(String, Vec<u8>)]) -> u64 {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = FNV_OFFSET;
    for (rel_path, bytes) in entries.iter() {
        hash = fnv1a(hash, rel_path.as_bytes());
        hash = fnv1a(hash, &[0]);
        hash = fnv1a(hash, bytes);
        hash = fnv1a(hash, &[0]);
    }
    hash
}

fn collect_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
    } else if path.is_dir() {
        let Ok(read_dir) = fs::read_dir(path) else {
            return;
        };
        for entry in read_dir.flatten() {
            collect_files(&entry.path(), out);
        }
    }
}

// Paths (relative to the repository root) that determine the compiled
// libghostty-vt static lib. Mirrors the cargo:rerun-if-changed list in main —
// if a from-source build would react to a change here, the prebuilt staleness
// hash must react to it too.
const VENDOR_SOURCE_ROOTS: &[&str] = &[
    "vendor/libghostty-vt.vendor.json",
    "vendor/libghostty-vt/build.zig",
    "vendor/libghostty-vt/build.zig.zon",
    "vendor/libghostty-vt/VERSION",
    "vendor/libghostty-vt/include",
    "vendor/libghostty-vt/pkg",
    "vendor/libghostty-vt/src",
];

/// The vendored source and the `prebuilt/` directory stay at the repository
/// root, two levels above this crate's manifest.
fn repo_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .ancestors()
        .nth(2)
        .expect("crates/ghostty-vt has no repository root above it")
        .to_path_buf()
}

/// Hashes the vendor tree with repository-root-relative paths, so the stamp
/// value does not depend on where this crate's manifest lives.
fn hash_vendor_source(repo_root: &Path) -> u64 {
    let mut files = Vec::new();
    for root in VENDOR_SOURCE_ROOTS {
        collect_files(&repo_root.join(root), &mut files);
    }
    let mut entries: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(repo_root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&path).unwrap_or_default();
            (rel_str, bytes)
        })
        .collect();
    hash_entries(&mut entries)
}

fn prebuilt_stamp_path(candidate: &Path) -> PathBuf {
    candidate.with_extension("vendor-hash")
}

/// Checks the `.vendor-hash` stamp next to an auto-detected prebuilt .a
/// against the current vendor/libghostty-vt sources. Missing/malformed/stale
/// stamp => reject with a cargo:warning so the caller falls back to
/// from-source instead of silently linking a stale prebuilt.
fn prebuilt_stamp_matches(candidate: &Path, repo_root: &Path) -> bool {
    let stamp_path = prebuilt_stamp_path(candidate);
    let stamp = match fs::read_to_string(&stamp_path) {
        Ok(s) => s,
        Err(_) => {
            println!(
                "cargo:warning=stale prebuilt: {} has no {} stamp; rebuilding libghostty-vt from source",
                candidate.display(),
                stamp_path.display()
            );
            return false;
        }
    };
    let Ok(stamped_hash) = u64::from_str_radix(stamp.trim(), 16) else {
        println!(
            "cargo:warning=stale prebuilt: {} has a malformed vendor-hash stamp; rebuilding libghostty-vt from source",
            stamp_path.display()
        );
        return false;
    };
    let current_hash = hash_vendor_source(repo_root);
    if stamped_hash != current_hash {
        println!(
            "cargo:warning=stale prebuilt: {} vendor hash {stamped_hash:016x} does not match current vendor/libghostty-vt ({current_hash:016x}); rebuilding libghostty-vt from source",
            candidate.display()
        );
        return false;
    }
    true
}

/// `build.rs --write-stamp <zig-target>` — regenerates the `.vendor-hash`
/// stamp for `prebuilt/libghostty-vt-<target>.a` from the current vendor
/// sources. build.rs has no external deps, so it can be compiled and run
/// standalone (`rustc --edition 2021 crates/ghostty-vt/build.rs -o tool &&
/// CARGO_MANIFEST_DIR=crates/ghostty-vt ./tool --write-stamp <target>`) — this
/// is the single source of truth the guard above checks against, so writer
/// and reader can never drift apart.
fn write_prebuilt_stamp(target: &str) {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = repo_root(&manifest_dir);
    let candidate = repo_root.join(format!("prebuilt/libghostty-vt-{target}.a"));
    let hash = hash_vendor_source(&repo_root);
    let stamp_path = prebuilt_stamp_path(&candidate);
    fs::create_dir_all(stamp_path.parent().expect("stamp path has no parent"))
        .expect("failed to create prebuilt dir");
    fs::write(&stamp_path, format!("{hash:016x}\n")).expect("failed to write vendor-hash stamp");
    println!("wrote {} ({hash:016x})", stamp_path.display());
}

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "aarch64-apple-ios" => "aarch64-ios",
        "aarch64-apple-ios-sim" => "aarch64-ios-simulator",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("unsupported target for libghostty-vt build: {other}"),
    }
}

fn env_bool(name: &str) -> Option<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            other => panic!("invalid boolean value for {name}: {other}"),
        },
        Err(env::VarError::NotPresent) => None,
        Err(err) => panic!("failed to read {name}: {err}"),
    }
}

/// Library file name the link step asks for: `-l static=ghostty-vt` resolves
/// to `libghostty-vt.a`, the MSVC archive is linked as `ghostty-vt-static`.
fn static_link_name(target: &str) -> &'static str {
    if target.contains("windows-msvc") {
        "ghostty-vt-static"
    } else {
        "ghostty-vt"
    }
}

fn static_archive_file_name(target: &str) -> String {
    if target.contains("windows-msvc") {
        format!("{}.lib", static_link_name(target))
    } else {
        format!("lib{}.a", static_link_name(target))
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--write-stamp") {
        let target = args
            .get(pos + 1)
            .expect("--write-stamp requires a <zig-target> argument");
        write_prebuilt_stamp(target);
        return;
    }

    // The vendored source stays at the repository root, shared with its maintenance scripts.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = repo_root(&manifest_dir);
    let vendor_dir = repo_root.join("vendor");
    let vendored_dir = vendor_dir.join("libghostty-vt");
    let prebuilt_dir = repo_root.join("prebuilt");

    println!("cargo:rerun-if-changed=build.rs");
    for path in [
        vendor_dir.join("libghostty-vt.vendor.json"),
        vendored_dir.join("build.zig"),
        vendored_dir.join("build.zig.zon"),
        vendored_dir.join("include"),
        vendored_dir.join("pkg"),
        vendored_dir.join("src"),
        vendored_dir.join("VERSION"),
        prebuilt_dir.clone(),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_WINDOWS_LIBC");

    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let simd = env_bool("LIBGHOSTTY_VT_SIMD").unwrap_or(true);
    let target = env::var("TARGET").expect("TARGET");
    let zig_target = zig_target(&target);

    // Install into this build's OUT_DIR so concurrent builds for different targets
    // from one checkout never link each other's archive.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // ponytail: prebuilt bypass — stopgap because ambient zig lacked the SDK link for the vendored libghostty-vt
    // which cannot link the macOS 26 SDK, and zig 0.16 is rejected by the vendored build.zig.
    // Remove once upstream Ghostty's zig-0.16 migration (PR #12726) lands and we vendor-update;
    // at that point delete this block and return to from-source build.
    //
    // Auto-detected prebuilt/libghostty-vt-<target>.a must carry a matching
    // .vendor-hash stamp (see prebuilt_stamp_matches) or it's ignored and we
    // fall through to from-source — this is what stops a stale .a (built
    // against an older vendor tree) from linking silently. LIBGHOSTTY_VT_PREBUILT
    // is an explicit manual override and skips the guard: whoever sets it owns
    // the consequences.
    //
    // This build script belongs to a library crate, so `rustc-link-arg` would
    // not reach the final `bora` binary; the archive is copied into OUT_DIR
    // under its canonical name and linked with `rustc-link-lib`, which does
    // propagate. The dedicated directory holds only the static archive, so
    // Apple's linker cannot prefer a sibling dylib.
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_PREBUILT");
    let prebuilt: Option<PathBuf> = if let Ok(p) = env::var("LIBGHOSTTY_VT_PREBUILT") {
        Some(PathBuf::from(p))
    } else {
        let candidate = prebuilt_dir.join(format!("libghostty-vt-{zig_target}.a"));
        if candidate.exists() && prebuilt_stamp_matches(&candidate, &repo_root) {
            Some(candidate)
        } else {
            None
        }
    };
    if let Some(path) = prebuilt {
        let path = path
            .canonicalize()
            .expect("failed to canonicalize prebuilt path");
        println!("cargo:rerun-if-changed={}", path.display());
        let link_dir = out_dir.join("prebuilt-lib");
        fs::create_dir_all(&link_dir)
            .expect("failed to create prebuilt libghostty-vt link directory");
        fs::copy(&path, link_dir.join(static_archive_file_name(&target)))
            .expect("failed to copy prebuilt libghostty-vt static archive");
        println!("cargo:rustc-link-search=native={}", link_dir.display());
        println!("cargo:rustc-link-lib=static={}", static_link_name(&target));
        return;
    }

    let version_string = fs::read_to_string(vendored_dir.join("VERSION"))
        .expect("failed to read vendored libghostty-vt VERSION")
        .trim()
        .to_string();
    let zig_prefix = out_dir.join("zig-out");

    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("--prefix")
        .arg(&zig_prefix)
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version_string}"))
        .arg("-Demit-xcframework=false");
    if target.ends_with("windows-msvc") {
        if let Some(libc_file) = env::var_os("LIBGHOSTTY_VT_WINDOWS_LIBC") {
            println!(
                "cargo:rerun-if-changed={}",
                PathBuf::from(&libc_file).display()
            );
            command.arg("--libc").arg(libc_file);
        }
    }
    if let Ok(system_dir) = env::var("LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }

    let status = command
        .current_dir(&vendored_dir)
        .status()
        .unwrap_or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                panic!(
                    "zig executable not found (looked for {zig:?}; set the ZIG \
                     environment variable to point at the zig binary). Building \
                     the vendored libghostty-vt requires Zig 0.16.0: on macOS run \
                     `brew install zig` (0.16.0), elsewhere install it from \
                     https://ziglang.org/download/, then retry the build"
                );
            }
            panic!("failed to execute zig build for vendored libghostty-vt: {err}");
        });
    assert!(
        status.success(),
        "zig build for vendored libghostty-vt failed: {status}. \
         Building bora requires Zig 0.16.0; check `zig version` \
         or set ZIG to the path of a Zig 0.16.0 binary, then retry"
    );

    let mut lib_dir = zig_prefix.join("lib");
    if target.contains("-apple-") {
        // Apple's linker prefers the sibling dylib for `-l`, so search a directory
        // holding only the static archive.
        let static_dir = out_dir.join("lib");
        fs::create_dir_all(&static_dir).expect("failed to create libghostty-vt link directory");
        fs::copy(
            lib_dir.join("libghostty-vt.a"),
            static_dir.join("libghostty-vt.a"),
        )
        .expect("failed to copy libghostty-vt static archive");
        lib_dir = static_dir;
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static={}", static_link_name(&target));
}

// Run standalone (not wired into `cargo test`/nextest — build.rs is a
// build-dependency binary, not part of the crate's test target):
//   rustc --edition 2021 --test crates/ghostty-vt/build.rs -o target/build_rs_tests && target/build_rs_tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_entries_is_order_independent() {
        let mut a = vec![
            ("b.zig".to_string(), b"two".to_vec()),
            ("a.zig".to_string(), b"one".to_vec()),
        ];
        let mut b = vec![
            ("a.zig".to_string(), b"one".to_vec()),
            ("b.zig".to_string(), b"two".to_vec()),
        ];
        assert_eq!(hash_entries(&mut a), hash_entries(&mut b));
    }

    #[test]
    fn hash_entries_changes_when_content_changes() {
        let mut a = vec![("src/x.zig".to_string(), b"old".to_vec())];
        let mut b = vec![("src/x.zig".to_string(), b"new".to_vec())];
        assert_ne!(hash_entries(&mut a), hash_entries(&mut b));
    }

    #[test]
    fn hash_entries_changes_when_path_changes() {
        let mut a = vec![("src/a.zig".to_string(), b"same".to_vec())];
        let mut b = vec![("src/b.zig".to_string(), b"same".to_vec())];
        assert_ne!(hash_entries(&mut a), hash_entries(&mut b));
    }

    #[test]
    fn hash_entries_is_deterministic() {
        let mut a = vec![("src/x.zig".to_string(), b"content".to_vec())];
        let mut b = a.clone();
        assert_eq!(hash_entries(&mut a), hash_entries(&mut b));
    }

    #[test]
    fn repo_root_is_two_levels_above_the_crate_manifest() {
        let manifest = Path::new("/repo/crates/ghostty-vt");
        assert_eq!(repo_root(manifest), PathBuf::from("/repo"));
    }
}
