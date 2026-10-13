mod rules;

pub use rules::SidebarTokenRule;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::app::state::Palette;
use crate::detect::Agent;

const MAX_SIDEBAR_ROWS: usize = 16;
const MAX_SIDEBAR_TOKENS_PER_ROW: usize = 16;
const DEFAULT_SIDEBAR_ROW_GAP: u16 = 0;

fn deserialize_sidebar_rows<'de, D, T>(deserializer: D) -> Result<Vec<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let rows = Vec::<Vec<T>>::deserialize(deserializer)?;
    validate_sidebar_rows(&rows).map_err(serde::de::Error::custom)?;
    Ok(rows)
}

fn validate_sidebar_rows<T>(rows: &[Vec<T>]) -> Result<(), String> {
    if rows.len() > MAX_SIDEBAR_ROWS {
        return Err(format!(
            "sidebar layouts may contain at most {MAX_SIDEBAR_ROWS} rows"
        ));
    }
    if rows
        .iter()
        .any(|row| row.len() > MAX_SIDEBAR_TOKENS_PER_ROW)
    {
        return Err(format!(
            "sidebar rows may contain at most {MAX_SIDEBAR_TOKENS_PER_ROW} tokens"
        ));
    }
    Ok(())
}

/// A sidebar token's foreground. Config only ever spells it `#RGB`/`#RRGGBB`
/// (always `Color::Rgb`); the client's idle-tier recolor (ceo-bora#352)
/// builds one from a theme `Palette` color with `from_palette`, which under
/// the 16-color `terminal` theme is a named color (`Cyan`, `Gray`, `Reset`).
/// Storing the ratatui color keeps both on the one per-token style-patch
/// channel (`apply_token_style` in `src/ui/sidebar.rs`) without changing the
/// TOML shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarTokenColor(ratatui::style::Color);

impl SidebarTokenColor {
    pub(crate) fn from_palette(color: ratatui::style::Color) -> Self {
        Self(color)
    }

    pub(crate) fn ratatui(self) -> ratatui::style::Color {
        self.0
    }

