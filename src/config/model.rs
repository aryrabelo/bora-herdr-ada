use std::{collections::BTreeSet, num::NonZeroUsize};

use crossterm::event::KeyModifiers;
use serde::{de, Deserialize, Deserializer, Serialize};

use super::{
    ActionKeybinds, AgentsConfig, BindingConfig, CommandKeybindConfig, IndexedKeybind, Keybinds,
    SidebarConfig, SoundConfig, TabBarRightEntryConfig, ThemeConfig,
    DEFAULT_MOBILE_WIDTH_THRESHOLD, DEFAULT_MOUSE_SCROLL_LINES, DEFAULT_SCROLLBACK_LIMIT_BYTES,
};

pub const MAX_TOAST_DELAY_SECONDS: u64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannelConfig {
    #[default]
    Stable,
    Preview,
}

impl UpdateChannelConfig {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Preview => "preview",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub channel: UpdateChannelConfig,
    pub version_check: bool,
    pub manifest_check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel: default_update_channel(),
            version_check: true,
            manifest_check: true,
        }
    }
}

fn default_update_channel() -> UpdateChannelConfig {
    default_update_channel_for_build(cfg!(windows), crate::build_info::is_preview())
}

fn default_update_channel_for_build(is_windows: bool, is_preview: bool) -> UpdateChannelConfig {
    if is_windows && is_preview {
        UpdateChannelConfig::Preview
    } else {
        UpdateChannelConfig::Stable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToastDelivery {
    #[default]
    Off,
    Herdr,
    Terminal,
    System,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum ToastHerdrPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ToastClipboardPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentPanelSortConfig {
    #[default]
    #[serde(alias = "workspaces")]
    Spaces,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LegacyAgentPanelScopeConfig {
    Current,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StatusIndicatorStyle {
    #[default]
    Dots,
    Symbols,
}

impl StatusIndicatorStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dots => "dots",
            Self::Symbols => "symbols",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostCursorModeConfig {
    #[default]
    Auto,
    Native,
    Drawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarCollapsedModeConfig {
    #[default]
    Compact,
    Hidden,
}

/// Sidebar workspace view mode. `Flat` shows a freely drag-reorderable list
/// with no grouping at all — repo, channel, and visual groups all dissolve.
/// `Folders` is a flat list too, but honors user-defined `visual_group`
/// folders (and nothing else): no repo auto-grouping, no branch brackets.
/// Both `Flat` and `Folders` render `[ui.sidebar.spaces].rows` templates
/// (ceo-bora#302); `Folders` additionally shows a per-pane status dots
/// strip on the row (`hide_pane_badges` suppresses it there). `Repo`
/// groups workspaces under repo headers and is the historical default.
///
/// Deliberately backward compatible with the retired
/// `group_workspaces_by_repo` boolean: this type's `Deserialize` impl
/// accepts a bare bool (`true` -> `Repo`, `false` -> `Flat`) in addition to
/// the `"flat"`/`"folders"`/`"repo"` strings, and `UiConfig::view_mode`
/// carries `#[serde(alias = "group_workspaces_by_repo")]` so the old key
/// still works. A config that sets BOTH keys at once is a genuine
/// ambiguity and fails to parse rather than silently picking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    Flat,
    Folders,
    #[default]
    Repo,
}

impl ViewMode {
    /// Flat -> Folders -> Repo -> Flat.
    pub fn cycle(self) -> ViewMode {
        match self {
            ViewMode::Flat => ViewMode::Folders,
            ViewMode::Folders => ViewMode::Repo,
            ViewMode::Repo => ViewMode::Flat,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Flat => "flat",
            ViewMode::Folders => "folders",
            ViewMode::Repo => "repo",
        }
    }
}

impl Serialize for ViewMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ViewMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Str(String),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Bool(true) => Ok(ViewMode::Repo),
            Raw::Bool(false) => Ok(ViewMode::Flat),
            Raw::Str(s) => match s.as_str() {
                "flat" => Ok(ViewMode::Flat),
                "folders" => Ok(ViewMode::Folders),
                "repo" => Ok(ViewMode::Repo),
                other => Err(de::Error::unknown_variant(
                    other,
                    &["flat", "folders", "repo"],
                )),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RightClickPassthroughModifierConfig(Option<KeyModifiers>);

impl RightClickPassthroughModifierConfig {
    pub fn modifiers(self) -> Option<KeyModifiers> {
        self.0
    }
}

impl<'de> Deserialize<'de> for RightClickPassthroughModifierConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_right_click_passthrough_modifier(&value)
            .map(Self)
            .ok_or_else(|| {
                de::Error::custom(
                    "right_click_passthrough_modifier must be empty, off, none, disabled, ctrl/control, alt/option, cmd/command/super, meta, hyper, or a + separated combination without shift",
                )
            })
    }
}

fn parse_right_click_passthrough_modifier(value: &str) -> Option<Option<KeyModifiers>> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("disabled")
    {
        return Some(None);
    }

    let mut modifiers = KeyModifiers::empty();
    for token in trimmed.split('+') {
        let token = token.trim().to_ascii_lowercase();
        let modifier = match token.as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "option" => KeyModifiers::ALT,
            "cmd" | "command" | "super" => KeyModifiers::SUPER,
            "meta" => KeyModifiers::META,
            "hyper" => KeyModifiers::HYPER,
            "shift" => return None,
            _ => return None,
        };
        modifiers |= modifier;
    }

    (!modifiers.is_empty()).then_some(Some(modifiers))
}

#[derive(Debug, Clone)]
pub struct ToastConfig {
    pub delivery: ToastDelivery,
    pub delay_seconds: u64,
    pub herdr: HerdrToastConfig,
    pub clipboard: ClipboardToastConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HerdrToastConfig {
    pub position: ToastHerdrPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ClipboardToastConfig {
    pub enabled: bool,
    pub position: ToastClipboardPosition,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NewTerminalCwdConfig {
    #[default]
    Follow,
    Home,
    Current,
    Path(String),
}

impl<'de> Deserialize<'de> for NewTerminalCwdConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim() {
            "" | "follow" => Ok(Self::Follow),
            "home" => Ok(Self::Home),
            "current" => Ok(Self::Current),
            _ => Ok(Self::Path(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellModeConfig {
    #[default]
    Auto,
    Login,
    NonLogin,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Executable used for new interactive panes. Empty means SHELL, then /bin/sh.
    pub default_shell: String,
    /// Startup mode for new interactive pane shells.
    pub shell_mode: ShellModeConfig,
    /// CWD policy for new interactive panes, tabs, and workspaces.
    pub new_cwd: NewTerminalCwdConfig,
    /// Render Kitty graphics in compatible outer terminals. Default: true.
    pub kitty_graphics: Option<bool>,
    /// Rewrap already-painted rows when a pane narrows. Default: true.
    pub reflow_on_resize: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_experimental_kitty_graphics_setting_remains_compatible() {
        let disabled: Config = toml::from_str(
            r#"
[experimental]
kitty_graphics = false
"#,
        )
        .unwrap();
        assert!(!disabled.kitty_graphics_enabled());

        let stable_setting_wins: Config = toml::from_str(
            r#"
[terminal]
kitty_graphics = false

[experimental]
kitty_graphics = true
"#,
        )
        .unwrap();
        assert!(!stable_setting_wins.kitty_graphics_enabled());
    }

    #[test]
    fn experimental_config_parses() {
        let toml = r#"
[experimental]
allow_nested = true
kitty_graphics = true
pane_history = true
switch_ascii_input_source_in_prefix = true
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.experimental.allow_nested);
        assert_eq!(config.experimental.kitty_graphics, Some(true));
        assert!(config.kitty_graphics_enabled());
        assert!(config.experimental.pane_history);
        assert!(config.experimental.switch_ascii_input_source_in_prefix);
    }

    #[test]
    fn advanced_config_parses() {
        let toml = r#"
[advanced]
scrollback_limit_bytes = 12345
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.advanced.scrollback_limit_bytes, 12345);
    }

    #[test]
    fn advanced_legacy_scrollback_lines_alias_parses() {
        let toml = r#"
[advanced]
scrollback_lines = 12345
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.advanced.scrollback_limit_bytes, 12345);
    }
}
