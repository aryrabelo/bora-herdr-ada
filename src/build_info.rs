//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The upstream herdr release this fork's `master` merge is based on, and the
/// upstream commit that merge brought in. Both are updated by hand during an
/// upstream sync — see the "Fork version identity" rule in AGENTS.md.
pub const UPSTREAM_HERDR_VERSION: &str = "0.9.5";
pub const UPSTREAM_HERDR_COMMIT: &str = "2f3432c6";

/// Human-facing fork identity, `<upstream herdr release>+<bora version>`, e.g.
/// `0.9.5+0.52.11`: the upstream release this fork is merged up to, then this
/// fork's own version (with the channel suffix [`version`] adds on preview).
///
/// Display only. Every machine comparison — update checks, the wire protocol
/// `version` field, live-handoff acceptance, seen-state storage keys — keeps
/// using [`version`]/[`BASE_VERSION`], which stay plain semver. The fork version
/// cannot move into semver build metadata (`+`): precedence ignores it, so two
/// bora builds on the same upstream would compare equal.
pub fn fork_version_display() -> String {
    format!("{UPSTREAM_HERDR_VERSION}+{}", version())
}

/// `bora --version` line: the fork identity plus the upstream commit merged in.
pub fn version_line() -> String {
    format!(
        "bora {} (herdr {UPSTREAM_HERDR_COMMIT})",
        fork_version_display()
    )
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