    /// The `#rrggbb` spelling config uses; `None` for a named theme color,
    /// which only `from_palette` (render-time, never written to config) holds.
    fn hex(self) -> Option<String> {
        let ratatui::style::Color::Rgb(r, g, b) = self.0 else {
            return None;
        };
        Some(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

impl Serialize for SidebarTokenColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let Some(hex) = self.hex() else {
            return Err(serde::ser::Error::custom(
                "sidebar token fg has no #RRGGBB form for a named theme color",
            ));
        };
        serializer.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for SidebarTokenColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let hex = value.strip_prefix('#').filter(|hex| {
            hex.is_ascii()
                && matches!(hex.len(), 3 | 6)
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        let Some(hex) = hex else {
            return Err(serde::de::Error::custom(
                "sidebar token fg must be #RGB or #RRGGBB",
            ));
        };
        let (r, g, b) = if hex.len() == 3 {
            let mut digits = hex
                .bytes()
                .map(|byte| char::from(byte).to_digit(16).expect("validated hex digit") as u8 * 17);
            (
                digits.next().expect("three hex digits"),
                digits.next().expect("three hex digits"),
                digits.next().expect("three hex digits"),
            )
        } else {
            (
                u8::from_str_radix(&hex[0..2], 16).expect("validated hex digits"),
                u8::from_str_radix(&hex[2..4], 16).expect("validated hex digits"),
                u8::from_str_radix(&hex[4..6], 16).expect("validated hex digits"),
            )
        };
        Ok(Self(ratatui::style::Color::Rgb(r, g, b)))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SidebarTokenStyle {
    pub fg: Option<SidebarTokenColor>,
    pub bold: Option<bool>,
    pub dim: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSidebarToken {
    StateIcon,
    StateText,
    Machine,
    Workspace,
    Tab,
    Pane,
    Agent,
    TerminalTitle,
    TerminalTitleStripped,
    AgentMark,
    Custom(String),
    Styled {
        token: Box<AgentSidebarToken>,
        style: SidebarTokenStyle,
        rules: Vec<SidebarTokenRule>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceSidebarToken {
    StateIcon,
    StateText,
    Workspace,
    Branch,
    GitStatus,
    Custom(String),
    Styled {
        token: Box<SpaceSidebarToken>,
        style: SidebarTokenStyle,
        rules: Vec<SidebarTokenRule>,
    },
}

impl AgentSidebarToken {
    pub(crate) fn style_for_value(&self, value: &str) -> Option<SidebarTokenStyle> {
        match self {
            Self::Styled { style, rules, .. } => rules::matching_style(rules, *style, value),
            _ => Some(SidebarTokenStyle::default()),
        }
    }

    pub(crate) fn parts(&self) -> (&Self, SidebarTokenStyle) {
        match self {
            Self::Styled { token, style, .. } => (token, *style),
            token => (token, SidebarTokenStyle::default()),
        }
    }
}

impl SpaceSidebarToken {
    pub(crate) fn style_for_value(&self, value: &str) -> Option<SidebarTokenStyle> {
        match self {
            Self::Styled { style, rules, .. } => rules::matching_style(rules, *style, value),
            _ => Some(SidebarTokenStyle::default()),
        }
    }

    pub(crate) fn parts(&self) -> (&Self, SidebarTokenStyle) {
        match self {
            Self::Styled { token, style, .. } => (token, *style),
            token => (token, SidebarTokenStyle::default()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyledSidebarToken {
    token: String,
    #[serde(default)]
    fg: Option<SidebarTokenColor>,
    #[serde(default)]
    bold: Option<bool>,
    #[serde(default)]
    dim: Option<bool>,
    #[serde(default)]
    rules: Vec<SidebarTokenRule>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSidebarToken {
    Plain(String),
    Styled(RawStyledSidebarToken),
}

impl RawSidebarToken {
    fn parts(self) -> Result<(String, Option<SidebarTokenStyle>, Vec<SidebarTokenRule>), String> {
        match self {
            Self::Plain(token) => Ok((token, None, Vec::new())),
            Self::Styled(token) => {
                if token.rules.len() > 16 {
                    return Err("sidebar tokens may contain at most 16 rules".into());
                }
                if !token.rules.is_empty()
                    && matches!(
                        token.token.as_str(),
                        "state_icon" | "git_status" | "agent_mark"
                    )
                {
                    return Err("sidebar rules require a text-valued token".into());
                }
                Ok((
                    token.token,
                    Some(SidebarTokenStyle {
                        fg: token.fg,
                        bold: token.bold,
                        dim: token.dim,
                    }),
                    token.rules,
                ))
            }
        }
    }
}

fn parse_sidebar_token<T>(value: String, builtins: &[(&str, T)]) -> Result<T, String>
where
    T: Clone + From<String>,
{
    if let Some((_, token)) = builtins.iter().find(|(name, _)| *name == value) {
        return Ok(token.clone());
    }
    let Some(name) = value.strip_prefix('$') else {
        return Err(format!(
            "unknown sidebar token `{value}`; custom tokens must start with `$`"
        ));
    };
    if name.is_empty()
        || name.len() > 32
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err(format!("invalid custom sidebar token `{value}`"));
    }
    Ok(T::from(name.to_string()))
}

fn serialize_styled_token<S>(
    name: String,
    style: SidebarTokenStyle,
    rules: &[SidebarTokenRule],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(None)?;
    map.serialize_entry("token", &name)?;
    if let Some(fg) = style.fg {
        map.serialize_entry("fg", &fg)?;
    }
    if let Some(bold) = style.bold {
        map.serialize_entry("bold", &bold)?;
    }
    if let Some(dim) = style.dim {
        map.serialize_entry("dim", &dim)?;
    }
    if !rules.is_empty() {
        map.serialize_entry("rules", rules)?;
    }
    map.end()
}

fn agent_token_name(token: &AgentSidebarToken) -> String {
    match token {
        AgentSidebarToken::StateIcon => "state_icon".into(),
        AgentSidebarToken::StateText => "state_text".into(),
        AgentSidebarToken::Machine => "machine".into(),
        AgentSidebarToken::Workspace => "workspace".into(),
        AgentSidebarToken::Tab => "tab".into(),
        AgentSidebarToken::Pane => "pane".into(),
        AgentSidebarToken::Agent => "agent".into(),
        AgentSidebarToken::TerminalTitle => "terminal_title".into(),
        AgentSidebarToken::TerminalTitleStripped => "terminal_title_stripped".into(),
        AgentSidebarToken::AgentMark => "agent_mark".into(),
        AgentSidebarToken::Custom(name) => format!("${name}"),
        AgentSidebarToken::Styled { token, .. } => agent_token_name(token),
    }
}

fn space_token_name(token: &SpaceSidebarToken) -> String {
    match token {
        SpaceSidebarToken::StateIcon => "state_icon".into(),
        SpaceSidebarToken::StateText => "state_text".into(),
        SpaceSidebarToken::Workspace => "workspace".into(),
        SpaceSidebarToken::Branch => "branch".into(),
        SpaceSidebarToken::GitStatus => "git_status".into(),
        SpaceSidebarToken::Custom(name) => format!("${name}"),
        SpaceSidebarToken::Styled { token, .. } => space_token_name(token),
    }
}

impl Serialize for AgentSidebarToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Styled {
                token,
                style,
                rules,
            } => serialize_styled_token(agent_token_name(token), *style, rules, serializer),
            token => serializer.serialize_str(&agent_token_name(token)),
        }
    }
}

impl From<String> for AgentSidebarToken {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl<'de> Deserialize<'de> for AgentSidebarToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (value, style, rules) = RawSidebarToken::deserialize(deserializer)?
            .parts()
            .map_err(serde::de::Error::custom)?;
        let token = parse_sidebar_token(
            value,
            &[
                ("state_icon", Self::StateIcon),
                ("state_text", Self::StateText),
                ("machine", Self::Machine),
                ("workspace", Self::Workspace),
                ("tab", Self::Tab),
                ("pane", Self::Pane),
                ("agent", Self::Agent),
                ("terminal_title", Self::TerminalTitle),
                ("terminal_title_stripped", Self::TerminalTitleStripped),
                ("agent_mark", Self::AgentMark),
            ],
        )
        .map_err(serde::de::Error::custom)?;
        Ok(style.map_or(token.clone(), |style| Self::Styled {
            token: Box::new(token),
            style,
            rules,
        }))
    }
}

impl Serialize for SpaceSidebarToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Styled {
                token,
                style,
                rules,
            } => serialize_styled_token(space_token_name(token), *style, rules, serializer),
            token => serializer.serialize_str(&space_token_name(token)),
        }
    }
}

impl From<String> for SpaceSidebarToken {
    fn from(value: String) -> Self {
        Self::Custom(value)
    }
}

impl<'de> Deserialize<'de> for SpaceSidebarToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (value, style, rules) = RawSidebarToken::deserialize(deserializer)?
            .parts()
            .map_err(serde::de::Error::custom)?;
        let token = parse_sidebar_token(
            value,
            &[
                ("state_icon", Self::StateIcon),
                ("state_text", Self::StateText),
                ("workspace", Self::Workspace),
                ("branch", Self::Branch),
                ("git_status", Self::GitStatus),
            ],
        )
        .map_err(serde::de::Error::custom)?;
        Ok(style.map_or(token.clone(), |style| Self::Styled {
            token: Box::new(token),
            style,
            rules,
        }))
    }
}

type AgentSidebarRows = Vec<Vec<AgentSidebarToken>>;
type SpaceSidebarRows = Vec<Vec<SpaceSidebarToken>>;

fn deserialize_rows_by_agent<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, AgentSidebarRows>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let rows_by_agent = BTreeMap::<String, AgentSidebarRows>::deserialize(deserializer)?;
    for (id, rows) in &rows_by_agent {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in sidebar rows_by_agent"
            )));
        }
        validate_sidebar_rows(rows).map_err(serde::de::Error::custom)?;
    }
    Ok(rows_by_agent)
}

