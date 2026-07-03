//! Rendering of the configurable `[flow]` command template used by the
//! Issues tab "Run with bora-flow" action.
//!
//! Herdr stays agnostic of the external flow orchestrator's CLI: the shell
//! command template configured in `config.toml` (global) or a repo's
//! `.bora.toml` (per-repo override) is the only coupling.

/// Values substituted into a flow command template.
pub(crate) struct FlowCommandContext {
    /// `owner/repo#N`, derived from `GitSpaceMetadata.repo_identity`
    /// (`host/owner/repo`) with the host segment stripped.
    pub issue_ref: String,
    /// Issue number.
    pub number: u64,
    /// Issue URL.
    pub url: String,
    /// Absolute repo checkout path.
    pub repo_path: String,
}

/// Single-quote a value for safe interpolation into a `sh -c` command string.
/// Same idiom as the right-panel diff path quoting in `navigate.rs`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Render a flow command template by substituting `{issue}`, `{number}`,
/// `{url}`, and `{repo}` placeholders. String-valued substitutions are
/// single-quoted so they are shell-safe; `{number}` is numeric and inserted
/// verbatim. Unknown placeholders are left literal.
pub(crate) fn render_flow_command(template: &str, ctx: &FlowCommandContext) -> String {
    template
        .replace("{issue}", &shell_quote(&ctx.issue_ref))
        .replace("{number}", &ctx.number.to_string())
        .replace("{url}", &shell_quote(&ctx.url))
        .replace("{repo}", &shell_quote(&ctx.repo_path))
}

/// Resolve the effective flow command template: the per-repo `.bora.toml`
/// override wins over the global `[flow]` config; blank templates count as
/// unset and disable the action.
pub(crate) fn resolve_flow_template(
    per_repo: Option<&str>,
    global: Option<&str>,
) -> Option<String> {
    let pick = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|template| !template.is_empty())
            .map(str::to_string)
    };
    pick(per_repo).or_else(|| pick(global))
}

/// Derive the `{issue}` value (`owner/repo#N`) from a repo identity of the
/// form `host/owner/repo`. Falls back to the full identity when there is no
/// host segment to strip.
pub(crate) fn issue_ref_from_repo_identity(repo_identity: &str, number: u64) -> String {
    let repo = repo_identity
        .split_once('/')
        .map_or(repo_identity, |(_host, rest)| rest);
    format!("{repo}#{number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> FlowCommandContext {
        FlowCommandContext {
            issue_ref: "owner/repo#12".to_string(),
            number: 12,
            url: "https://github.com/owner/repo/issues/12".to_string(),
            repo_path: "/home/user/src/repo".to_string(),
        }
    }

    #[test]
    fn renders_every_placeholder() {
        let rendered = render_flow_command(
            "bora-flow run {issue} --number {number} --url {url} --repo {repo}",
            &ctx(),
        );
        assert_eq!(
            rendered,
            "bora-flow run 'owner/repo#12' --number 12 \
             --url 'https://github.com/owner/repo/issues/12' --repo '/home/user/src/repo'"
        );
    }

    #[test]
    fn renders_subset_of_placeholders() {
        assert_eq!(
            render_flow_command("bora-flow run {issue}", &ctx()),
            "bora-flow run 'owner/repo#12'"
        );
    }

    #[test]
    fn template_without_placeholders_is_unchanged() {
        assert_eq!(
            render_flow_command("bora-flow run --latest", &ctx()),
            "bora-flow run --latest"
        );
    }

    #[test]
    fn unknown_placeholder_is_left_literal() {
        assert_eq!(
            render_flow_command("bora-flow run {issue} {branch}", &ctx()),
            "bora-flow run 'owner/repo#12' {branch}"
        );
    }

    #[test]
    fn single_quotes_in_values_are_escaped() {
        let ctx = FlowCommandContext {
            issue_ref: "owner/repo#7".to_string(),
            number: 7,
            url: "https://example.com/issues/7?q='quoted'".to_string(),
            repo_path: "/tmp/it's a repo".to_string(),
        };
        assert_eq!(
            render_flow_command("run {url} {repo}", &ctx),
            "run 'https://example.com/issues/7?q='\\''quoted'\\''' '/tmp/it'\\''s a repo'"
        );
    }

    #[test]
    fn resolve_prefers_per_repo_over_global() {
        assert_eq!(
            resolve_flow_template(Some("repo-cmd {issue}"), Some("global-cmd {issue}")),
            Some("repo-cmd {issue}".to_string())
        );
        assert_eq!(
            resolve_flow_template(None, Some("global-cmd {issue}")),
            Some("global-cmd {issue}".to_string())
        );
        assert_eq!(resolve_flow_template(None, None), None);
    }

    #[test]
    fn resolve_treats_blank_templates_as_unset() {
        assert_eq!(resolve_flow_template(Some("  "), None), None);
        assert_eq!(
            resolve_flow_template(Some(""), Some("global {issue}")),
            Some("global {issue}".to_string())
        );
        assert_eq!(
            resolve_flow_template(Some("  repo {issue}  "), None),
            Some("repo {issue}".to_string())
        );
    }

    #[test]
    fn issue_ref_strips_host_segment() {
        assert_eq!(
            issue_ref_from_repo_identity("github.com/owner/repo", 12),
            "owner/repo#12"
        );
        assert_eq!(
            issue_ref_from_repo_identity("local-repo", 3),
            "local-repo#3"
        );
    }
}
