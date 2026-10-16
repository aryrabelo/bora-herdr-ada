use crate::config::{
    AgentSidebarToken, AgentsSidebarConfig, SidebarTokenStyle, SpaceSidebarToken,
    SpacesSidebarConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedToken {
    pub kind: ResolvedTokenKind,
    pub style: SidebarTokenStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedTokenKind {
    StateIcon,
    StateText(String),
    Machine(String),
    Workspace(String),
    Tab(String),
    Pane(String),
    Agent(String),
    TerminalTitle(String),
    Branch(String),
    GitStatus {
        ahead: usize,
        behind: usize,
    },
    Custom(String),
    /// The agent kind's mark: a glyph drawn in its own colour, both from
    /// `AgentsSidebarConfig::mark_for_agent`.
    AgentMark {
        glyph: String,
        fg: ratatui::style::Color,
    },
}

impl ResolvedTokenKind {
    fn text_value(&self) -> Option<&str> {
        match self {
            Self::StateText(value)
            | Self::Machine(value)
            | Self::Workspace(value)
            | Self::Tab(value)
            | Self::Pane(value)
            | Self::Agent(value)
            | Self::TerminalTitle(value)
            | Self::Branch(value)
            | Self::Custom(value) => Some(value),
            Self::StateIcon | Self::GitStatus { .. } | Self::AgentMark { .. } => None,
        }
    }
}

impl ResolvedToken {
    fn new(kind: ResolvedTokenKind, style: SidebarTokenStyle) -> Self {
        Self { kind, style }
    }

    #[cfg(test)]
    pub(super) fn unstyled(kind: ResolvedTokenKind) -> Self {
        Self::new(kind, SidebarTokenStyle::default())
    }
}

pub(crate) struct AgentTokenContext<'a> {
    pub(crate) machine: Option<&'a str>,
    pub(crate) workspace: &'a str,
    pub(crate) tab: Option<&'a str>,
    pub(crate) pane: Option<&'a str>,
    pub(crate) agent_label: Option<&'a str>,
    pub(crate) terminal_title: Option<&'a str>,
    pub(crate) terminal_title_stripped: Option<&'a str>,
    pub(crate) canonical_agent: Option<crate::detect::Agent>,
    pub(crate) tokens: &'a std::collections::HashMap<String, String>,
    /// The live theme palette `agent_mark` resolves a built-in colour against
    /// and checks a configured one for red/green.
    pub(crate) palette: &'a crate::app::state::Palette,
}

pub(crate) fn agent_rows(
    config: &AgentsSidebarConfig,
    context: AgentTokenContext<'_>,
    state_text: &str,
) -> Vec<Vec<ResolvedToken>> {
    config
        .rows_for_agent(context.canonical_agent)
        .iter()
        .filter_map(|row| {
            let resolved = row
                .iter()
                .filter_map(|configured| {
                    let (token, style) = configured.parts();
                    let kind = match token {
                        AgentSidebarToken::StateIcon => Some(ResolvedTokenKind::StateIcon),
                        AgentSidebarToken::StateText => {
                            Some(ResolvedTokenKind::StateText(state_text.to_string()))
                        }
                        AgentSidebarToken::Machine => context
                            .machine
                            .map(|value| ResolvedTokenKind::Machine(value.to_string())),
                        AgentSidebarToken::Workspace => {
                            Some(ResolvedTokenKind::Workspace(context.workspace.to_string()))
                        }
                        AgentSidebarToken::Tab => context
                            .tab
                            .map(|value| ResolvedTokenKind::Tab(value.to_string())),
                        AgentSidebarToken::Pane => context
                            .pane
                            .map(|value| ResolvedTokenKind::Pane(value.to_string())),
                        AgentSidebarToken::Agent => context
                            .agent_label
                            .map(|value| ResolvedTokenKind::Agent(value.to_string())),
                        AgentSidebarToken::TerminalTitle => context
                            .terminal_title
                            .map(|value| ResolvedTokenKind::TerminalTitle(value.to_string())),
                        AgentSidebarToken::TerminalTitleStripped => context
                            .terminal_title_stripped
                            .map(|value| ResolvedTokenKind::TerminalTitle(value.to_string())),
                        AgentSidebarToken::Custom(name) => context
                            .tokens
                            .get(name)
                            .cloned()
                            .map(ResolvedTokenKind::Custom),
                        AgentSidebarToken::AgentMark => context
                            .canonical_agent
                            .and_then(|agent| config.mark_for_agent(agent, context.palette))
                            .map(|(glyph, fg)| ResolvedTokenKind::AgentMark {
                                glyph: glyph.to_string(),
                                fg,
                            }),
                        AgentSidebarToken::Styled { .. } => None,
                    }?;
                    let style = kind
                        .text_value()
                        .map_or(Some(style), |value| configured.style_for_value(value))?;
                    Some(ResolvedToken::new(kind, style))
                })
                .collect::<Vec<_>>();
            (!resolved.is_empty()).then_some(resolved)
        })
        .collect()
}

pub(crate) struct SpaceTokenContext<'a> {
    pub(crate) workspace: &'a str,
    pub(crate) branch: Option<&'a str>,
    pub(crate) state_text: &'a str,
    pub(crate) ahead_behind: Option<(usize, usize)>,
    pub(crate) tokens: &'a std::collections::HashMap<String, String>,
    pub(crate) suppress_git_details: bool,
}