/// One `[ui.sidebar.agents.marks]` entry: the glyph the `agent_mark` token
/// draws for a canonical agent id, in a strict `#RGB`/`#RRGGBB` foreground
/// (the same syntax as an occurrence style's `fg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMark {
    pub glyph: String,
    pub fg: SidebarTokenColor,
}

/// A built-in `agent_mark`. `fg` picks a theme `Palette` slot, resolved at
/// render time, so a default follows the theme (named colors under the
/// 16-color `terminal` theme). None of the slots is `red` or `green`, which
/// belong to agent states across the whole sidebar (AGENTS.md colour
/// reservation) -- but a theme may map a slot onto its red or green
/// (`rose-pine`'s `blue` IS its `green`), so `mark_for_agent` draws such a
/// mark in `text` instead.
struct DefaultAgentMark {
    id: &'static str,
    glyph: &'static str,
    fg: fn(&Palette) -> ratatui::style::Color,
}

/// Built-in marks, ONLY for glyphs measured present, codepoint by codepoint,
/// in the operator's font file (`JetBrainsMonoNerdFontMono-Regular.ttf`,
/// cmap parsed directly; AGENTS.md glyph-coverage rule). A canonical id with
/// no entry here and none in `[ui.sidebar.agents.marks]` draws nothing.
const DEFAULT_AGENT_MARKS: &[DefaultAgentMark] = &[
    // nf-cod-openai
    DefaultAgentMark {
        id: "codex",
        glyph: "\u{EC81}",
        fg: |palette| palette.text,
    },
    // nf-cod-claude
    DefaultAgentMark {
        id: "claude",
        glyph: "\u{EC82}",
        fg: |palette| palette.peach,
    },
    // nf-cod-copilot
    DefaultAgentMark {
        id: "copilot",
        glyph: "\u{EC1E}",
        fg: |palette| palette.overlay1,
    },
    // nf-md-star_four_points
    DefaultAgentMark {
        id: "gemini",
        glyph: "\u{F0AE2}",
        fg: |palette| palette.blue,
    },
    // nf-md-pi
    DefaultAgentMark {
        id: "pi",
        glyph: "\u{F03FF}",
        fg: |palette| palette.mauve,
    },
    // nf-md-pi: omp is pi lineage; its colour tells the two apart.
    DefaultAgentMark {
        id: "omp",
        glyph: "\u{F03FF}",
        fg: |palette| palette.subtext0,
    },
    // nf-md-cursor_default
    DefaultAgentMark {
        id: "cursor",
        glyph: "\u{F01C0}",
        fg: |palette| palette.text,
    },
];

