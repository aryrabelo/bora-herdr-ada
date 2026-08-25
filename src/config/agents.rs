//! Per-agent-kind interactive launch command overrides.
//!
//! `bora agent start --kind <kind>` normally types the agent's canonical
//! executable name (see `detect::interactive_agent_executable`) into the
//! target pane's already-running interactive shell. Some agents are wrapped
//! locally by a shell function or alias with different behavior (for
//! example a sandboxing wrapper); `[agents.commands]` lets a user point a
//! kind at a different single-token command instead, without bora having to
//! know anything about the wrapper.

use std::collections::BTreeMap;

use serde::Deserialize;

fn deserialize_agent_commands<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let commands = BTreeMap::<String, String>::deserialize(deserializer)?;
    for (id, command) in &commands {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in agents.commands"
            )));
        }
        if command.trim().is_empty() || command.chars().any(char::is_whitespace) {
            return Err(serde::de::Error::custom(format!(
                "agents.commands.{id} must be a single executable token with no whitespace"
            )));
        }
    }
    Ok(commands)
}
fn deserialize_agent_default<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let default = Option::<String>::deserialize(deserializer)?;
    if let Some(id) = &default {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in agents.default"
            )));
        }
    }
    Ok(default)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Per-agent-kind override for the executable `agent start` launches,
    /// keyed by canonical agent id (e.g. `omp`). Falls back to
    /// `detect::interactive_agent_executable` when an id has no override.
    #[serde(deserialize_with = "deserialize_agent_commands")]
    pub commands: BTreeMap<String, String>,
    /// Agent kind `bora agent --new` starts when the caller passes no
    /// `--kind`. Falls back to the hardcoded `"omp"` when unset.
    #[serde(deserialize_with = "deserialize_agent_default")]
    pub default: Option<String>,
}

impl AgentsConfig {
    /// The executable `agent start` should type for `agent`: the configured
    /// override, or the canonical interactive executable when unset.
    pub fn command_for(&self, agent: crate::detect::Agent) -> &str {
        self.commands
            .get(crate::detect::agent_label(agent))
            .map(String::as_str)
            .unwrap_or_else(|| crate::detect::interactive_agent_executable(agent))
    }
    /// The agent kind for `bora agent --new`: the configured default, or
    /// `"omp"`. A `--kind` CLI flag wins over both — handled at the call
    /// site, not here.
    pub fn default_kind(&self) -> &str {
        self.default.as_deref().unwrap_or("omp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_no_overrides_and_canonical_executables() {
        let config = AgentsConfig::default();
        assert!(config.commands.is_empty());
        assert_eq!(
            config.command_for(crate::detect::Agent::Omp),
            crate::detect::interactive_agent_executable(crate::detect::Agent::Omp)
        );
    }

    #[test]
    fn parses_and_applies_a_configured_override() {
        let config: crate::config::Config = toml::from_str(
            r#"
[agents.commands]
omp = "omp-raw"
"#,
        )
        .expect("agents config");
        assert_eq!(
            config.agents.command_for(crate::detect::Agent::Omp),
            "omp-raw"
        );
        // Agents without an override still resolve to their canonical
        // executable.
        assert_eq!(
            config.agents.command_for(crate::detect::Agent::Claude),
            "claude"
        );
    }

    #[test]
    fn rejects_an_unknown_agent_id() {
        let result: Result<crate::config::Config, _> = toml::from_str(
            r#"
[agents.commands]
not-a-real-agent = "foo"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_a_command_containing_whitespace() {
        let result: Result<crate::config::Config, _> = toml::from_str(
            r#"
[agents.commands]
omp = "omp-raw --danger"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_an_empty_command() {
        let result: Result<crate::config::Config, _> = toml::from_str(
            r#"
[agents.commands]
omp = ""
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn parses_a_configured_default_kind() {
        let config: AgentsConfig = toml::from_str(r#"default = "pi""#).unwrap();
        assert_eq!(config.default.as_deref(), Some("pi"));
        assert_eq!(config.default_kind(), "pi");
    }

    #[test]
    fn default_kind_falls_back_to_omp_when_unset() {
        let config = AgentsConfig::default();
        assert_eq!(config.default, None);
        assert_eq!(config.default_kind(), "omp");
    }

    #[test]
    fn rejects_an_unknown_default_kind() {
        let err = toml::from_str::<AgentsConfig>(r#"default = "bogus""#).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown canonical agent id `bogus` in agents.default"),
            "unexpected error: {err}"
        );
    }
}