pub(crate) fn space_rows(
    config: &SpacesSidebarConfig,
    context: SpaceTokenContext<'_>,
) -> Vec<Vec<ResolvedToken>> {
    config
        .rows
        .iter()
        .filter_map(|row| {
            let resolved = row
                .iter()
                .filter_map(|configured| {
                    let (token, style) = configured.parts();
                    let kind = match token {
                        SpaceSidebarToken::StateIcon => Some(ResolvedTokenKind::StateIcon),
                        SpaceSidebarToken::StateText => {
                            Some(ResolvedTokenKind::StateText(context.state_text.to_string()))
                        }
                        SpaceSidebarToken::Workspace => {
                            Some(ResolvedTokenKind::Workspace(context.workspace.to_string()))
                        }
                        SpaceSidebarToken::Branch if !context.suppress_git_details => context
                            .branch
                            .map(|branch| ResolvedTokenKind::Branch(branch.to_string())),
                        SpaceSidebarToken::Branch => None,
                        SpaceSidebarToken::GitStatus if !context.suppress_git_details => context
                            .ahead_behind
                            .filter(|(ahead, behind)| *ahead > 0 || *behind > 0)
                            .map(|(ahead, behind)| ResolvedTokenKind::GitStatus { ahead, behind }),
                        SpaceSidebarToken::GitStatus => None,
                        SpaceSidebarToken::Custom(name) => context
                            .tokens
                            .get(name)
                            .cloned()
                            .map(ResolvedTokenKind::Custom),
                        SpaceSidebarToken::Styled { .. } => None,
                    }?;
                    let style = kind
                        .text_value()
                        .map_or(Some(style), |value| configured.style_for_value(value))?;
                    Some(ResolvedToken::new(kind, style))
                })
                .collect::<Vec<_>>();
            (!resolved.is_empty()).then_some(resolved)
        })
        .collect()
}