/// Whether a mark colour is the palette's red or green, which a kind mark may
/// never take (AGENTS.md colour reservation): the one rule behind both the
/// config diagnostic and the render-time refusal.
fn is_reserved_mark_color(fg: ratatui::style::Color, palette: &Palette) -> bool {
    fg == palette.red || fg == palette.green
}

fn deserialize_agent_marks<'de, D>(deserializer: D) -> Result<BTreeMap<String, AgentMark>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let marks = BTreeMap::<String, AgentMark>::deserialize(deserializer)?;
    for (id, mark) in &marks {
        if crate::detect::parse_canonical_agent_label(id).is_none() {
            return Err(serde::de::Error::custom(format!(
                "unknown canonical agent id `{id}` in sidebar marks"
            )));
        }
        if mark.glyph.is_empty() || mark.glyph.chars().any(char::is_control) {
            return Err(serde::de::Error::custom(format!(
                "sidebar mark glyph for `{id}` must be non-empty with no control characters"
            )));
        }
    }
    Ok(marks)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentsSidebarConfig {
    #[serde(deserialize_with = "deserialize_sidebar_rows")]
    pub rows: AgentSidebarRows,
    #[serde(default, deserialize_with = "deserialize_rows_by_agent")]
    pub rows_by_agent: BTreeMap<String, AgentSidebarRows>,
    pub row_gap: u16,
    #[serde(default, deserialize_with = "deserialize_agent_marks")]
    pub marks: BTreeMap<String, AgentMark>,
    pub title_by_status: bool,
}

impl AgentsSidebarConfig {
    pub(crate) fn rows_for_agent(&self, agent: Option<Agent>) -> &AgentSidebarRows {
        agent
            .and_then(|agent| self.rows_by_agent.get(crate::detect::agent_label(agent)))
            .unwrap_or(&self.rows)
    }

