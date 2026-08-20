//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The upstream herdr release this fork's `master` merge is based on, and the
/// upstream commit that merge brought in. Both are updated by hand during an
/// upstream sync — see the "Fork version identity" rule in AGENTS.md.
pub const UPSTREAM_HERDR_VERSION: &str = "0.8.2";
pub const UPSTREAM_HERDR_COMMIT: &str = "2c042bb2";

/// Human-facing fork identity, e.g. `v0.8.1[a5c69bea].bora-24`: the upstream
/// herdr release, the upstream commit merged into this fork, and this fork's
/// own build number (`BASE_VERSION`'s minor, plus its patch when non-zero).
///
/// Display only. Every machine comparison — update checks, the wire protocol
/// `version` field, live-handoff acceptance, seen-state storage keys — keeps
/// using [`version`]/[`BASE_VERSION`], which stay plain semver.
pub fn fork_version_display() -> String {
    format!(
        "v{UPSTREAM_HERDR_VERSION}[{UPSTREAM_HERDR_COMMIT}].bora-{}",
        fork_build_number()
    )
}

fn fork_build_number() -> String {
    match crate::update::Version::parse(BASE_VERSION) {
        Some(version) if version.patch == 0 => version.minor.to_string(),
        Some(version) => format!("{}.{}", version.minor, version.patch),
        None => BASE_VERSION.to_string(),
    }
}

pub fn channel() -> &'static str {
    non_empty(option_env!("HERDR_BUILD_CHANNEL")).unwrap_or("stable")
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID"))
}

pub fn version() -> String {
    match channel() {
        "stable" => BASE_VERSION.to_string(),
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }
}