pub(crate) fn separator(previous: &ResolvedToken, current: &ResolvedToken) -> &'static str {
    if matches!(
        previous.kind,
        ResolvedTokenKind::StateIcon | ResolvedTokenKind::AgentMark { .. }
    ) || matches!(current.kind, ResolvedTokenKind::GitStatus { .. })
    {
        " "
    } else {
        " · "
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentSidebarToken, SpaceSidebarToken};

    struct Entry {
        workspace: String,
        tab: Option<String>,
        pane: Option<String>,
        agent_label: Option<String>,
        terminal_title: Option<String>,
        terminal_title_stripped: Option<String>,
        canonical_agent: Option<crate::detect::Agent>,
        tokens: std::collections::HashMap<String, String>,
        palette: crate::app::state::Palette,
    }

    fn entry() -> Entry {
        Entry {
            workspace: "repo".into(),
            tab: None,
            pane: None,
            agent_label: Some("pi".into()),
            terminal_title: None,
            terminal_title_stripped: None,
            canonical_agent: Some(crate::detect::Agent::Pi),
            tokens: std::collections::HashMap::new(),
            palette: crate::app::state::Palette::catppuccin(),
        }
    }

    fn context(entry: &Entry) -> AgentTokenContext<'_> {
        AgentTokenContext {
            machine: None,
            workspace: &entry.workspace,
            tab: entry.tab.as_deref(),
            pane: entry.pane.as_deref(),
            agent_label: entry.agent_label.as_deref(),
            terminal_title: entry.terminal_title.as_deref(),
            terminal_title_stripped: entry.terminal_title_stripped.as_deref(),
            canonical_agent: entry.canonical_agent,
            tokens: &entry.tokens,
            palette: &entry.palette,
        }
    }

    #[test]
    fn conditional_styles_merge_first_match_and_keep_missing_values_absent() {
        let config: AgentsSidebarConfig = toml::from_str(r##"
rows = [["state_icon", { token = "machine", fg = "#fff", bold = true, dim = true, rules = [{ equals = "Local", fg = "#f00", bold = false }, { contains = "Loc", fg = "#0f0", dim = false }] }], [{ token = "$missing", rules = [{ equals = "", bold = true }] }]]
"##).unwrap();
        let entry = entry();
        for (machine, color, bold, dim) in [
            ("Local", (255, 0, 0), false, true),
            ("Localhost", (0, 255, 0), true, false),
            ("local", (255, 255, 255), true, true),
        ] {
            let mut context = context(&entry);
            context.machine = Some(machine);
            let rows = agent_rows(&config, context, "working");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0][0],
                ResolvedToken::unstyled(ResolvedTokenKind::StateIcon)
            );
            let token = &rows[0][1];
            assert_eq!(token.kind, ResolvedTokenKind::Machine(machine.into()));
            assert_eq!(
                token.style.fg.unwrap().ratatui(),
                ratatui::style::Color::Rgb(color.0, color.1, color.2)
            );
            assert_eq!(token.style.bold, Some(bold));
            assert_eq!(token.style.dim, Some(dim));
        }
        assert_eq!(agent_rows(&config, context(&entry), "working")[0].len(), 1);
    }

    #[test]
    fn conditional_style_survives_truncation_and_removes_theme_modifiers() {
        use ratatui::style::{Color, Modifier, Style};
        let config: AgentsSidebarConfig = toml::from_str(r##"
rows = [[{ token = "workspace", rules = [{ equals = "long-workspace-name", fg = "#f00", bold = false, dim = false }] }]]
"##).unwrap();
        let mut entry = entry();
        entry.workspace = "long-workspace-name".into();
        let rows = agent_rows(&config, context(&entry), "working");
        let theme = Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD | Modifier::DIM);
        for width in [4, 40] {
            let spans = super::super::resolved_token_spans(
                &rows[0],
                ("*", theme),
                theme,
                theme,
                theme,
                theme,
                &super::super::Palette::catppuccin(),
                width,
            );
            assert_eq!(spans.len(), 1);
            assert!(super::super::display_width(&spans[0].content) <= width);
            assert_eq!(spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
            assert!(!spans[0]
                .style
                .add_modifier
                .intersects(Modifier::BOLD | Modifier::DIM));
            assert!(spans[0]
                .style
                .sub_modifier
                .contains(Modifier::BOLD | Modifier::DIM));
        }
    }

    #[test]
    fn custom_numeric_rules_resolve_in_agent_overrides_and_space_rows() {
        let config: crate::config::SidebarConfig = toml::from_str(
            r#"
[agents]
rows = [["workspace"]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ gt = 80, bold = true }, { gt = 50, dim = true }] }]]
[spaces]
rows = [[{ token = "$load", rules = [{ lt = 50, dim = true }] }]]
"#,
        )
        .unwrap();
        let mut entry = entry();
        for (value, bold, dim) in [
            ("90", Some(true), None),
            ("60", None, Some(true)),
            ("20", None, None),
            ("90%", None, None),
        ] {
            entry.tokens.insert("load".into(), value.into());
            let rows = agent_rows(&config.agents, context(&entry), "working");
            assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom(value.into()));
            assert_eq!(rows[0][0].style.bold, bold);
            assert_eq!(rows[0][0].style.dim, dim);
            let spaces = space_rows(
                &config.spaces,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "working",
                    ahead_behind: None,
                    suppress_git_details: false,
                    tokens: &entry.tokens,
                },
            );
            assert_eq!(spaces[0][0].style.dim, (value == "20").then_some(true));
        }
    }

    #[test]
    fn conditional_hide_removes_tokens_and_empty_rows() {
        let config: crate::config::SidebarConfig = toml::from_str(
            r##"
[agents]
rows = [[{ token = "machine", fg = "#61afef", rules = [{ equals = "Local", hide = true }] }, "agent"]]
[agents.rows_by_agent]
pi = [[{ token = "$load", rules = [{ lt = 50, hide = true }] }], ["agent"]]
[spaces]
rows = [[{ token = "$load", rules = [{ lt = 50, hide = true }] }], ["workspace"]]
"##,
        ).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("hide = true"));
        let config: crate::config::SidebarConfig = toml::from_str(&encoded).unwrap();
        let mut entry = entry();
        entry.canonical_agent = None;
        for (machine, count) in [("Local", 1), ("Remote", 2)] {
            let mut ctx = context(&entry);
            ctx.machine = Some(machine);
            let rows = agent_rows(&config.agents, ctx, "working");
            assert_eq!(rows[0].len(), count);
            assert_eq!(
                rows[0].last().unwrap().kind,
                ResolvedTokenKind::Agent("pi".into())
            );
        }
        entry.canonical_agent = Some(crate::detect::Agent::Pi);
        for (value, count) in [("20", 1), ("90", 2)] {
            entry.tokens.insert("load".into(), value.into());
            assert_eq!(
                agent_rows(&config.agents, context(&entry), "working").len(),
                count
            );
            let rows = space_rows(
                &config.spaces,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "working",
                    ahead_behind: None,
                    suppress_git_details: false,
                    tokens: &entry.tokens,
                },
            );
            assert_eq!(rows.len(), count);
        }
    }

    #[test]
    fn conditional_hide_preserves_first_match_wins() {
        for first in ["hide = false", "bold = true"] {
            let config: AgentsSidebarConfig = toml::from_str(&format!(
                "rows = [[{{ token = 'agent', rules = [{{ equals = 'pi', {first} }}, {{ contains = '', hide = true }}] }}]]"
            )).unwrap();
            let entry = entry();
            let rows = agent_rows(&config, context(&entry), "working");
            assert_eq!(rows[0][0].kind, ResolvedTokenKind::Agent("pi".into()));
        }
    }

    #[test]
    fn missing_custom_tokens_elide_rows_and_separators() {
        let entry = entry();
        let config = AgentsSidebarConfig {
            rows: vec![
                vec![
                    AgentSidebarToken::StateIcon,
                    AgentSidebarToken::Custom("missing".into()),
                ],
                vec![AgentSidebarToken::Custom("missing".into())],
                vec![AgentSidebarToken::Agent],
            ],
            ..Default::default()
        };

        let rows = agent_rows(&config, context(&entry), "working");

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            vec![ResolvedToken::unstyled(ResolvedTokenKind::StateIcon)]
        );
        assert_eq!(
            rows[1],
            vec![ResolvedToken::unstyled(ResolvedTokenKind::Agent(
                "pi".into()
            ))]
        );
    }

    #[test]
    fn machine_token_only_resolves_for_multi_machine_rows() {
        let entry = entry();
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::Machine,
                AgentSidebarToken::Workspace,
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                "repo".into()
            ))]]
        );

        let mut remote_context = context(&entry);
        remote_context.machine = Some("Build");
        assert_eq!(
            agent_rows(&config, remote_context, "working"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::Machine("Build".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Workspace("repo".into())),
            ]]
        );
    }

    #[test]
    fn state_text_and_arbitrary_values_are_independent_tokens() {
        let mut entry = entry();
        entry
            .tokens
            .insert("summary".into(), "reviewing auth".into());
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::StateText,
                AgentSidebarToken::Custom("summary".into()),
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "deep in the mines"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::StateText("deep in the mines".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Custom("reviewing auth".into())),
            ]]
        );
    }

    #[test]
    fn terminal_title_builtins_are_distinct_from_custom_tokens() {
        let mut entry = entry();
        entry.terminal_title = Some("⠋ raw title".into());
        entry.terminal_title_stripped = Some("raw title".into());
        entry
            .tokens
            .insert("terminal_title".into(), "custom title".into());
        let config = AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::TerminalTitle,
                AgentSidebarToken::TerminalTitleStripped,
                AgentSidebarToken::Custom("terminal_title".into()),
            ]],
            ..Default::default()
        };

        assert_eq!(
            agent_rows(&config, context(&entry), "working"),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::TerminalTitle("⠋ raw title".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::TerminalTitle("raw title".into())),
                ResolvedToken::unstyled(ResolvedTokenKind::Custom("custom title".into())),
            ]]
        );
    }

    #[test]
    fn known_agent_override_replaces_default_rows() {
        let mut config = AgentsSidebarConfig {
            rows: vec![vec![AgentSidebarToken::Workspace]],
            ..Default::default()
        };
        config
            .rows_by_agent
            .insert("pi".into(), vec![vec![AgentSidebarToken::Agent]]);
        let mut pi = entry();
        pi.agent_label = Some("renamed pi".into());

        assert_eq!(
            agent_rows(&config, context(&pi), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Agent(
                "renamed pi".into()
            ))]]
        );

        pi.canonical_agent = None;
        assert_eq!(
            agent_rows(&config, context(&pi), "working"),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Workspace(
                "repo".into()
            ))]]
        );
    }

    #[test]
    fn grouped_children_suppress_all_builtin_git_details() {
        let config = SpacesSidebarConfig::default();

        assert_eq!(
            space_rows(
                &config,
                SpaceTokenContext {
                    workspace: "feature",
                    branch: Some("worktree/feature"),
                    state_text: "idle",
                    ahead_behind: Some((2, 1)),
                    tokens: &std::collections::HashMap::new(),
                    suppress_git_details: true,
                },
            ),
            vec![vec![
                ResolvedToken::unstyled(ResolvedTokenKind::StateIcon),
                ResolvedToken::unstyled(ResolvedTokenKind::Workspace("feature".into())),
            ]]
        );
    }

    #[test]
    fn workspace_custom_token_can_replace_git_specific_details() {
        let tokens = std::collections::HashMap::from([("jj_status".into(), "2 changes".into())]);
        let config = SpacesSidebarConfig {
            rows: vec![vec![SpaceSidebarToken::Custom("jj_status".into())]],
            ..Default::default()
        };

        assert_eq!(
            space_rows(
                &config,
                SpaceTokenContext {
                    workspace: "repo",
                    branch: None,
                    state_text: "idle",
                    ahead_behind: None,
                    tokens: &tokens,
                    suppress_git_details: false,
                },
            ),
            vec![vec![ResolvedToken::unstyled(ResolvedTokenKind::Custom(
                "2 changes".into()
            ))]]
        );
    }

    #[test]
    fn agent_mark_resolves_glyph_and_color_by_canonical_id() {
        use ratatui::style::Color;
        let config: AgentsSidebarConfig = toml::from_str(
            r##"
rows = [[{ token = "agent_mark", bold = true }, "agent"]]
[marks]
claude = { glyph = "C", fg = "#89b4fa" }
codex = { glyph = "X", fg = "#f38ba8" }
"##,
        )
        .unwrap();
        let mut entry = entry();
        let palette = entry.palette.clone();
        let mark = |entry: &Entry| agent_rows(&config, context(entry), "working")[0][0].clone();

        entry.canonical_agent = Some(crate::detect::Agent::Claude);
        let claude = mark(&entry);
        assert_eq!(
            claude.kind,
            ResolvedTokenKind::AgentMark {
                glyph: "C".into(),
                fg: Color::Rgb(137, 180, 250),
            }
        );
        assert_eq!(claude.style.bold, Some(true));

        entry.canonical_agent = Some(crate::detect::Agent::Pi);
        assert_eq!(
            mark(&entry).kind,
            ResolvedTokenKind::AgentMark {
                glyph: "\u{F03FF}".into(),
                fg: palette.mauve,
            },
            "an id without a user entry draws its built-in mark"
        );

        entry.canonical_agent = Some(crate::detect::Agent::Codex);
        assert_eq!(
            mark(&entry).kind,
            ResolvedTokenKind::AgentMark {
                glyph: "\u{EC81}".into(),
                fg: palette.text,
            },
            "a red fg is refused and the built-in mark draws instead"
        );

        entry.palette = crate::app::state::Palette::terminal();
        entry.canonical_agent = Some(crate::detect::Agent::Claude);
        assert_eq!(
            mark(&entry).kind,
            ResolvedTokenKind::AgentMark {
                glyph: "C".into(),
                fg: Color::Rgb(137, 180, 250),
            }
        );
        entry.canonical_agent = Some(crate::detect::Agent::Gemini);
        assert_eq!(
            mark(&entry).kind,
            ResolvedTokenKind::AgentMark {
                glyph: "\u{F0AE2}".into(),
                fg: Color::Blue,
            }
        );
    }

    #[test]
    fn agent_mark_without_an_entry_renders_nothing_and_takes_zero_width() {
        use ratatui::style::Style;
        let marked: AgentsSidebarConfig =
            toml::from_str(r#"rows = [["state_icon", "agent_mark", "agent"], ["agent_mark"]]"#)
                .unwrap();
        let plain: AgentsSidebarConfig =
            toml::from_str(r#"rows = [["state_icon", "agent"]]"#).unwrap();
        let render = |config: &AgentsSidebarConfig, entry: &Entry| {
            let rows = agent_rows(config, context(entry), "working");
            let spans = super::super::resolved_token_spans(
                &rows[0],
                ("*", Style::default()),
                Style::default(),
                Style::default(),
                Style::default(),
                Style::default(),
                &entry.palette,
                40,
            );
            (
                rows,
                spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
            )
        };

        let mut entry = entry();
        entry.agent_label = Some("devin".into());
        for canonical_agent in [Some(crate::detect::Agent::Devin), None] {
            entry.canonical_agent = canonical_agent;
            let (rows, text) = render(&marked, &entry);
            assert_eq!(
                rows,
                vec![vec![
                    ResolvedToken::unstyled(ResolvedTokenKind::StateIcon),
                    ResolvedToken::unstyled(ResolvedTokenKind::Agent("devin".into())),
                ]],
                "no mark token, and a row holding only the mark disappears"
            );
            assert_eq!(text, "* devin");
            assert_eq!(text, render(&plain, &entry).1, "no stray space");
        }

        entry.canonical_agent = Some(crate::detect::Agent::Pi);
        let (rows, text) = render(&marked, &entry);
        assert_eq!(rows.len(), 2);
        assert_eq!(text, "* \u{F03FF} devin");
    }

    /// cubic P2 on aryrabelo/bora-herdr-ada#57: even a programmatic occurrence
    /// `fg` (config refuses one) cannot repaint the mark; bold/dim still apply.
    #[test]
    fn agent_mark_occurrence_fg_never_overrides_the_mark_color() {
        use ratatui::style::{Modifier, Style};
        let palette = crate::app::state::Palette::catppuccin();
        let mark = ResolvedToken::new(
            ResolvedTokenKind::AgentMark {
                glyph: "\u{EC82}".into(),
                fg: palette.peach,
            },
            SidebarTokenStyle {
                fg: Some(crate::config::SidebarTokenColor::from_palette(palette.red)),
                bold: Some(true),
                dim: Some(true),
            },
        );
        let spans = super::super::resolved_token_spans(
            &[mark],
            ("*", Style::default()),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            &palette,
            10,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "\u{EC82}");
        assert_eq!(spans[0].style.fg, Some(palette.peach));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    /// A built-in mark needs no `[marks]` entry: its colour is the theme's.
    #[test]
    fn agent_mark_builtin_draws_in_its_theme_color_without_a_marks_entry() {
        let config: AgentsSidebarConfig = toml::from_str(r#"rows = [["agent_mark"]]"#).unwrap();
        assert!(config.marks.is_empty());
        let mut entry = entry();
        entry.canonical_agent = Some(crate::detect::Agent::Claude);
        for palette in [
            crate::app::state::Palette::catppuccin(),
            crate::app::state::Palette::terminal(),
        ] {
            entry.palette = palette.clone();
            assert_eq!(
                agent_rows(&config, context(&entry), "working")[0][0].kind,
                ResolvedTokenKind::AgentMark {
                    glyph: "\u{EC82}".into(),
                    fg: palette.peach,
                }
            );
        }
        assert_eq!(
            crate::app::state::Palette::terminal().peach,
            ratatui::style::Color::Yellow
        );
    }
}