    /// The `agent_mark` glyph and colour for one canonical agent: its
    /// `[ui.sidebar.agents.marks]` entry unless that `fg` is the palette's
    /// red or green (refused; `reserved_mark_color_diagnostics` reports it),
    /// else the built-in default -- in `text` when its slot is this palette's
    /// red or green. `None` draws nothing.
    pub(crate) fn mark_for_agent(
        &self,
        agent: Agent,
        palette: &Palette,
    ) -> Option<(&str, ratatui::style::Color)> {
        let id = crate::detect::agent_label(agent);
        self.marks
            .get(id)
            .map(|mark| (mark.glyph.as_str(), mark.fg.ratatui()))
            .filter(|(_, fg)| !is_reserved_mark_color(*fg, palette))
            .or_else(|| {
                let mark = DEFAULT_AGENT_MARKS.iter().find(|mark| mark.id == id)?;
                let fg = (mark.fg)(palette);
                if is_reserved_mark_color(fg, palette) {
                    Some((mark.glyph, palette.text))
                } else {
                    Some((mark.glyph, fg))
                }
            })
    }

    /// One config diagnostic per `[ui.sidebar.agents.marks]` entry whose
    /// `fg` is the red or green of any palette the theme config can resolve
    /// to. The render path refuses the same entry (`mark_for_agent`).
    pub(crate) fn reserved_mark_color_diagnostics(&self, palettes: &[Palette]) -> Vec<String> {
        self.marks
            .iter()
            .filter_map(|(id, mark)| {
                let fg = mark.fg.ratatui();
                let reserved = palettes.iter().find_map(|palette| {
                    if fg == palette.red {
                        Some("red")
                    } else if fg == palette.green {
                        Some("green")
                    } else {
                        None
                    }
                })?;
                Some(format!(
                    "ui.sidebar.agents.marks.{id}.fg ({}) is the theme's {reserved}, reserved for agent states; ignoring this mark",
                    mark.fg.hex().unwrap_or_default()
                ))
            })
            .collect()
    }
}

impl Default for AgentsSidebarConfig {
    fn default() -> Self {
        Self {
            rows: vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Machine,
                    AgentSidebarToken::Workspace,
                    AgentSidebarToken::Tab,
                ],
                vec![AgentSidebarToken::Agent],
            ],
            rows_by_agent: BTreeMap::new(),
            row_gap: DEFAULT_SIDEBAR_ROW_GAP,
            marks: BTreeMap::new(),
            title_by_status: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SpacesSidebarConfig {
    #[serde(deserialize_with = "deserialize_sidebar_rows")]
    pub rows: SpaceSidebarRows,
    pub row_gap: u16,
}

impl Default for SpacesSidebarConfig {
    fn default() -> Self {
        Self {
            rows: vec![
                vec![SpaceSidebarToken::StateIcon, SpaceSidebarToken::Workspace],
                vec![SpaceSidebarToken::Branch, SpaceSidebarToken::GitStatus],
            ],
            row_gap: DEFAULT_SIDEBAR_ROW_GAP,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SidebarConfig {
    pub agents: AgentsSidebarConfig,
    pub spaces: SpacesSidebarConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_compact_agent_and_existing_space_layouts() {
        let config = SidebarConfig::default();
        assert_eq!(
            config.agents.rows,
            vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Machine,
                    AgentSidebarToken::Workspace,
                    AgentSidebarToken::Tab,
                ],
                vec![AgentSidebarToken::Agent],
            ]
        );
        assert!(config.agents.rows_by_agent.is_empty());
        assert_eq!(config.agents.row_gap, 0);
        assert_eq!(
            config.spaces.rows,
            vec![
                vec![SpaceSidebarToken::StateIcon, SpaceSidebarToken::Workspace],
                vec![SpaceSidebarToken::Branch, SpaceSidebarToken::GitStatus],
            ]
        );
        assert_eq!(config.spaces.row_gap, 0);
    }

    #[test]
    fn parses_builtin_and_arbitrary_custom_tokens() {
        let config: crate::config::Config = toml::from_str(
            r#"
[ui.sidebar.agents]
rows = [["state_icon", "workspace"], ["state_text", "agent", "$summary"], ["terminal_title", "terminal_title_stripped", "$terminal_title"]]
row_gap = 1

[ui.sidebar.agents.rows_by_agent]
claude = [["terminal_title_stripped"], ["agent", "$model"]]

[ui.sidebar.spaces]
rows = [["workspace"], ["$jj_status"]]
row_gap = 3
"#,
        )
        .expect("sidebar token config");

        assert_eq!(
            config.ui.sidebar.agents.rows[1],
            vec![
                AgentSidebarToken::StateText,
                AgentSidebarToken::Agent,
                AgentSidebarToken::Custom("summary".into()),
            ]
        );
        assert_eq!(
            config.ui.sidebar.agents.rows[2],
            vec![
                AgentSidebarToken::TerminalTitle,
                AgentSidebarToken::TerminalTitleStripped,
                AgentSidebarToken::Custom("terminal_title".into()),
            ]
        );
        assert_eq!(
            config.ui.sidebar.agents.rows_by_agent["claude"],
            vec![
                vec![AgentSidebarToken::TerminalTitleStripped],
                vec![
                    AgentSidebarToken::Agent,
                    AgentSidebarToken::Custom("model".into()),
                ],
            ]
        );
        assert_eq!(config.ui.sidebar.agents.row_gap, 1);
        assert_eq!(
            config.ui.sidebar.spaces.rows[1],
            vec![SpaceSidebarToken::Custom("jj_status".into())]
        );
        assert_eq!(config.ui.sidebar.spaces.row_gap, 3);
    }

    #[test]
    fn parses_occurrence_styles_without_changing_plain_tokens() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents]
rows = [[{ token = "workspace", fg = "#abc", bold = false }, "workspace"], [{ token = "$summary", dim = false }]]

[ui.sidebar.agents.rows_by_agent]
claude = [[{ token = "agent", fg = "#112233", bold = true, dim = false }]]

[ui.sidebar.spaces]
rows = [[{ token = "git_status", fg = "#ff00aa" }], [{ token = "$jj", bold = true }]]
"##,
        )
        .unwrap();

        let (token, style) = config.ui.sidebar.agents.rows[0][0].parts();
        assert_eq!(token, &AgentSidebarToken::Workspace);
        assert_eq!(style.bold, Some(false));
        assert_eq!(
            style.fg.unwrap().ratatui(),
            ratatui::style::Color::Rgb(0xaa, 0xbb, 0xcc)
        );
        assert_eq!(
            config.ui.sidebar.agents.rows[0][1],
            AgentSidebarToken::Workspace
        );

        let (token, style) = config.ui.sidebar.agents.rows_by_agent["claude"][0][0].parts();
        assert_eq!(token, &AgentSidebarToken::Agent);
        assert_eq!(style.bold, Some(true));
        assert_eq!(style.dim, Some(false));

        let (token, style) = config.ui.sidebar.spaces.rows[0][0].parts();
        assert_eq!(token, &SpaceSidebarToken::GitStatus);
        assert_eq!(
            style.fg.unwrap().ratatui(),
            ratatui::style::Color::Rgb(0xff, 0x00, 0xaa)
        );
        let (token, style) = config.ui.sidebar.spaces.rows[1][0].parts();
        assert_eq!(token, &SpaceSidebarToken::Custom("jj".into()));
        assert_eq!(style.bold, Some(true));
    }

    #[test]
    fn conditional_sidebar_rules_round_trip() {
        let input = r##"
[agents]
rows = [[{ token = "machine", fg = "#fff", rules = [{ equals = "Local", fg = "#f00" }, { starts_with = "fed", ignore_case = true, bold = true }] }]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ gt = 80, dim = false }, { lt = 20.5, dim = true }] }]]
[spaces]
rows = [[{ token = "$status", rules = [{ contains = "error", bold = true }] }]]
"##;
        let config: SidebarConfig = toml::from_str(input).expect("conditional sidebar config");
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("rules"));
        assert_eq!(toml::from_str::<SidebarConfig>(&encoded).unwrap(), config);
    }

    #[test]
    fn conditional_sidebar_rules_reject_invalid_conditions_and_nontext_tokens() {
        for rule in [
            "{ bold = true }",
            "{ equals = 'x', contains = 'x' }",
            "{ regex = 'x' }",
            "{ gt = '80' }",
            "{ equals = 80 }",
            "{ gt = nan }",
            "{ lt = inf }",
            "{ gt = 80, ignore_case = false }",
            "{ equals = 'x', underline = true }",
            "{ equals = 'x', fg = 'red' }",
        ] {
            let input = format!("[agents]\nrows = [[{{ token = 'machine', rules = [{rule}] }}]]");
            assert!(toml::from_str::<SidebarConfig>(&input).is_err(), "{rule}");
        }
        for (section, token) in [
            ("agents", "state_icon"),
            ("spaces", "state_icon"),
            ("spaces", "git_status"),
        ] {
            let input = format!(
                "[{section}]\nrows = [[{{ token = '{token}', rules = [{{ equals = 'x' }}] }}]]"
            );
            assert!(toml::from_str::<SidebarConfig>(&input).is_err());
        }
        for count in [16, 17] {
            let rules = std::iter::repeat_n("{ equals = 'x' }", count)
                .collect::<Vec<_>>()
                .join(",");
            let input = format!("[agents]\nrows = [[{{ token = 'machine', rules = [{rules}] }}]]");
            assert_eq!(toml::from_str::<SidebarConfig>(&input).is_ok(), count == 16);
        }
    }

    #[test]
    fn rejects_invalid_occurrence_styles() {
        for entry in [
            r##"{ token = "workspace", fg = "red" }"##,
            r##"{ token = "workspace", fg = "#abcd" }"##,
            r##"{ token = "workspace", underline = true }"##,
        ] {
            let input = format!("[ui.sidebar.agents]\nrows = [[{entry}]]\n");
            assert!(
                toml::from_str::<crate::config::Config>(&input).is_err(),
                "accepted {entry}"
            );
        }
    }

    #[test]
    fn rejects_unknown_bare_and_malformed_custom_tokens() {
        for token in ["summary", "$", "$bad.name"] {
            let input = format!("[ui.sidebar.agents]\\nrows = [[\"{token}\"]]\\n");
            assert!(toml::from_str::<crate::config::Config>(&input).is_err());
        }
    }

    #[test]
    fn rejects_oversized_sidebar_layouts() {
        let too_many_rows = std::iter::repeat_n("[\"agent\"]", MAX_SIDEBAR_ROWS + 1)
            .collect::<Vec<_>>()
            .join(",");
        let input = format!("[ui.sidebar.agents]\nrows = [{too_many_rows}]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());

        let too_many_tokens = std::iter::repeat_n("\"workspace\"", MAX_SIDEBAR_TOKENS_PER_ROW + 1)
            .collect::<Vec<_>>()
            .join(",");
        let input = format!("[ui.sidebar.spaces]\nrows = [[{too_many_tokens}]]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());

        let input = format!("[ui.sidebar.agents.rows_by_agent]\nclaude = [{too_many_rows}]\n");
        assert!(toml::from_str::<crate::config::Config>(&input).is_err());
    }

    #[test]
    fn accepts_every_canonical_agent_override_key() {
        let agents = Agent::ALL;
        let entries = agents
            .iter()
            .map(|agent| format!("{} = [[\"agent\"]]", crate::detect::agent_label(*agent)))
            .collect::<Vec<_>>()
            .join("\n");
        let input = format!("[ui.sidebar.agents.rows_by_agent]\n{entries}\n");
        let config: crate::config::Config = toml::from_str(&input).expect("canonical keys");

        assert_eq!(config.ui.sidebar.agents.rows_by_agent.len(), agents.len());
    }

    #[test]
    fn rejects_alias_case_whitespace_and_unknown_override_keys() {
        for key in ["claude-code", "Claude", "' claude '", "unknown"] {
            let input = format!("[ui.sidebar.agents.rows_by_agent]\n{key} = [[\"agent\"]]\n");
            assert!(
                toml::from_str::<crate::config::Config>(&input).is_err(),
                "accepted key {key:?}"
            );
        }
    }

    #[test]
    fn agent_mark_entries_parse_by_canonical_id_and_override_the_builtin() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents]
title_by_status = true

[ui.sidebar.agents.rows_by_agent]
claude = [["agent_mark", "agent"]]

[ui.sidebar.agents.marks]
claude = { glyph = "C", fg = "#89b4fa" }
"##,
        )
        .expect("agent mark config");
        let agents = &config.ui.sidebar.agents;
        let palette = Palette::catppuccin();

        assert!(agents.title_by_status);
        assert_eq!(
            agents.rows_by_agent["claude"],
            vec![vec![AgentSidebarToken::AgentMark, AgentSidebarToken::Agent]]
        );
        assert_eq!(
            agents.marks["claude"],
            AgentMark {
                glyph: "C".into(),
                fg: SidebarTokenColor::from_palette(ratatui::style::Color::Rgb(137, 180, 250)),
            }
        );
        assert_eq!(
            agents.mark_for_agent(Agent::Claude, &palette),
            Some(("C", ratatui::style::Color::Rgb(137, 180, 250)))
        );
        assert_eq!(
            agents.mark_for_agent(Agent::Codex, &palette),
            Some(("\u{EC81}", palette.text)),
            "an id without a user entry keeps its built-in mark"
        );
        assert_eq!(agents.mark_for_agent(Agent::Devin, &palette), None);
        assert_eq!(
            toml::from_str::<SidebarConfig>(&toml::to_string(&config.ui.sidebar).unwrap()).unwrap(),
            config.ui.sidebar
        );

        let defaults = AgentsSidebarConfig::default();
        assert!(!defaults.title_by_status);
        assert!(defaults.marks.is_empty());

        for entry in [
            r##"claude-code = { glyph = "C", fg = "#89b4fa" }"##,
            r##"claude = { glyph = "", fg = "#89b4fa" }"##,
            r##"claude = { glyph = "\u0007", fg = "#89b4fa" }"##,
            r##"claude = { glyph = "C", fg = "blue" }"##,
            r##"claude = { glyph = "C" }"##,
            r##"claude = { glyph = "C", fg = "#89b4fa", bold = true }"##,
        ] {
            let input = format!("[ui.sidebar.agents.marks]\n{entry}\n");
            assert!(
                toml::from_str::<crate::config::Config>(&input).is_err(),
                "accepted {entry}"
            );
        }
        let input =
            "[ui.sidebar.agents]\nrows = [[{ token = 'agent_mark', rules = [{ equals = 'x' }] }]]";
        assert!(toml::from_str::<crate::config::Config>(input).is_err());
    }

    #[test]
    fn agent_mark_red_or_green_fg_is_refused_with_a_config_diagnostic() {
        let palette = Palette::catppuccin();
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents.marks]
claude = { glyph = "C", fg = "#f38ba8" }
codex = { glyph = "X", fg = "#a6e3a1" }
gemini = { glyph = "G", fg = "#89b4fa" }
"##,
        )
        .expect("agent mark config");
        let diagnostics = config.collect_diagnostics();
        let mark_diagnostics = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.starts_with("ui.sidebar.agents.marks."))
            .collect::<Vec<_>>();

        assert_eq!(
            mark_diagnostics,
            vec![
                "ui.sidebar.agents.marks.claude.fg (#f38ba8) is the theme's red, reserved for agent states; ignoring this mark",
                "ui.sidebar.agents.marks.codex.fg (#a6e3a1) is the theme's green, reserved for agent states; ignoring this mark",
            ],
            "{diagnostics:?}"
        );
        let agents = &config.ui.sidebar.agents;
        assert_eq!(
            agents.mark_for_agent(Agent::Claude, &palette),
            Some(("\u{EC82}", palette.peach)),
            "a refused entry falls back to the built-in mark"
        );
        assert_eq!(
            agents.mark_for_agent(Agent::Codex, &palette),
            Some(("\u{EC81}", palette.text))
        );
        assert_eq!(
            agents.mark_for_agent(Agent::Gemini, &palette),
            Some(("G", ratatui::style::Color::Rgb(137, 180, 250)))
        );
    }

    #[test]
    fn agent_mark_builtins_are_canonical_and_never_red_or_green_in_any_theme() {
        for mark in DEFAULT_AGENT_MARKS {
            assert!(
                crate::detect::parse_canonical_agent_label(mark.id).is_some(),
                "{} is not a canonical agent id",
                mark.id
            );
        }
        let config = AgentsSidebarConfig::default();
        for name in crate::config::THEME_NAMES {
            let palette = Palette::from_name(name).expect("built-in theme");
            for agent in Agent::ALL {
                if let Some((_, fg)) = config.mark_for_agent(agent, &palette) {
                    assert!(
                        !is_reserved_mark_color(fg, &palette),
                        "{agent:?} mark is red or green under {name}"
                    );
                }
            }
        }
        let rose_pine = Palette::from_name("rose-pine").expect("rose-pine");
        assert_eq!(rose_pine.blue, rose_pine.green);
        assert_eq!(
            config.mark_for_agent(Agent::Gemini, &rose_pine),
            Some(("\u{F0AE2}", rose_pine.text)),
            "a built-in slot that is the theme's green draws in text"
        );
    }
}
