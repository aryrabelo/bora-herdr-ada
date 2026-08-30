use crate::config::{
    Keybinds, NewTerminalCwdConfig, SoundConfig, TabBarPositionConfig, ToastConfig, ToastDelivery,
};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Direction, Rect};
use ratatui::style::Color;

use crate::detect::AgentState;
use crate::layout::{PaneId, PaneInfo, SplitBorder};
use crate::selection::Selection;

pub(crate) type InstalledPluginRegistry =
    std::collections::HashMap<String, crate::api::schema::InstalledPluginInfo>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginPaneRecord {
    pub plugin_id: String,
    pub entrypoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopupPaneState {
    pub pane_id: PaneId,
    pub terminal_id: crate::terminal::TerminalId,
    pub width: Option<crate::popup_size::PopupSize>,
    pub height: Option<crate::popup_size::PopupSize>,
}

/// One on-disk worktree, canonicalized ONCE at refresh time (bora-qdi).
/// `checkout_key` is computed the same way a `Workspace`'s own
/// `GitSpaceMetadata.checkout_key` is (`workspace::git::discovery`'s
/// derivation: canonicalize the checkout path, stringify it) —
/// on the background thread that lists it
/// (`App::start_worktree_inventory_refresh_if_due`, `src/app/runtime.rs`),
/// never on the render path, so `ui::sidebar::project_view` (which must stay
/// I/O-free — "Multiplicative performance paths", AGENTS.md) compares it as
/// a plain string against `Workspace.git_space().checkout_key` with no
/// filesystem call of its own and no chance of drifting from the real key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InventoryWorktree {
    pub checkout_key: String,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_prunable: bool,
}

/// Background `git worktree list` result for one repo (bora-qdi). Mirrors
/// `crate::workspace::RepoOpenPrs`'s shape: the worktrees found on disk plus
/// an `error` so a failed listing renders as a visible failure rather than
/// silently no unopened rows. Written by
/// `App::start_worktree_inventory_refresh_if_due` (`src/app/runtime.rs`);
/// read by `ui::sidebar::project_view` from `AppState::worktree_inventory`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct RepoWorktreeInventory {
    pub worktrees: Vec<InventoryWorktree>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Selection autoscroll types
// ---------------------------------------------------------------------------

/// Direction of automatic scrolling during text selection drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionAutoscrollDirection {
    Up,
    Down,
}

/// State for automatic scrolling during text selection drag.
///
/// When the cursor hovers in the 1-row hot zone at the top or bottom edge
/// of a pane (or outside the pane), this struct captures the direction and
/// last known mouse position so a recurring 30ms tick can continue scrolling
/// and extending the selection even when the mouse is not moving.
#[derive(Clone, Debug)]
pub(crate) struct SelectionAutoscroll {
    pub direction: SelectionAutoscrollDirection,
    pub last_mouse_screen_col: u16,
    pub last_mouse_screen_row: u16,
    pub inner_rect: Rect,
}

#[derive(Clone)]
pub(crate) struct RightClickPassthroughGesture {
    pub pane_info: PaneInfo,
    pub modifiers: KeyModifiers,
}
use crate::terminal_theme::{HostAppearance, TerminalTheme};
use crate::workspace::Workspace;

// ---------------------------------------------------------------------------
// Theme palette — all UI colors in one place, ready for theming
// ---------------------------------------------------------------------------

/// All colors used by the UI. Derived from a base accent color for now,
/// but structured so a full theme system can replace it later.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // all fields defined for theming — some used later
pub struct Palette {
    /// Primary accent (highlight, active borders).
    pub accent: Color,
    /// Background for the tab bar, floating panels, overlays, and modals.
    pub panel_bg: Color,
    /// Optional desktop sidebar background. Reset preserves the terminal background.
    pub sidebar_bg: Color,
    /// Background for the active workspace and focused agent rows.
    pub active_row_bg: Color,
    /// Background for the Navigate-mode cursor row in the sidebar.
    pub selection_bg: Color,
    /// Subtle surface background for selected/focused items.
    pub surface0: Color,
    /// Slightly lighter surface for hover/active states.
    pub surface1: Color,
    /// Very dim surface for separators.
    pub surface_dim: Color,
    /// Muted text (secondary info, numbers).
    pub overlay0: Color,
    /// Slightly brighter overlay text.
    pub overlay1: Color,
    /// Main text color — soft white.
    pub text: Color,
    /// Subdued text (workspace numbers, dim labels).
    pub subtext0: Color,
    /// Branch name / special label color.
    pub mauve: Color,
    /// Done / idle states.
    pub green: Color,
    /// Working / running states.
    pub yellow: Color,
    /// Needs attention / blocked states.
    pub red: Color,
    /// Unseen / done notification accent.
    pub blue: Color,
    /// Notification accent / unseen markers.
    pub teal: Color,
    /// Interrupted / warning states.
    pub peach: Color,
}

impl Palette {
    /// Catppuccin Mocha — the default.
    pub fn catppuccin() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250), // blue
            panel_bg: Color::Rgb(24, 24, 37),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(30, 30, 46),
            selection_bg: Color::Rgb(49, 50, 68),
            surface0: Color::Rgb(49, 50, 68),
            surface1: Color::Rgb(69, 71, 90),
            surface_dim: Color::Rgb(30, 30, 46),
            overlay0: Color::Rgb(108, 112, 134),
            overlay1: Color::Rgb(127, 132, 156),
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            mauve: Color::Rgb(203, 166, 247),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            blue: Color::Rgb(137, 180, 250),
            teal: Color::Rgb(148, 226, 213),
            peach: Color::Rgb(250, 179, 135),
        }
    }

    /// Catppuccin Latte — the light Catppuccin flavor.
    pub fn catppuccin_latte() -> Self {
        Self {
            accent: Color::Rgb(30, 102, 245),
            panel_bg: Color::Rgb(239, 241, 245),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(230, 233, 239),
            selection_bg: Color::Rgb(189, 208, 245),
            surface0: Color::Rgb(204, 208, 218),
            surface1: Color::Rgb(188, 192, 204),
            surface_dim: Color::Rgb(230, 233, 239),
            overlay0: Color::Rgb(156, 160, 176),
            overlay1: Color::Rgb(140, 143, 161),
            text: Color::Rgb(76, 79, 105),
            subtext0: Color::Rgb(108, 111, 133),
            mauve: Color::Rgb(136, 57, 239),
            green: Color::Rgb(64, 160, 43),
            yellow: Color::Rgb(223, 142, 29),
            red: Color::Rgb(210, 15, 57),
            blue: Color::Rgb(30, 102, 245),
            teal: Color::Rgb(23, 146, 153),
            peach: Color::Rgb(254, 100, 11),
        }
    }

    /// Terminal 16-color theme.
    pub fn terminal() -> Self {
        Self {
            accent: Color::Blue,
            panel_bg: Color::Reset,
            sidebar_bg: Color::Reset,
            active_row_bg: Color::DarkGray,
            selection_bg: Color::Reset,
            // Was `Reset`, i.e. identical to `sidebar_bg` — so any row using
            // `surface0` as a "slightly lighter" fill got no fill at all.
            // That became load-bearing when the Project-view header row
            // dropped BOLD in favour of exactly that background. `DarkGray`
            // duplicates `surface1`, which is the least-bad option in a
            // 16-color palette: the two are never asked to distinguish
            // themselves on the same row (`surface1` is the drag-preview
            // fill), whereas a fill that equals the background is always
            // wrong.
            surface0: Color::DarkGray,
            surface1: Color::DarkGray,
            surface_dim: Color::DarkGray,
            overlay0: Color::Gray,
            overlay1: Color::White,
            text: Color::Reset,
            subtext0: Color::Gray,
            // Was `Color::Gray`, identical to `overlay0` above, so every
            // mauve accent — the Project-view header name, the worktree
            // glyph, the merged-PR chip — rendered as ordinary muted text on
            // this theme. `Magenta` is the ANSI purple slot and is otherwise
            // unclaimed here, matching how every neighbour maps to its own
            // slot (green→Green, yellow→Yellow, teal→Cyan).
            mauve: Color::Magenta,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::LightRed,
            blue: Color::Blue,
            teal: Color::Cyan,
            peach: Color::Yellow,
        }
    }

    /// Tokyo Night — blue-purple aesthetic.
    pub fn tokyo_night() -> Self {
        Self {
            accent: Color::Rgb(122, 162, 247), // blue
            panel_bg: Color::Rgb(26, 27, 38),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(35, 38, 54),
            selection_bg: Color::Rgb(45, 54, 80),
            surface0: Color::Rgb(36, 40, 59),
            surface1: Color::Rgb(65, 72, 104),
            surface_dim: Color::Rgb(26, 27, 38),
            overlay0: Color::Rgb(86, 95, 137),
            overlay1: Color::Rgb(105, 113, 150),
            text: Color::Rgb(192, 202, 245),
            subtext0: Color::Rgb(169, 177, 214),
            mauve: Color::Rgb(187, 154, 247),
            green: Color::Rgb(158, 206, 106),
            yellow: Color::Rgb(224, 175, 104),
            red: Color::Rgb(247, 118, 142),
            blue: Color::Rgb(122, 162, 247),
            teal: Color::Rgb(125, 207, 255),
            peach: Color::Rgb(255, 158, 100),
        }
    }

    /// Tokyo Night Day — the light Tokyo Night style.
    pub fn tokyo_night_day() -> Self {
        Self {
            accent: Color::Rgb(46, 125, 233),
            panel_bg: Color::Rgb(225, 226, 231),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(210, 211, 218),
            selection_bg: Color::Rgb(182, 202, 231),
            surface0: Color::Rgb(196, 200, 218),
            surface1: Color::Rgb(168, 174, 203),
            surface_dim: Color::Rgb(210, 211, 218),
            overlay0: Color::Rgb(137, 144, 179),
            overlay1: Color::Rgb(104, 112, 154),
            text: Color::Rgb(55, 96, 191),
            subtext0: Color::Rgb(97, 114, 176),
            mauve: Color::Rgb(120, 71, 189),
            green: Color::Rgb(88, 117, 57),
            yellow: Color::Rgb(140, 108, 62),
            red: Color::Rgb(245, 42, 101),
            blue: Color::Rgb(46, 125, 233),
            teal: Color::Rgb(17, 140, 116),
            peach: Color::Rgb(177, 92, 0),
        }
    }

    /// Dracula — purple/pink/green.
    pub fn dracula() -> Self {
        Self {
            accent: Color::Rgb(189, 147, 249), // purple
            panel_bg: Color::Rgb(40, 42, 54),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(55, 60, 82),
            selection_bg: Color::Rgb(70, 63, 93),
            surface0: Color::Rgb(68, 71, 90),
            surface1: Color::Rgb(98, 114, 164),
            surface_dim: Color::Rgb(40, 42, 54),
            overlay0: Color::Rgb(98, 114, 164),
            overlay1: Color::Rgb(130, 140, 180),
            text: Color::Rgb(248, 248, 242),
            subtext0: Color::Rgb(210, 210, 220),
            mauve: Color::Rgb(255, 121, 198), // pink
            green: Color::Rgb(80, 250, 123),
            yellow: Color::Rgb(241, 250, 140),
            red: Color::Rgb(255, 85, 85),
            blue: Color::Rgb(139, 233, 253), // cyan-ish
            teal: Color::Rgb(139, 233, 253),
            peach: Color::Rgb(255, 184, 108),
        }
    }

    /// Nord — frosty blue palette.
    pub fn nord() -> Self {
        Self {
            accent: Color::Rgb(136, 192, 208), // frost
            panel_bg: Color::Rgb(46, 52, 64),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(67, 76, 94),
            selection_bg: Color::Rgb(64, 80, 93),
            surface0: Color::Rgb(59, 66, 82),
            surface1: Color::Rgb(67, 76, 94),
            surface_dim: Color::Rgb(46, 52, 64),
            overlay0: Color::Rgb(76, 86, 106),
            overlay1: Color::Rgb(100, 110, 130),
            text: Color::Rgb(236, 239, 244),
            subtext0: Color::Rgb(216, 222, 233),
            mauve: Color::Rgb(180, 142, 173),
            green: Color::Rgb(163, 190, 140),
            yellow: Color::Rgb(235, 203, 139),
            red: Color::Rgb(191, 97, 106),
            blue: Color::Rgb(129, 161, 193),
            teal: Color::Rgb(143, 188, 187),
            peach: Color::Rgb(208, 135, 112),
        }
    }

    /// Gruvbox Dark — warm retro palette.
    pub fn gruvbox() -> Self {
        Self {
            accent: Color::Rgb(215, 153, 33), // yellow
            panel_bg: Color::Rgb(40, 40, 40),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(50, 49, 48),
            selection_bg: Color::Rgb(75, 63, 39),
            surface0: Color::Rgb(60, 56, 54),
            surface1: Color::Rgb(80, 73, 69),
            surface_dim: Color::Rgb(40, 40, 40),
            overlay0: Color::Rgb(146, 131, 116),
            overlay1: Color::Rgb(168, 153, 132),
            text: Color::Rgb(235, 219, 178),
            subtext0: Color::Rgb(213, 196, 161),
            mauve: Color::Rgb(211, 134, 155),
            green: Color::Rgb(184, 187, 38),
            yellow: Color::Rgb(250, 189, 47),
            red: Color::Rgb(251, 73, 52),
            blue: Color::Rgb(131, 165, 152),
            teal: Color::Rgb(142, 192, 124),
            peach: Color::Rgb(254, 128, 25),
        }
    }

    /// Gruvbox Light — the light retro palette.
    pub fn gruvbox_light() -> Self {
        Self {
            accent: Color::Rgb(7, 102, 120),
            panel_bg: Color::Rgb(251, 241, 199),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(242, 229, 188),
            selection_bg: Color::Rgb(235, 219, 178),
            surface0: Color::Rgb(235, 219, 178),
            surface1: Color::Rgb(213, 196, 161),
            surface_dim: Color::Rgb(242, 229, 188),
            overlay0: Color::Rgb(146, 131, 116),
            overlay1: Color::Rgb(124, 111, 100),
            text: Color::Rgb(60, 56, 54),
            subtext0: Color::Rgb(80, 73, 69),
            mauve: Color::Rgb(143, 63, 113),
            green: Color::Rgb(121, 116, 14),
            yellow: Color::Rgb(181, 118, 20),
            red: Color::Rgb(157, 0, 6),
            blue: Color::Rgb(7, 102, 120),
            teal: Color::Rgb(66, 123, 88),
            peach: Color::Rgb(175, 58, 3),
        }
    }

    /// One Dark — Atom's classic dark theme.
    pub fn one_dark() -> Self {
        Self {
            accent: Color::Rgb(97, 175, 239), // blue
            panel_bg: Color::Rgb(40, 44, 52),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(49, 54, 64),
            selection_bg: Color::Rgb(51, 70, 89),
            surface0: Color::Rgb(44, 49, 58),
            surface1: Color::Rgb(62, 68, 81),
            surface_dim: Color::Rgb(40, 44, 52),
            overlay0: Color::Rgb(92, 99, 112),
            overlay1: Color::Rgb(115, 122, 135),
            text: Color::Rgb(171, 178, 191),
            subtext0: Color::Rgb(150, 156, 168),
            mauve: Color::Rgb(198, 120, 221),
            green: Color::Rgb(152, 195, 121),
            yellow: Color::Rgb(229, 192, 123),
            red: Color::Rgb(224, 108, 117),
            blue: Color::Rgb(97, 175, 239),
            teal: Color::Rgb(86, 182, 194),
            peach: Color::Rgb(209, 154, 102),
        }
    }

    /// One Light — Atom's classic light theme.
    pub fn one_light() -> Self {
        Self {
            accent: Color::Rgb(64, 120, 242),
            panel_bg: Color::Rgb(250, 250, 250),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(216, 219, 226),
            selection_bg: Color::Rgb(205, 219, 248),
            surface0: Color::Rgb(240, 240, 241),
            surface1: Color::Rgb(229, 229, 230),
            surface_dim: Color::Rgb(245, 245, 246),
            overlay0: Color::Rgb(160, 161, 167),
            overlay1: Color::Rgb(104, 107, 119),
            text: Color::Rgb(56, 58, 66),
            subtext0: Color::Rgb(104, 107, 119),
            mauve: Color::Rgb(166, 38, 164),
            green: Color::Rgb(80, 161, 79),
            yellow: Color::Rgb(193, 132, 1),
            red: Color::Rgb(228, 86, 73),
            blue: Color::Rgb(64, 120, 242),
            teal: Color::Rgb(1, 132, 188),
            peach: Color::Rgb(152, 104, 1),
        }
    }

    /// Solarized Dark — Ethan Schoonover's classic.
    pub fn solarized() -> Self {
        Self {
            accent: Color::Rgb(38, 139, 210), // blue
            panel_bg: Color::Rgb(0, 43, 54),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(22, 75, 87),
            selection_bg: Color::Rgb(8, 62, 85),
            surface0: Color::Rgb(7, 54, 66),
            surface1: Color::Rgb(88, 110, 117),
            surface_dim: Color::Rgb(0, 43, 54),
            overlay0: Color::Rgb(88, 110, 117),
            overlay1: Color::Rgb(101, 123, 131),
            text: Color::Rgb(147, 161, 161),
            subtext0: Color::Rgb(131, 148, 150),
            mauve: Color::Rgb(211, 54, 130),
            green: Color::Rgb(133, 153, 0),
            yellow: Color::Rgb(181, 137, 0),
            red: Color::Rgb(220, 50, 47),
            blue: Color::Rgb(38, 139, 210),
            teal: Color::Rgb(42, 161, 152),
            peach: Color::Rgb(203, 75, 22),
        }
    }

    /// Solarized Light — Ethan Schoonover's light variant.
    pub fn solarized_light() -> Self {
        Self {
            accent: Color::Rgb(38, 139, 210),
            panel_bg: Color::Rgb(253, 246, 227),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(238, 232, 213),
            selection_bg: Color::Rgb(201, 220, 223),
            surface0: Color::Rgb(238, 232, 213),
            surface1: Color::Rgb(147, 161, 161),
            surface_dim: Color::Rgb(238, 232, 213),
            overlay0: Color::Rgb(147, 161, 161),
            overlay1: Color::Rgb(88, 110, 117),
            text: Color::Rgb(101, 123, 131),
            subtext0: Color::Rgb(131, 148, 150),
            mauve: Color::Rgb(211, 54, 130),
            green: Color::Rgb(133, 153, 0),
            yellow: Color::Rgb(181, 137, 0),
            red: Color::Rgb(220, 50, 47),
            blue: Color::Rgb(38, 139, 210),
            teal: Color::Rgb(42, 161, 152),
            peach: Color::Rgb(203, 75, 22),
        }
    }

    /// Kanagawa — inspired by Katsushika Hokusai.
    pub fn kanagawa() -> Self {
        Self {
            accent: Color::Rgb(126, 156, 216), // blue
            panel_bg: Color::Rgb(31, 31, 40),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(54, 54, 70),
            selection_bg: Color::Rgb(50, 56, 75),
            surface0: Color::Rgb(42, 42, 55),
            surface1: Color::Rgb(54, 54, 70),
            surface_dim: Color::Rgb(31, 31, 40),
            overlay0: Color::Rgb(114, 113, 105),
            overlay1: Color::Rgb(135, 134, 125),
            text: Color::Rgb(220, 215, 186),
            subtext0: Color::Rgb(200, 195, 170),
            mauve: Color::Rgb(149, 127, 184),
            green: Color::Rgb(118, 148, 106),
            yellow: Color::Rgb(192, 163, 110),
            red: Color::Rgb(195, 64, 67),
            blue: Color::Rgb(126, 156, 216),
            teal: Color::Rgb(127, 180, 202),
            peach: Color::Rgb(255, 160, 102),
        }
    }

    /// Kanagawa Lotus — the light Kanagawa variant.
    pub fn kanagawa_lotus() -> Self {
        Self {
            accent: Color::Rgb(77, 105, 155),
            panel_bg: Color::Rgb(242, 236, 188),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(213, 206, 163),
            selection_bg: Color::Rgb(220, 213, 172),
            surface0: Color::Rgb(220, 213, 172),
            surface1: Color::Rgb(201, 203, 209),
            surface_dim: Color::Rgb(213, 206, 163),
            overlay0: Color::Rgb(160, 156, 172),
            overlay1: Color::Rgb(138, 137, 128),
            text: Color::Rgb(84, 84, 100),
            subtext0: Color::Rgb(67, 67, 108),
            mauve: Color::Rgb(98, 76, 131),
            green: Color::Rgb(111, 137, 78),
            yellow: Color::Rgb(119, 113, 63),
            red: Color::Rgb(200, 64, 83),
            blue: Color::Rgb(77, 105, 155),
            teal: Color::Rgb(78, 140, 162),
            peach: Color::Rgb(204, 109, 0),
        }
    }

    /// Rosé Pine — muted, elegant.
    pub fn rose_pine() -> Self {
        Self {
            accent: Color::Rgb(196, 167, 231), // iris
            panel_bg: Color::Rgb(25, 23, 36),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(38, 35, 58),
            selection_bg: Color::Rgb(59, 52, 75),
            surface0: Color::Rgb(31, 29, 46),
            surface1: Color::Rgb(38, 35, 58),
            surface_dim: Color::Rgb(38, 35, 58),
            overlay0: Color::Rgb(110, 106, 134),
            overlay1: Color::Rgb(144, 140, 170),
            text: Color::Rgb(224, 222, 244),
            subtext0: Color::Rgb(200, 197, 220),
            mauve: Color::Rgb(196, 167, 231),  // iris
            green: Color::Rgb(49, 116, 143),   // pine
            yellow: Color::Rgb(246, 193, 119), // gold
            red: Color::Rgb(235, 111, 146),    // love
            blue: Color::Rgb(49, 116, 143),    // pine
            teal: Color::Rgb(156, 207, 216),   // foam
            peach: Color::Rgb(234, 154, 151),  // rose
        }
    }

    /// Rosé Pine Dawn — the light Rosé Pine variant.
    pub fn rose_pine_dawn() -> Self {
        Self {
            accent: Color::Rgb(144, 122, 169),
            panel_bg: Color::Rgb(250, 244, 237),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(227, 217, 207),
            selection_bg: Color::Rgb(242, 233, 225),
            surface0: Color::Rgb(242, 233, 225),
            surface1: Color::Rgb(255, 250, 243),
            surface_dim: Color::Rgb(242, 233, 225),
            overlay0: Color::Rgb(152, 147, 165),
            overlay1: Color::Rgb(121, 117, 147),
            text: Color::Rgb(70, 66, 97),
            subtext0: Color::Rgb(121, 117, 147),
            mauve: Color::Rgb(144, 122, 169),
            green: Color::Rgb(40, 105, 131),
            yellow: Color::Rgb(234, 157, 52),
            red: Color::Rgb(180, 99, 122),
            blue: Color::Rgb(40, 105, 131),
            teal: Color::Rgb(86, 148, 159),
            peach: Color::Rgb(215, 130, 126),
        }
    }

    /// Vesper — minimal high-contrast monochrome with peach and mint accents.
    pub fn vesper() -> Self {
        Self {
            accent: Color::Rgb(255, 199, 153),
            panel_bg: Color::Rgb(26, 26, 26),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(16, 16, 16),
            selection_bg: Color::Rgb(35, 35, 35),
            surface0: Color::Rgb(35, 35, 35),
            surface1: Color::Rgb(40, 40, 40),
            surface_dim: Color::Rgb(16, 16, 16),
            overlay0: Color::Rgb(92, 92, 92),
            overlay1: Color::Rgb(126, 126, 126),
            text: Color::Rgb(255, 255, 255),
            subtext0: Color::Rgb(160, 160, 160),
            mauve: Color::Rgb(255, 209, 168),
            green: Color::Rgb(153, 255, 228),
            yellow: Color::Rgb(255, 199, 153),
            red: Color::Rgb(255, 128, 128),
            blue: Color::Rgb(176, 176, 176),
            teal: Color::Rgb(102, 221, 204),
            peach: Color::Rgb(255, 199, 153),
        }
    }

    /// Resolve a theme by name. Returns None for unknown names.
    pub fn from_name(name: &str) -> Option<Self> {
        match crate::config::canonical_theme_name(name)? {
            "catppuccin" => Some(Self::catppuccin()),
            "catppuccin-latte" => Some(Self::catppuccin_latte()),
            "terminal" => Some(Self::terminal()),
            "tokyo-night" => Some(Self::tokyo_night()),
            "tokyo-night-day" => Some(Self::tokyo_night_day()),
            "dracula" => Some(Self::dracula()),
            "nord" => Some(Self::nord()),
            "gruvbox" => Some(Self::gruvbox()),
            "gruvbox-light" => Some(Self::gruvbox_light()),
            "one-dark" => Some(Self::one_dark()),
            "one-light" => Some(Self::one_light()),
            "solarized" => Some(Self::solarized()),
            "solarized-light" => Some(Self::solarized_light()),
            "kanagawa" => Some(Self::kanagawa()),
            "kanagawa-lotus" => Some(Self::kanagawa_lotus()),
            "rose-pine" => Some(Self::rose_pine()),
            "rose-pine-dawn" => Some(Self::rose_pine_dawn()),
            "vesper" => Some(Self::vesper()),
            _ => None,
        }
    }

    /// Apply custom color overrides on top of this palette.
    pub fn with_overrides(mut self, custom: &crate::config::CustomThemeColors) -> Self {
        use crate::config::parse_color;
        if let Some(c) = &custom.accent {
            self.accent = parse_color(c);
        }
        if let Some(c) = &custom.panel_bg {
            self.panel_bg = parse_color(c);
        }
        if let Some(c) = &custom.sidebar_bg {
            self.sidebar_bg = parse_color(c);
        }
        if let Some(c) = &custom.active_row_bg {
            self.active_row_bg = parse_color(c);
        }
        if let Some(c) = &custom.selection_bg {
            self.selection_bg = parse_color(c);
        }
        if let Some(c) = &custom.surface0 {
            self.surface0 = parse_color(c);
        }
        if let Some(c) = &custom.surface1 {
            self.surface1 = parse_color(c);
        }
        if let Some(c) = &custom.surface_dim {
            self.surface_dim = parse_color(c);
        }
        if let Some(c) = &custom.overlay0 {
            self.overlay0 = parse_color(c);
        }
        if let Some(c) = &custom.overlay1 {
            self.overlay1 = parse_color(c);
        }
        if let Some(c) = &custom.text {
            self.text = parse_color(c);
        }
        if let Some(c) = &custom.subtext0 {
            self.subtext0 = parse_color(c);
        }
        if let Some(c) = &custom.mauve {
            self.mauve = parse_color(c);
        }
        if let Some(c) = &custom.green {
            self.green = parse_color(c);
        }
        if let Some(c) = &custom.yellow {
            self.yellow = parse_color(c);
        }
        if let Some(c) = &custom.red {
            self.red = parse_color(c);
        }
        if let Some(c) = &custom.blue {
            self.blue = parse_color(c);
        }
        if let Some(c) = &custom.teal {
            self.teal = parse_color(c);
        }
        if let Some(c) = &custom.peach {
            self.peach = parse_color(c);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceCardArea {
    pub ws_idx: usize,
    pub rect: Rect,
    pub indented: bool,
}

/// Layout area for a collapsible group header row in the sidebar workspace list
/// (a visual group, or a synthesized repo header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupHeaderCardArea {
    pub name: String,
    pub collapse_key: String,
    pub rect: Rect,
}

/// Layout area for one Project-view row. `target` says what a click means, so
/// hit-testing never re-derives it from the row's position — the geometry pass
/// is the single source of truth, and a click at an offset row cannot land on
/// the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRowHitArea {
    pub rect: Rect,
    pub target: ProjectRowTarget,
}

/// What a Project-view row does when clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRowTarget {
    /// Toggle the project's collapse state.
    Project { collapse_key: String },
    /// Open an on-disk worktree that has no workspace yet.
    OpenWorktree { checkout_key: String },
    /// Toggle a COMMANDS/CHECKS band header (`WorkspaceListEntry::SectionHeader`).
    Band { collapse_key: String },
    /// Toggle a workspace's own section — one full section per workspace,
    /// main checkout or worktree alike (bora-c1h, `WorkspaceListEntry::SectionRow`).
    /// `checkout_key` names the git checkout for the bora-uqv
    /// `ProjectMemberTargets` right-click menu (resolved directly, no more
    /// `wt:`-prefix stripping); `ws_idx` is the workspace this section is
    /// for; `collapse_key` (`wsec:{ws_idx}`) toggles only its own panes.
    Section {
        ws_idx: usize,
        checkout_key: String,
        collapse_key: String,
    },
    /// T4 (bora-79l, P3): the SectionRow header's trailing 3-cell "+" —
    /// create a worktree+workspace in THIS section's context. Carries the
    /// section's `(repo_identity, branch)` — the branch_group pair, not a
    /// `ws_idx` — so T6's same-branch section merge re-keys nothing here:
    /// the drain resolves a live source workspace from the pair. Emitted
    /// BEFORE the full-row `Section` area of the same row, so
    /// `project_row_target_at`'s first-match resolves the "+" inside its
    /// own 3 cells (the same precedence the PaneDotsRow dot cells already
    /// use against the block card).
    SectionNew {
        repo_identity: String,
        branch: String,
    },
    /// Activate a row inside a band (run a command, open a check).
    /// Activate a row inside a band: run a command (COMMANDS rows carry
    /// the workspace to launch into), open a check/todo/doc (not wired).
    SectionItem {
        kind: &'static crate::ui::SectionDescriptor,
        label: String,
        ws_idx: Option<usize>,
    },
    /// Focus one pane of a multi-pane workspace.
    Pane { ws_idx: usize, pane_id: String },
    /// Open a PR from the project-level PULL REQUESTS band in a new worktree.
    ///
    /// `ws_idx` is a representative workspace of the PR's repo, resolved once
    /// when the band is built — NOT the active workspace. It exists only to
    /// name which repo the worktree is created in: `start_pr_worktree_create`
    /// turns it into the `workspace_id` of a `WorktreeCreate` call. Resolving
    /// it per render would be a workspace scan per row per pane per client,
    /// which the render path forbids.
    ///
    /// This is the same destination `ContextMenuKind::RepoPr`'s
    /// "Open in worktree" reaches, so a sidebar PR row and a right-click on
    /// the right panel's PR list do the same thing by construction.
    OpenPr { ws_idx: usize, number: u64 },
}

/// Layout area for the "+" (create worktree) affordance on a repo header row
/// in the sidebar workspace list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNewHitArea {
    pub repo_identity: String,
    pub rect: Rect,
}

/// Which tab the Create worktree modal is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeCreateTab {
    #[default]
    Github,
    Branch,
    Name,
}

/// Filter query + selection index for one of the modal's derived lists
/// (GitHub picks, local branches). The entries themselves are derived at
/// render/input time from the repo caches, so only the query and cursor live
/// here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeListPick {
    pub query: String,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateState {
    pub source_workspace_id: String,
    pub source_checkout_path: std::path::PathBuf,
    pub source_existing_membership: Option<crate::workspace::WorktreeSpaceMembership>,
    pub source_repo_root: std::path::PathBuf,
    pub repo_key: String,
    pub repo_name: String,
    pub branch: String,
    pub checkout_path: std::path::PathBuf,
    pub error: Option<String>,
    pub creating: bool,
    /// Which tab is active in the Create worktree modal.
    pub active_tab: WorktreeCreateTab,
    /// Repo identity (`GitSpaceMetadata.repo_identity`) used to key the
    /// `repo_open_prs` / `repo_issues` / `repo_branches` caches for this modal.
    pub repo_identity: String,
    /// Query + selection for the GitHub tab's merged PR/issue list.
    pub github_pick: WorktreeListPick,
    /// Query + selection for the Branch tab's local-branch list.
    pub branch_pick: WorktreeListPick,
}

/// Whether a GitHub pick row is a pull request or an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubPickKind {
    Pr,
    Issue,
}

/// A row in the Create worktree modal's GitHub tab — derived by merging the
/// repo's cached open PRs (first) and issues (second).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubPickEntry {
    pub kind: GithubPickKind,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// PR head branch, when known (PRs only).
    pub head_ref: Option<String>,
    /// Whether selecting the row does anything. Issue rows are disabled when
    /// no `[flow]` command is configured.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRemoveState {
    pub workspace_id: String,
    pub repo_root: std::path::PathBuf,
    pub path: std::path::PathBuf,
    pub error: Option<String>,
    pub removing: bool,
    pub force_confirmation: bool,
    /// Branch checked out in this worktree, when known — enables "merge & close".
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeOpenEntry {
    pub path: std::path::PathBuf,
    pub branch: Option<String>,
    pub is_linked_worktree: bool,
    pub already_open_ws_idx: Option<usize>,
}

impl WorktreeOpenEntry {
    pub(crate) fn display_name(&self) -> String {
        self.branch.clone().unwrap_or_else(|| {
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| self.path.display().to_string())
        })
    }

    pub(crate) fn status_label(&self) -> &'static str {
        if self.already_open_ws_idx.is_some() {
            "open"
        } else if self.branch.is_some() {
            ""
        } else if self.is_linked_worktree {
            "detached"
        } else {
            "root"
        }
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.display_name(),
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
            self.path.display(),
            self.status_label()
        )
        .to_lowercase()
    }

    fn matches_query(&self, query: &str) -> bool {
        text_matches_query(query, &self.search_text())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeOpenState {
    pub source_workspace_id: String,
    pub source_existing_membership: Option<crate::workspace::WorktreeSpaceMembership>,
    pub source_checkout_path: std::path::PathBuf,
    pub source_repo_root: std::path::PathBuf,
    pub repo_key: String,
    pub repo_name: String,
    pub entries: Vec<WorktreeOpenEntry>,
    pub selected: usize,
    pub query: String,
    pub search_focused: bool,
    pub error: Option<String>,
}

impl WorktreeOpenState {
    pub(crate) fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                (query.is_empty() || entry.matches_query(query)).then_some(idx)
            })
            .collect()
    }

    pub(crate) fn selected_entry_index(&self) -> Option<usize> {
        let indices = self.filtered_indices();
        if indices.contains(&self.selected) {
            Some(self.selected)
        } else {
            indices.first().copied()
        }
    }

    pub(crate) fn normalize_selection(&mut self) {
        if let Some(selected) = self.selected_entry_index() {
            self.selected = selected;
        }
    }

    pub(crate) fn select_previous_filtered(&mut self) {
        let indices = self.filtered_indices();
        let Some(current) = self.selected_entry_index() else {
            return;
        };
        let pos = indices.iter().position(|idx| *idx == current).unwrap_or(0);
        self.selected = indices[pos.saturating_sub(1)];
    }

    pub(crate) fn select_next_filtered(&mut self) {
        let indices = self.filtered_indices();
        let Some(current) = self.selected_entry_index() else {
            return;
        };
        let pos = indices.iter().position(|idx| *idx == current).unwrap_or(0);
        self.selected = indices[(pos + 1).min(indices.len().saturating_sub(1))];
    }
}

pub(crate) fn text_matches_query(query: &str, text: &str) -> bool {
    let haystack = text.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|needle| haystack.contains(needle))
}

/// Computed view geometry — derived from AppState + terminal size.
/// Updated before each render, consumed by render and mouse handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewLayout {
    Desktop,
    Mobile,
}

pub struct ViewState {
    pub layout: ViewLayout,
    pub sidebar_rect: Rect,
    pub workspace_card_areas: Vec<WorkspaceCardArea>,
    pub workspace_group_header_areas: Vec<GroupHeaderCardArea>,
    /// Project-view row hit areas, refreshed by the geometry pass. Empty in
    /// the Flat and Repo views.
    pub project_row_areas: Vec<ProjectRowHitArea>,
    pub worktree_new_hit_areas: Vec<WorktreeNewHitArea>,
    pub tab_bar_rect: Rect,
    pub tab_hit_areas: Vec<Rect>,
    pub tab_scroll_left_hit_area: Rect,
    pub tab_scroll_right_hit_area: Rect,
    pub new_tab_hit_area: Rect,
    pub terminal_area: Rect,
    pub mobile_header_rect: Rect,
    pub mobile_menu_hit_area: Rect,
    pub mobile_prev_tab_hit_area: Rect,
    pub mobile_next_tab_hit_area: Rect,
    pub toast_hit_area: Rect,
    pub pane_infos: Vec<PaneInfo>,
    pub split_borders: Vec<SplitBorder>,
    pub right_panel_rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPanelTab {
    #[default]
    Changes,
    Checks,
    Issues,
    PullRequests,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Onboarding,
    ReleaseNotes,
    ProductAnnouncement,
    Navigate,
    Prefix,
    Copy,
    Terminal,
    RenameWorkspace,
    RenameTab,
    RenamePane,
    /// User is typing a visual group name for a workspace.
    SetWorkspaceGroup,
    /// User is typing a project name (creating or renaming a project in
    /// `projects.yml`). What the name is for lives in
    /// `AppState::project_name_target`.
    ProjectNameInput,
    /// User is typing an arbitrary shell command from the sidebar Programs
    /// launcher's "+ run command…" row.
    NewLinkedWorktree,
    OpenExistingWorktree,
    ConfirmRemoveWorktree,
    Resize,
    ConfirmClose,
    ContextMenu,
    Settings,
    GlobalMenu,
    KeybindHelp,
    Navigator,
    Chat,
}

impl Mode {
    pub(crate) fn mouse_motion_changes_view(self) -> bool {
        matches!(self, Self::GlobalMenu | Self::ContextMenu | Self::Navigator)
    }

    /// Whether keys in this mode are commands/navigation (an ASCII input source is wanted) rather
    /// than free text. This is an explicit **allowlist** of the prefix command/navigation realm:
    /// any mode NOT listed defaults to leaving the user's IME alone (the safe default), so adding a
    /// new text-entry or overlay mode can never silently force ASCII. Used by
    /// `sync_prefix_input_source` (gated by `switch_ascii_input_source_in_prefix`) so multi-level
    /// prefix commands keep ASCII until they return to the terminal.
    ///
    /// Known limitation: the search boxes in `Navigator` and `KeybindHelp` are also held on ASCII,
    /// since this `Mode`-level predicate can't see `search_focused` (non-ASCII filtering there
    /// would need a runtime check).
    pub(crate) fn wants_ascii_input(self) -> bool {
        matches!(
            self,
            Mode::Prefix
                | Mode::Navigate
                | Mode::Navigator
                | Mode::Copy
                | Mode::Resize
                | Mode::ConfirmClose
                | Mode::ConfirmRemoveWorktree
                | Mode::ContextMenu
                | Mode::GlobalMenu
                | Mode::KeybindHelp
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NavigatorTarget {
    Workspace {
        ws_idx: usize,
    },
    Tab {
        ws_idx: usize,
        tab_idx: usize,
    },
    Pane {
        ws_idx: usize,
        tab_idx: usize,
        pane_id: PaneId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigatorRow {
    pub target: NavigatorTarget,
    pub depth: u8,
    pub label: String,
    pub meta: String,
    pub status: AgentState,
    pub seen: bool,
    pub is_current: bool,
    pub is_workspace: bool,
    pub is_tab: bool,
    pub expanded: bool,
    pub search_text: String,
    /// Whether this row itself matched the active query/state filter, as
    /// opposed to being included as ancestor context or cascaded subtree of a
    /// matching workspace or tab. Always true when no filter is active.
    pub matched: bool,
}

/// One rendered line in the navigator body. Spacer lines separate workspace
/// groups visually and are not selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigatorDisplayLine {
    Spacer,
    Row(usize),
}

pub(crate) fn navigator_display_lines(rows: &[NavigatorRow]) -> Vec<NavigatorDisplayLine> {
    let mut lines = Vec::with_capacity(rows.len().saturating_mul(2));
    for (idx, row) in rows.iter().enumerate() {
        if row.is_workspace && !lines.is_empty() {
            lines.push(NavigatorDisplayLine::Spacer);
        }
        lines.push(NavigatorDisplayLine::Row(idx));
    }
    lines
}

pub(crate) fn navigator_display_index_of_row(
    lines: &[NavigatorDisplayLine],
    row_idx: usize,
) -> Option<usize> {
    lines
        .iter()
        .position(|line| *line == NavigatorDisplayLine::Row(row_idx))
}

pub(crate) fn navigator_first_row_at_or_after(
    lines: &[NavigatorDisplayLine],
    line_idx: usize,
) -> Option<usize> {
    lines.get(line_idx..)?.iter().find_map(|line| match line {
        NavigatorDisplayLine::Row(idx) => Some(*idx),
        NavigatorDisplayLine::Spacer => None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigatorStateFilter {
    Blocked,
    Working,
    Idle,
    Done,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NavigatorState {
    pub query: String,
    pub selected: usize,
    pub scroll: usize,
    pub search_focused: bool,
    pub state_filter: Option<NavigatorStateFilter>,
    pub expanded_workspaces: std::collections::HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyModeState {
    pub pane_id: PaneId,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub entry_offset_from_bottom: usize,
    pub selection: Option<CopyModeSelection>,
    pub search: CopyModeSearchState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyModeSelection {
    Character,
    Linewise { anchor_row: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyModeSearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyModeSearchPrompt {
    pub direction: CopyModeSearchDirection,
    pub query: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CopyModeSearchState {
    pub prompt: Option<CopyModeSearchPrompt>,
    pub query: String,
    pub direction: Option<CopyModeSearchDirection>,
    pub matches: Vec<crate::pane::TerminalTextMatch>,
    pub current: Option<usize>,
    pub geometry: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentPanelSort {
    #[default]
    Spaces,
    Priority,
}

// ---------------------------------------------------------------------------
// Settings UI state
// ---------------------------------------------------------------------------

/// Which section of the settings panel is focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Theme,
    Indicators,
    Sound,
    Toast,
    PaneLabels,
    Sidebar,
    Integrations,
}

impl SettingsSection {
    pub const ALL: &[Self] = &[
        Self::Theme,
        Self::Indicators,
        Self::Sound,
        Self::Toast,
        Self::PaneLabels,
        Self::Sidebar,
        Self::Integrations,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Indicators => "indicators",
            Self::Sound => "sound",
            Self::Toast => "toasts",
            Self::PaneLabels => "pane labels",
            Self::Sidebar => "sidebar",
            Self::Integrations => "integrations",
        }
    }
}

/// All built-in theme names in display order.
pub const THEME_NAMES: &[&str] = crate::config::THEME_NAMES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuListState {
    pub highlighted: usize,
}

impl MenuListState {
    pub fn new(highlighted: usize) -> Self {
        Self { highlighted }
    }

    pub fn move_prev(&mut self) {
        self.highlighted = self.highlighted.saturating_sub(1);
    }

    pub fn move_next(&mut self, item_count: usize) {
        if item_count > 0 {
            self.highlighted = (self.highlighted + 1).min(item_count - 1);
        }
    }

    pub fn hover(&mut self, idx: Option<usize>) {
        if let Some(idx) = idx {
            self.highlighted = idx;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionListState {
    pub selected: usize,
}

impl SelectionListState {
    pub fn new(selected: usize) -> Self {
        Self { selected }
    }

    pub fn move_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_next(&mut self, item_count: usize) {
        if item_count > 0 {
            self.selected = (self.selected + 1).min(item_count - 1);
        }
    }

    pub fn select(&mut self, idx: usize) {
        self.selected = idx;
    }
}

#[derive(Debug, Clone)]
pub struct ThemeRuntimeConfig {
    pub manual_name: String,
    pub dark_name: String,
    pub light_name: String,
    pub auto_switch: bool,
    pub custom: Option<crate::config::CustomThemeColors>,
    pub legacy_accent: Option<String>,
}

pub struct SettingsState {
    /// Which section tab is active.
    pub section: SettingsSection,
    /// Selected item index within the current section.
    pub list: SelectionListState,
    /// The palette before opening settings (for cancel/restore).
    pub original_palette: Option<Palette>,
    /// The theme name before opening settings.
    pub original_theme: Option<String>,
}

pub(crate) enum DragTarget {
    WorkspaceReorder {
        source_id: crate::app::InputSourceId,
        source_ws_idx: usize,
        insert_idx: Option<usize>,
    },
    TabReorder {
        source_id: crate::app::InputSourceId,
        ws_idx: usize,
        source_tab_idx: usize,
        insert_idx: Option<usize>,
    },
    WorkspaceListScrollbar {
        grab_row_offset: u16,
    },
    AgentPanelScrollbar {
        grab_row_offset: u16,
    },
    PaneSplit {
        path: Vec<bool>,
        direction: Direction,
        area: Rect,
        grab_offset: u16,
    },
    PaneScrollbar {
        pane_id: crate::layout::PaneId,
        grab_row_offset: u16,
    },
    ReleaseNotesScrollbar {
        grab_row_offset: u16,
    },
    ProductAnnouncementScrollbar {
        grab_row_offset: u16,
    },
    KeybindHelpScrollbar {
        grab_row_offset: u16,
    },
    SidebarDivider,
    SidebarSectionDivider,
}

/// Active mouse drag on a split border or sidebar divider.
pub(crate) struct DragState {
    pub target: DragTarget,
}

pub(crate) struct WorkspacePressState {
    pub ws_idx: usize,
    pub start_col: u16,
    pub start_row: u16,
}

pub(crate) struct TabPressState {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub start_col: u16,
    pub start_row: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuKind {
    Workspace {
        ws_idx: usize,
        hidden: bool,
    },
    GitWorkspace {
        ws_idx: usize,
        is_linked_worktree: bool,
        has_worktree_children: bool,
        collapsed: bool,
        hidden: bool,
    },
    /// A sidebar group/project header row (visual group or repo group).
    /// `collapse_key` is the same key used for collapse state; `hidden` is
    /// whether that key is currently hidden. Its plugin-action context is
    /// `Global` (bora-1e9) — the general-purpose surface every enabled
    /// plugin action declaring `contexts = ["global"]` appears on, resolved
    /// at menu-build time by `build_context_menu_items`, never cached here.
    GroupHeader {
        name: String,
        collapse_key: String,
        hidden: bool,
    },
    /// A Project-view group header row: a declared project (`slug: Some`) or
    /// the synthetic `Ungrouped` orphans bucket (`slug: None`). `collapse_key`
    /// doubles as the Hide key, same as `GroupHeader`. Its plugin-action
    /// context is `Global`, same surface as `GroupHeader`.
    ProjectHeader {
        slug: Option<String>,
        collapse_key: String,
        hidden: bool,
    },
    /// The orphan-workspace picker opened by "Add workspaces…" on a
    /// ProjectHeader menu. `orphans` are the candidate member dirs, aligned
    /// with the menu items by index; `slug` is the project to file the pick
    /// into, or `None` when the picker came from Ungrouped and the target
    /// project is chosen in the follow-up `ProjectMemberTargets` menu.
    ProjectOrphanPicker {
        slug: Option<String>,
        orphans: Vec<String>,
    },
    /// The per-dir project membership menu (Add to <slug> / New project… /
    /// Remove), opened from a Project-view worktree/checkout row or from the
    /// orphan picker. `member_dir` is the exact string `projects.yml`
    /// `Member.dir` comparisons use (see `ProjectAssemblyContext::for_dir`).
    ProjectMemberTargets {
        member_dir: String,
    },
    Tab {
        ws_idx: usize,
        tab_idx: usize,
    },
    Pane {
        ws_idx: usize,
        tab_idx: usize,
        pane_id: PaneId,
        source_pane_id: Option<PaneId>,
        has_manual_label: bool,
        right_click_passthrough: bool,
    },
    /// An open PR row in the right panel's `PullRequests` tab — not the
    /// sidebar; the sidebar has no PR row variant. Built at exactly one site
    /// (`src/app/input/mouse.rs`, the `RightPanelTab::PullRequests` arm), so
    /// `ws_idx` is whatever workspace is active at click time, which is only
    /// coincidentally a workspace of the PR's repo. `request_open_pr_worktree`
    /// consumes the pair, and "Open in worktree" is therefore already a
    /// working right-click PR action.
    RepoPr {
        ws_idx: usize,
        number: u64,
        url: String,
        head_ref: String,
    },
    /// An issue row in the right-panel Issues tab. `flow_available` is
    /// resolved at menu-open time from the per-repo `.bora.toml` `[flow]`
    /// override and the global `[flow]` config template.
    RepoIssue {
        number: u64,
        url: String,
        flow_available: bool,
    },
}

/// Right-click context menu state.
pub struct ContextMenuState {
    pub kind: ContextMenuKind,
    pub x: u16,
    pub y: u16,
    pub list: MenuListState,
    pub items: Vec<String>,
    pub bora_commands: Vec<crate::bora_config::BoraCommand>,
    pub bora_port: Option<u16>,
}

/// Menu separator: rendered as a dim line, not selectable.
pub const CONTEXT_MENU_SEPARATOR: &str = "─";
/// What the `ProjectNameInput` modal writes on confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectNameTarget {
    /// Set `name:` on the existing project `slug` in `projects.yml`.
    Rename { slug: String },
    /// Create a new project named as typed; `member_dir`, when present (the
    /// row the menu was opened from), becomes its first member.
    New { member_dir: Option<String> },
}

pub fn build_context_menu_items(
    kind: &ContextMenuKind,
    workspaces: &[crate::workspace::Workspace],
    view_mode: crate::config::ViewMode,
    assembly_items: &[String],
    custom_commands: &[String],
    installed_plugins: &InstalledPluginRegistry,
) -> Vec<String> {
    let groups: Vec<String> = {
        let mut set = std::collections::BTreeSet::new();
        for ws in workspaces {
            if let Some(g) = &ws.visual_group {
                set.insert(g.clone());
            }
        }
        set.into_iter().collect()
    };
    let sep = || CONTEXT_MENU_SEPARATOR.to_string();
    let push_groups = |v: &mut Vec<String>| {
        v.push("New group\u{2026}".to_string());
        for g in &groups {
            v.push(format!("\u{2192} {g}"));
        }
        v.push("Remove from group".to_string());
    };
    let push_hide = |v: &mut Vec<String>, hidden: bool| {
        v.push(sep());
        if hidden {
            v.push("Unhide".to_string());
        } else {
            v.push("Hide 5m".to_string());
            v.push("Hide 10m".to_string());
            v.push("Hide 15m".to_string());
            v.push("Hide 30m".to_string());
        }
    };
    let mut v = match kind {
        ContextMenuKind::Workspace { hidden, .. } => {
            let mut v = vec![
                "Rename".to_string(),
                "Copy path".to_string(),
                "Refresh status".to_string(),
                sep(),
            ];
            if view_mode == crate::config::ViewMode::Project {
                v.extend(assembly_items.iter().cloned());
            } else {
                push_groups(&mut v);
            }
            if !custom_commands.is_empty() {
                v.push(sep());
                v.extend(custom_commands.iter().cloned());
            }
            v.push(sep());
            v.push("Close".to_string());
            push_hide(&mut v, *hidden);
            v
        }
        ContextMenuKind::GitWorkspace {
            is_linked_worktree: false,
            has_worktree_children: false,
            hidden,
            ..
        } => {
            let mut v = vec![
                "Rename".to_string(),
                "Copy path".to_string(),
                sep(),
                "New worktree".to_string(),
                "Open worktree\u{2026}".to_string(),
                "Sync".to_string(),
                "Refresh status".to_string(),
                sep(),
            ];
            if view_mode == crate::config::ViewMode::Project {
                v.extend(assembly_items.iter().cloned());
            } else {
                push_groups(&mut v);
            }
            if !custom_commands.is_empty() {
                v.push(sep());
                v.extend(custom_commands.iter().cloned());
            }
            v.push(sep());
            v.push("Close".to_string());
            push_hide(&mut v, *hidden);
            v
        }
        ContextMenuKind::GitWorkspace {
            is_linked_worktree: true,
            hidden,
            ..
        } => {
            let mut v = vec![
                "Rename".to_string(),
                "Copy path".to_string(),
                sep(),
                "Merge to main".to_string(),
                "Open PR".to_string(),
                "Sync".to_string(),
                "Refresh status".to_string(),
                sep(),
            ];
            if view_mode == crate::config::ViewMode::Project {
                v.extend(assembly_items.iter().cloned());
            } else {
                push_groups(&mut v);
            }
            if !custom_commands.is_empty() {
                v.push(sep());
                v.extend(custom_commands.iter().cloned());
            }
            v.push(sep());
            v.push("Close".to_string());
            v.push("Delete worktree\u{2026}".to_string());
            push_hide(&mut v, *hidden);
            v
        }
        ContextMenuKind::GitWorkspace {
            has_worktree_children: true,
            collapsed: true,
            hidden,
            ..
        } => {
            let mut v = vec![
                "Rename".to_string(),
                "Copy path".to_string(),
                sep(),
                "New worktree".to_string(),
                "Open worktree\u{2026}".to_string(),
                "Sync".to_string(),
                "Refresh status".to_string(),
                "Expand".to_string(),
                sep(),
            ];
            if view_mode == crate::config::ViewMode::Project {
                v.extend(assembly_items.iter().cloned());
            } else {
                push_groups(&mut v);
            }
            if !custom_commands.is_empty() {
                v.push(sep());
                v.extend(custom_commands.iter().cloned());
            }
            v.push(sep());
            v.push("Close workspace".to_string());
            push_hide(&mut v, *hidden);
            v
        }
        ContextMenuKind::GitWorkspace {
            has_worktree_children: true,
            collapsed: false,
            hidden,
            ..
        } => {
            let mut v = vec![
                "Rename".to_string(),
                "Copy path".to_string(),
                sep(),
                "New worktree".to_string(),
                "Open worktree\u{2026}".to_string(),
                "Sync".to_string(),
                "Refresh status".to_string(),
                "Collapse".to_string(),
                sep(),
            ];
            if view_mode == crate::config::ViewMode::Project {
                v.extend(assembly_items.iter().cloned());
            } else {
                push_groups(&mut v);
            }
            if !custom_commands.is_empty() {
                v.push(sep());
                v.extend(custom_commands.iter().cloned());
            }
            v.push(sep());
            v.push("Close workspace".to_string());
            push_hide(&mut v, *hidden);
            v
        }
        ContextMenuKind::GroupHeader { hidden, .. } => {
            if *hidden {
                vec!["Unhide".to_string()]
            } else {
                vec![
                    "Hide 5m".to_string(),
                    "Hide 10m".to_string(),
                    "Hide 15m".to_string(),
                    "Hide 30m".to_string(),
                ]
            }
        }
        ContextMenuKind::ProjectHeader { hidden, .. } => {
            // The assembly lead (Add workspaces… / New project… / Rename
            // project…) is computed at the call site from a fresh
            // projects.yml read; this builder owns only the shared
            // Hide/Unhide tail, same shape as GroupHeader.
            let mut v = assembly_items.to_vec();
            v.push(sep());
            if *hidden {
                v.push("Unhide".to_string());
            } else {
                v.push("Hide 5m".to_string());
                v.push("Hide 10m".to_string());
                v.push("Hide 15m".to_string());
                v.push("Hide 30m".to_string());
            }
            v
        }
        // Picker/follow-up kinds: every item is computed at the call site
        // (orphan dirs, membership resolved against a fresh projects.yml
        // read); the builder has nothing to add.
        ContextMenuKind::ProjectOrphanPicker { .. }
        | ContextMenuKind::ProjectMemberTargets { .. } => assembly_items.to_vec(),
        ContextMenuKind::Tab { .. } => {
            vec![
                "New tab".to_string(),
                "Rename".to_string(),
                "Close".to_string(),
            ]
        }
        ContextMenuKind::Pane {
            has_manual_label,
            source_pane_id,
            right_click_passthrough,
            ..
        } => {
            let mut v = vec!["Rename pane".to_string()];
            if *has_manual_label {
                v.push("Clear pane name".to_string());
            }
            if source_pane_id.is_some() {
                v.push("Swap with focused pane".to_string());
            }
            v.extend([
                "Split right".to_string(),
                "Split down".to_string(),
                "Zoom".to_string(),
            ]);
            v.push(if *right_click_passthrough {
                "Use Herdr right-click menu".to_string()
            } else {
                "Send right-clicks to pane".to_string()
            });
            // The sidebar no longer prints the `@<id>` badge on a pane row
            // (Ary's call: the id is reference material, not something you
            // read every frame), so this is where you get it when you do
            // need it — the same shape as RepoPr's "Copy URL".
            v.push("Copy pane ID".to_string());
            v.push("Close pane".to_string());
            v
        }
        ContextMenuKind::RepoPr { .. } => vec![
            "Open in worktree".to_string(),
            sep(),
            "Open in browser".to_string(),
            "Copy URL".to_string(),
        ],
        ContextMenuKind::RepoIssue { flow_available, .. } => {
            let mut v = Vec::new();
            if *flow_available {
                v.push("Run with bora-flow".to_string());
                v.push(sep());
            }
            v.push("Open in browser".to_string());
            v.push("Copy URL".to_string());
            v
        }
    };
    let plugin_titles = plugin_menu_titles(kind, installed_plugins);
    if !plugin_titles.is_empty() {
        v.push(sep());
        v.extend(plugin_titles);
    }
    v
}

/// Which `PluginActionContext` a menu kind exposes to plugin actions.
/// Exhaustive over `ContextMenuKind`: a new variant must decide its context
/// here or the match fails to compile — no default that silently exposes
/// nothing (bora-1e9, gate G1). `Global` actions are visible from every
/// menu regardless of this mapping (see `plugin_actions_for_context`); this
/// only fixes each kind's *own* context.
fn plugin_menu_context(kind: &ContextMenuKind) -> crate::api::schema::PluginActionContext {
    use crate::api::schema::PluginActionContext as Ctx;
    match kind {
        ContextMenuKind::Workspace { .. } => Ctx::Workspace,
        ContextMenuKind::GitWorkspace { .. } => Ctx::Workspace,
        // GroupHeader is the general-purpose surface — exactly where the
        // old dagr-only availability flag used to live on this variant.
        // Now any enabled plugin action declaring `contexts = ["global"]`
        // lands here through the ordinary mechanism below, dagr included.
        ContextMenuKind::GroupHeader { .. } => Ctx::Global,
        // The project-assembly surfaces are the same general-purpose
        // surface as GroupHeader: a project header, an orphan picker, and a
        // membership menu are all places a global plugin action makes sense.
        ContextMenuKind::ProjectHeader { .. } => Ctx::Global,
        ContextMenuKind::ProjectOrphanPicker { .. } => Ctx::Global,
        ContextMenuKind::ProjectMemberTargets { .. } => Ctx::Global,
        ContextMenuKind::Tab { .. } => Ctx::Tab,
        ContextMenuKind::Pane { .. } => Ctx::Pane,
        // No PluginActionContext variant models a PR/issue row (only
        // Global | Workspace | Tab | Pane | Selection exist). RepoPr's own
        // doc comment warns `ws_idx` is "only coincidentally" the PR's
        // workspace, so mapping to Workspace would be misleading; Global —
        // the same general-purpose surface GroupHeader uses — is the
        // honest fallback: a plugin wanting a PR/issue action declares
        // `contexts = ["global"]`.
        ContextMenuKind::RepoPr { .. } => Ctx::Global,
        ContextMenuKind::RepoIssue { .. } => Ctx::Global,
    }
}

/// Plugin action titles to append to a menu of `kind`, in registry order.
/// Delegates entirely to `plugin_actions_for_context` (app::api::plugins) —
/// enabled-only, context-matched, Global-everywhere; a disabled plugin or a
/// non-matching/absent context contributes nothing here.
fn plugin_menu_titles(
    kind: &ContextMenuKind,
    installed_plugins: &InstalledPluginRegistry,
) -> Vec<String> {
    crate::app::api::plugins::plugin_actions_for_context(
        installed_plugins,
        plugin_menu_context(kind),
    )
    .into_iter()
    .map(|action| action.title)
    .collect()
}

/// Resolve a selected menu label back to the plugin action it names, as the
/// fully-qualified `plugin_id.action_id` `find_plugin_action` (bora-1e9,
/// gate G4) resolves at invoke time. Mirrors `plugin_menu_titles` exactly —
/// same context, same registry read — so any label the menu actually shows
/// resolves here to exactly the action that produced it.
pub(crate) fn plugin_menu_action_id(
    kind: &ContextMenuKind,
    label: &str,
    installed_plugins: &InstalledPluginRegistry,
) -> Option<String> {
    crate::app::api::plugins::plugin_actions_for_context(
        installed_plugins,
        plugin_menu_context(kind),
    )
    .into_iter()
    .find(|action| action.title == label)
    .map(|action| action.qualified_id())
}

impl ContextMenuState {
    pub fn items(&self) -> &[String] {
        &self.items
    }
}

impl AppState {
    /// Resolve the effective flow command template for the active workspace's
    /// repo: the `.bora.toml` `[flow]` override wins over the global `[flow]`
    /// config template. `None` means the "Run with bora-flow" action is
    /// unavailable. Reads `.bora.toml` per call, matching the workspace
    /// context menu's per-click `.bora.toml` read.
    pub(crate) fn repo_issue_flow_template(&self) -> Option<String> {
        let per_repo = self
            .active
            .and_then(|idx| self.workspaces.get(idx))
            .and_then(|ws| {
                crate::bora_config::load_bora_config(ws.bora_config_root()?)?
                    .flow?
                    .command
            });
        crate::app::flow::resolve_flow_template(
            per_repo.as_deref(),
            self.flow_command_template.as_deref(),
        )
    }

    /// Merged GitHub picks for the Create worktree modal: open PRs first, then
    /// issues, filtered by the GitHub tab query (case-insensitive over
    /// `#<number>` and title). Empty when no modal is open. Issue rows are only
    /// enabled when a `[flow]` command is configured.
    pub(crate) fn create_worktree_github_entries(&self) -> Vec<GithubPickEntry> {
        let Some(create) = self.worktree_create.as_ref() else {
            return Vec::new();
        };
        let query = create.github_pick.query.trim().to_lowercase();
        let matches = |number: u64, title: &str| {
            query.is_empty()
                || format!("#{number}").contains(&query)
                || title.to_lowercase().contains(&query)
        };
        let issues_enabled = self.repo_issue_flow_template().is_some();
        let mut entries = Vec::new();
        if let Some(prs) = self.repo_open_prs.get(&create.repo_identity) {
            for pr in &prs.prs {
                if matches(pr.number, &pr.title) {
                    entries.push(GithubPickEntry {
                        kind: GithubPickKind::Pr,
                        number: pr.number,
                        title: pr.title.clone(),
                        url: pr.url.clone(),
                        head_ref: Some(pr.head_ref_name.clone()),
                        enabled: true,
                    });
                }
            }
        }
        if let Some(issues) = self.repo_issues.get(&create.repo_identity) {
            for issue in &issues.issues {
                if matches(issue.number, &issue.title) {
                    entries.push(GithubPickEntry {
                        kind: GithubPickKind::Issue,
                        number: issue.number,
                        title: issue.title.clone(),
                        url: issue.url.clone(),
                        head_ref: None,
                        enabled: issues_enabled,
                    });
                }
            }
        }
        entries
    }

    /// Local branches for the Create worktree modal's Branch tab, filtered by
    /// the Branch tab query (case-insensitive substring over the name). Empty
    /// when no modal is open or the branch cache is unpopulated.
    pub(crate) fn create_worktree_branch_entries(&self) -> Vec<crate::workspace::RepoBranch> {
        let Some(create) = self.worktree_create.as_ref() else {
            return Vec::new();
        };
        let query = create.branch_pick.query.trim().to_lowercase();
        self.repo_branches
            .get(&create.repo_identity)
            .map(|branches| {
                branches
                    .branches
                    .iter()
                    .filter(|b| query.is_empty() || b.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A request to run the configured flow command for a GitHub issue, set by
/// the Issues tab context menu and drained by the App event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRunRequest {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct PendingBoraCommand {
    pub ws_idx: usize,
    pub command: String,
    pub mode: crate::bora_config::BoraCommandMode,
    pub port: Option<u16>,
    /// Label of the originating bora command, so the Pane arm can tag the
    /// spawned pane. None for shell-mode runs (fire-and-forget, uncounted).
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    NeedsAttention,
    Finished,
    UpdateInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastTarget {
    pub workspace_id: String,
    pub pane_id: PaneId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastNotification {
    pub kind: ToastKind,
    pub title: String,
    pub context: String,
    pub position: Option<crate::config::ToastHerdrPosition>,
    pub target: Option<ToastTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAgentNotification {
    pub pane_id: PaneId,
    pub workspace_id: String,
    pub agent_label: String,
    pub known_agent: Option<crate::detect::Agent>,
    pub kind: ToastKind,
    pub state: AgentState,
    pub deadline: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNotificationDelivery {
    pub pane_id: PaneId,
    pub workspace_id: String,
    pub agent_label: String,
    pub known_agent: Option<crate::detect::Agent>,
    pub kind: ToastKind,
    pub toast: Option<ToastNotification>,
    pub client_notification: Option<ToastNotification>,
    pub sound: Option<crate::sound::Sound>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFeedback {
    pub message: String,
}

pub struct ReleaseNotesState {
    pub version: String,
    pub body: String,
    pub scroll: u16,
    pub preview: bool,
}

pub struct ProductAnnouncementState {
    pub version: String,
    pub id: String,
    pub title: String,
    pub body: String,
    pub scroll: u16,
    pub preview: bool,
}

#[derive(Default)]
pub struct KeybindHelpState {
    pub scroll: u16,
    pub query: String,
    pub search_focused: bool,
}

/// One candidate row of the `AddMember` prompt: a running agent pane that is
/// not yet a member of the selected channel. Built from `agent.list`, the
/// same agent inventory external clients read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMemberCandidate {
    /// Public pane id, passed straight to `channel.join`.
    pub pane_id: String,
    pub name: String,
    /// Shortened working directory, for telling same-named agents apart.
    pub cwd: Option<String>,
    /// Live agent status label ("idle", "working", ...).
    pub status: String,
}

/// Modal sub-mode of the chat view, drawn as one small centered box over the
/// overlay. While a prompt is open it owns the keyboard instead of the
/// composer, and `Esc` cancels the prompt without closing the chat view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPrompt {
    /// Create a channel by name (`channel.create`).
    NewChannel { input: String },
    /// Join a running agent to the selected channel (`channel.join`).
    /// `candidates` is the unfiltered list; `query` narrows it and
    /// `selected` indexes the narrowed view.
    AddMember {
        query: String,
        selected: usize,
        candidates: Vec<ChatMemberCandidate>,
    },
}

/// TUI chat view presentation state (client layer). Channel data is fetched
/// through the channel JSON API (`channel.list` / `channel.history` /
/// `channel.members`) and cached here for render; live appends are pushed in
/// by the send path while the view is open. Nothing here is server-authoritative.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatViewState {
    /// Selected row in the channel list.
    pub selected: usize,
    /// Message-area scroll offset, in wrapped display lines from the top.
    pub scroll: usize,
    /// Compose buffer for the input line.
    pub input: String,
    /// Transient status line (send errors, hints); cleared on next success.
    pub status: Option<String>,
    /// Cached `channel.list` result.
    pub channels: Vec<crate::api::schema::ChannelSummary>,
    /// Cached `channel.history` for the selected channel.
    pub messages: Vec<crate::api::schema::ChannelMessage>,
    /// Cached `channel.members` for the selected channel.
    pub members: Vec<crate::api::schema::ChannelMember>,
    /// Human's own per-channel "last seq shown" cursor — client
    /// presentation state, keyed by normalized (hashless) channel name.
    /// Advanced only when the human actually views a room's transcript
    /// (see `App::refresh_chat_channel_data`); never persisted, never sent
    /// to the server. `channel.list` reads with no pane identity from this
    /// view, so the server always reports `unread: 0` for it — this cursor
    /// is what restores a real badge/sort signal for the human, distinct
    /// from an agent's own server-side read cursor.
    pub seen: std::collections::HashMap<String, u64>,
    /// Open modal sub-mode (new channel / add member), when any.
    pub prompt: Option<ChatPrompt>,
    /// Index of the timeline message currently rendered expanded (unclamped).
    /// Lives here rather than in the render path because `render()` is pure:
    /// it can only read this, never derive or mutate it — the input layer
    /// toggles it and requests the full repaint the reflow requires. Chat
    /// local view state, never sent to the server.
    pub expanded_message: Option<usize>,
    /// Cache of public pane id -> addressable display name, for messages'
    /// `to_pane` destinations. Populated at data-refresh time (channel
    /// switch, live append) by delegating to `App::pane_display_name` —
    /// the same #31 identity chain every other sender/addressee label
    /// uses — so render stays a pure, cheap per-frame/per-line pass with
    /// no identity re-derivation. Entries accumulate across channels and
    /// are never evicted; a stale mapping is harmless (worst case a pane
    /// that changed name shows its old one until next resolved), while
    /// eviction would require tracking staleness for no render benefit.
    pub to_names: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarWidthSource {
    ConfigDefault,
    Persisted,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneFocusTarget {
    pub workspace_id: String,
    pub pane_id: PaneId,
}

/// All application state — pure data, no channels or async runtime.
/// Testable without PTYs or a tokio runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarStatusSegment {
    Zoom,
    Text(Option<String>),
}

pub struct AppState {
    pub terminals:
        std::collections::HashMap<crate::terminal::TerminalId, crate::terminal::TerminalState>,
    /// Terminal ids whose size is currently owned by a direct attach client.
    pub direct_attach_resize_locks: std::collections::HashSet<crate::terminal::TerminalId>,
    pub(crate) pane_id_aliases: std::collections::HashMap<u32, PaneId>,
    pub(crate) public_pane_id_aliases: std::collections::HashMap<String, PaneId>,
    pub workspaces: Vec<Workspace>,
    pub active: Option<usize>,
    pub(crate) previous_pane_focus: Option<PaneFocusTarget>,
    pub selected: usize,
    pub mode: Mode,
    pub should_quit: bool,
    /// In monolithic --no-session mode, detach exits the app because there is no server to detach from.
    pub detach_exits: bool,
    /// Set when the current client should detach from the persistent session.
    /// The server's event loop checks this and handles client detach.
    pub detach_requested: bool,
    pub request_new_workspace: bool,
    pub request_new_tab: bool,
    pub request_new_linked_worktree: Option<usize>,
    pub request_open_existing_worktree: Option<usize>,
    pub request_new_workspace_cwd: Option<std::path::PathBuf>,
    pub request_remove_linked_worktree: Option<usize>,
    pub request_merge_worktree_to_main: Option<usize>,
    pub request_open_worktree_pr: Option<usize>,
    pub request_sync_workspace_git: Option<usize>,
    pub request_submit_worktree_create: bool,
    pub request_submit_worktree_open: bool,
    pub request_submit_worktree_remove: bool,
    pub request_submit_worktree_merge: bool,
    pub request_reload_config: bool,
    /// Set when the headless server should ask attached clients to reload
    /// their client-local sound config from disk.
    pub request_client_config_reload: bool,
    /// Set when UI interaction requested a clipboard write that must be
    /// handled by the outer App/event loop instead of directly from AppState.
    pub request_clipboard_write: Option<Vec<u8>>,
    /// Set when UI interaction asked to open a URL in the system browser.
    pub request_open_url: Option<String>,
    /// Set when a context-menu selection named a plugin action
    /// (`plugin_id.action_id`, bora-1e9): the App loop invokes it through
    /// `find_plugin_action`/`invoke_plugin_action_from_ui`, same as every
    /// other deferred App-owned action (dagr's old dedicated flag is gone —
    /// dagr is just another plugin action now).
    pub request_plugin_action: Option<String>,
    /// Set when UI interaction asked to open a PR in a new worktree:
    /// (representative workspace index of the repo group, PR number).
    pub request_open_pr_worktree: Option<(usize, u64)>,
    /// Set when UI interaction asked to run the configured flow command for
    /// a GitHub issue; drained by App to spawn the flow pane.
    pub request_flow_run: Option<FlowRunRequest>,
    /// Set when UI interaction asked to open the chat view; drained by App,
    /// which fetches channel data through the JSON API (mouse handlers stay
    /// side-effect-light).
    pub request_open_chat: bool,
    /// Set when the sidebar "+" affordance asked to open the Create worktree
    /// modal for a repo (by `repo_identity`); drained by App to trigger fetches
    /// and open the modal so the mouse handler stays side-effect-light.
    pub request_open_create_worktree: Option<String>,
    /// Set when a Project-view SectionRow's "+" was clicked (T4, bora-79l):
    /// `(repo_identity, branch)` of the clicked section — the branch-group
    /// pair, resolved to a source workspace only at drain time so a stale
    /// area after a re-render degrades to a no-op instead of creating in the
    /// wrong repo. Drained by App into `start_section_worktree_create`.
    pub request_section_worktree_create: Option<(String, String)>,
    pub pending_bora_command: Option<PendingBoraCommand>,
    /// Transient port override consumed by custom_command_env for pane commands.
    pub bora_port_override: Option<u16>,
    pub creating_new_tab: bool,
    pub requested_new_tab_name: Option<String>,
    pub pending_workspace_create_cwd: Option<std::path::PathBuf>,
    pub rename_pane_target: Option<PaneId>,
    /// What the `Mode::ProjectNameInput` modal acts on: rename of an
    /// existing project or creation of a new one (whose first member may be
    /// the dir the menu was opened from). `None` unless that mode is active.
    pub project_name_target: Option<ProjectNameTarget>,
    pub worktree_create: Option<WorktreeCreateState>,
    pub worktree_open: Option<WorktreeOpenState>,
    pub chat: ChatViewState,
    pub worktree_remove: Option<WorktreeRemoveState>,
    pub worktree_directory: std::path::PathBuf,
    /// Global `[flow]` command template from config.toml. Repos can override
    /// it via `[flow]` in their `.bora.toml`; see `repo_issue_flow_template`.
    pub flow_command_template: Option<String>,
    /// `[agents.commands]` overrides from config.toml, keyed by canonical
    /// agent id; `agent start` uses these to pick the executable it types
    /// into the target pane instead of the built-in canonical one.
    pub agent_commands: crate::config::AgentsConfig,
    pub collapsed_space_keys: std::collections::HashSet<String>,
    /// Sidebar-only, non-persisted: workspace/group keys temporarily hidden
    /// from the main list, mapped to the instant each hide expires.
    pub hidden_space_keys: std::collections::HashMap<String, std::time::Instant>,
    /// Whether the collapsible bottom "Hidden" section is expanded.
    pub hidden_section_expanded: bool,
    pub request_complete_onboarding: bool,
    pub name_input: String,
    pub name_input_replace_on_type: bool,
    pub release_notes: Option<ReleaseNotesState>,
    pub product_announcement: Option<ProductAnnouncementState>,
    pub keybind_help: KeybindHelpState,
    pub navigator: NavigatorState,
    pub copy_mode: Option<CopyModeState>,
    pub workspace_scroll: usize,
    pub agent_panel_scroll: usize,
    pub tab_scroll: usize,
    pub tab_scroll_follow_active: bool,
    pub mobile_switcher_scroll: usize,
    // View geometry (computed before render, consumed by render + mouse)
    pub view: ViewState,
    pub(crate) drag: Option<DragState>,
    pub(crate) workspace_presses:
        std::collections::HashMap<crate::app::InputSourceId, WorkspacePressState>,
    pub(crate) tab_presses: std::collections::HashMap<crate::app::InputSourceId, TabPressState>,
    pub selection: Option<Selection>,
    pub selection_autoscroll: Option<SelectionAutoscroll>,
    pub context_menu: Option<ContextMenuState>,
    // Notifications
    pub update_available: Option<String>,
    pub update_install_command: String,
    pub latest_release_notes_available: bool,
    pub update_dismissed: bool,
    pub config_diagnostic: Option<String>,
    pub toast: Option<ToastNotification>,
    pub pending_agent_notifications: std::collections::HashMap<PaneId, PendingAgentNotification>,
    pub copy_feedback: Option<CopyFeedback>,
    /// Last reported focus state for the outer terminal hosting herdr.
    /// None means unsupported or not yet reported, which preserves active-pane suppression.
    pub outer_terminal_focus: Option<bool>,
    // Config
    pub prefix_code: KeyCode,
    pub prefix_mods: KeyModifiers,
    /// Virtual terminal size (columns, rows) used when no client is attached.
    pub(crate) headless_size: (u16, u16),
    pub default_sidebar_width: u16,
    pub sidebar_width: u16,
    pub sidebar_min_width: u16,
    pub sidebar_max_width: u16,
    pub mobile_width_threshold: u16,
    pub sidebar_width_source: SidebarWidthSource,
    pub sidebar_width_auto: bool,
    pub sidebar_collapsed: bool,
    pub sidebar_collapsed_mode: crate::config::SidebarCollapsedModeConfig,
    /// Ratio of sidebar height allocated to the workspaces section.
    pub sidebar_section_split: f32,
    /// `projects.yml`, refreshed from the tick (never from render — the entry
    /// builder runs on a multiplicative path and must not touch the disk).
    /// The sidebar is its only reader; right-click, the editor, and MCP are
    /// the writers.
    pub projects: crate::persist::projects::ProjectsStore,
    /// Sidebar TODOS snapshot per project slug (bora-s3y.3), refreshed by
    /// `refresh_project_todos_notes` — called from the todo/scratchpad verb
    /// handlers after every mutation and when projects (re)load. Render only
    /// reads it: the stores are never touched on the render path.
    pub project_todos: std::collections::HashMap<String, crate::persist::todos::TodosSummary>,
    /// Sidebar NOTES snapshot per project slug: scratchpad doc names, same
    /// refresh discipline as `project_todos`.
    pub project_notes: std::collections::HashMap<String, Vec<String>>,
    pub right_panel_collapsed: bool,
    pub right_panel_width: u16,
    pub right_panel_min_width: u16,
    pub right_panel_max_width: u16,
    pub right_panel_active_tab: RightPanelTab,
    pub right_panel_scroll: u16,
    pub right_panel_selected_file: Option<(crate::workspace::ChangeSectionKind, String)>,
    /// Set by mouse click on a file row; drained by App to spawn gitui/diff pane.
    pub right_panel_diff_requested: bool,
    /// Set when Checks tab is activated; drained by App to call start_checks_fetch.
    pub right_panel_checks_requested: bool,
    /// Set when Issues tab is activated; drained by App to call start_issues_fetch.
    pub right_panel_issues_requested: bool,
    /// Set when the PRs tab is activated; drained by App to call start_open_prs_fetch.
    pub right_panel_prs_requested: bool,
    pub agent_panel_sort: AgentPanelSort,
    pub status_indicators: crate::config::StatusIndicatorStyle,
    /// Transient session-wide projection override for the built-in Agents view.
    pub agent_view_override: Option<crate::api::schema::AgentViewSetParams>,
    pub sidebar_agents: crate::config::AgentsSidebarConfig,
    pub sidebar_spaces: crate::config::SpacesSidebarConfig,
    /// Project-view row_gap + glyph style (bora-c1h), mirrors sidebar_agents/sidebar_spaces.
    pub sidebar_project: crate::config::ProjectSidebarConfig,
    pub next_agent_state_change_seq: u64,
    /// Capture mouse input for Herdr's own mouse UI. When false, Herdr only
    /// captures mouse while the focused pane app requests mouse reporting.
    pub mouse_capture: bool,
    pub copy_on_select: bool,
    pub right_click_passthrough_modifiers: Option<KeyModifiers>,
    pub right_click_passthrough: Option<RightClickPassthroughGesture>,
    pub redraw_on_focus_gained: bool,
    pub mouse_scroll_lines: usize,
    pub confirm_close: bool,
    pub prompt_new_tab_name: bool,
    pub prompt_new_workspace_name: bool,
    pub pane_borders: bool,
    pub pane_outer_borders: bool,
    pub pane_scrollbars: bool,
    pub pane_gaps: bool,
    pub show_agent_labels_on_pane_borders: bool,
    /// Sidebar workspace view mode (`ui.view_mode`, back-compat alias
    /// `ui.group_workspaces_by_repo`).
    pub(crate) view_mode: crate::config::ViewMode,
    pub show_pane_ids_on_pane_borders: bool,
    pub channel_group_name: String,
    /// Whether the fork-only chat view surface is enabled (`ui.chat_view`).
    pub chat_view: bool,
    /// Resolved human chat identity (`ui.chat_name`, else OS username, else
    /// "you"). One source of truth for the chat send path and the renderer.
    pub chat_name: String,
    /// `ui.channel_burst_messages` — see `channel_burst_window`.
    pub channel_burst_messages: u32,
    /// `ui.channel_burst_window_secs`, as a `Duration`. Together with
    /// `channel_burst_messages`, defines the `channel.send` burst damper
    /// (`App::record_channel_burst_send` in `app::api::channels`).
    pub channel_burst_window: std::time::Duration,
    pub hide_tab_bar_when_single_tab: bool,
    pub tab_bar_position: TabBarPositionConfig,
    pub tab_bar_right: Vec<TabBarStatusSegment>,
    pub tab_bar_right_separator: String,
    pub pane_history_persistence: bool,
    /// Expose the focused pane's cursor anchor to the outer terminal even when
    /// the pane requested `?25l`. See `[experimental] reveal_hidden_cursor_for_cjk_ime`.
    pub reveal_hidden_cursor_for_cjk_ime: bool,
    /// Restrict cursor reveal to focused panes whose detected agent matches
    /// one of these. When false, apply to any focused pane.
    pub cjk_ime_agent_filter_configured: bool,
    pub cjk_ime_agents: Vec<crate::detect::Agent>,
    /// DECSCUSR shape parameter (1–6) for the IME anchor cursor.
    pub cjk_ime_cursor_shape: u8,
    /// While prefix mode is active, switch the macOS host input source to an
    /// ASCII-capable layout so prefix commands register as ASCII even when a
    /// CJK IME is active. macOS only; a no-op elsewhere. See
    /// `[experimental] switch_ascii_input_source_in_prefix`.
    pub switch_ascii_input_source_in_prefix: bool,
    pub kitty_graphics_enabled: bool,
    pub default_shell: String,
    pub shell_mode: crate::config::ShellModeConfig,
    pub new_terminal_cwd: NewTerminalCwdConfig,
    pub pane_scrollback_limit_bytes: usize,
    #[allow(dead_code)] // kept for backward compat; palette.accent is the source of truth
    pub accent: Color,
    pub sound: SoundConfig,
    pub local_sound_playback: bool,
    pub toast_config: ToastConfig,
    pub keybinds: Keybinds,
    /// Frame counter for spinner animations (wraps around).
    pub spinner_tick: u32,
    /// UI color palette — all sidebar/UI colors centralized for theming.
    pub palette: Palette,
    /// Currently applied theme name (for settings UI).
    pub theme_name: String,
    /// Runtime theme configuration used to resolve manual and auto-switch palettes.
    pub theme_runtime: ThemeRuntimeConfig,
    /// Last known foreground host terminal appearance.
    pub host_terminal_appearance: Option<HostAppearance>,
    /// True when the foreground host explicitly reported appearance via Mode 2031.
    pub host_terminal_appearance_explicit: bool,
    /// Settings panel state.
    pub settings: SettingsState,
    /// Cached integration recommendations for onboarding/settings UI.
    pub integration_recommendations: Vec<crate::integration::IntegrationRecommendation>,
    /// Cached detection manifest source/version summaries for runtime/API status.
    pub agent_manifest_summaries: Vec<crate::detect::manifest::AgentManifestSummary>,
    /// Cached remote detection manifest update diagnostics for runtime/API status.
    pub agent_manifest_update_status: crate::detect::manifest_update::ManifestUpdateStatus,
    /// Result messages from the latest integration install action.
    pub integration_install_messages: Vec<String>,
    /// Installed or linked plugins known to this running Herdr instance.
    pub(crate) installed_plugins: InstalledPluginRegistry,
    /// Pane ids opened through the plugin pane API.
    pub(crate) plugin_panes: std::collections::HashMap<PaneId, PluginPaneRecord>,
    /// Session-modal terminal popup. This is intentionally outside workspace layouts.
    pub(crate) popup_pane: Option<PopupPaneState>,
    /// Recent plugin action/event command executions.
    pub(crate) plugin_command_logs: Vec<crate::api::schema::PluginCommandLogInfo>,
    pub(crate) next_plugin_command_log_id: u64,
    pub(crate) plugin_commands_in_flight: usize,
    /// Highlight state for the bottom-right global launcher menu.
    pub global_menu: MenuListState,
    /// Resolved host terminal default colors for theming embedded panes.
    pub host_terminal_theme: TerminalTheme,
    /// Last known foreground host terminal cell size in pixels.
    pub(crate) host_cell_size: crate::kitty_graphics::HostCellSize,
    /// Exact pixel provenance only while one confirmed SGR report is dispatched.
    pub(crate) host_mouse_pixels: Option<crate::input::mouse::HostPixels>,
    /// Set when a persisted session snapshot would change.
    pub session_dirty: bool,
    /// Cached open PRs authored by the current user, keyed by repo identity
    /// (`GitSpaceMetadata.repo_identity`). Written by the periodic background
    /// refresh; read by UI/API surfaces in later phases.
    pub repo_open_prs: std::collections::HashMap<String, crate::workspace::RepoOpenPrs>,
    /// Cached `git worktree list` result per repo identity
    /// (`GitSpaceMetadata.repo_identity`), for the Project view's unopened
    /// worktree rows (bora-qdi). Written by
    /// `App::start_worktree_inventory_refresh_if_due`'s throttled background
    /// thread (`src/app/runtime.rs`), once per repo that some declared
    /// project member resolves to with `WorktreesScope::All`. Read only by
    /// `ui::sidebar::project_view::push_project_group`, which performs no
    /// I/O of its own on the render path.
    pub(crate) worktree_inventory: std::collections::HashMap<String, RepoWorktreeInventory>,
    /// Cached open issues relevant to the current user, keyed by repo identity
    /// (`GitSpaceMetadata.repo_identity`). Written by on-demand background
    /// fetches; read by UI/API surfaces in later phases.
    pub repo_issues: std::collections::HashMap<String, crate::workspace::RepoIssues>,
    /// Repo identities with an issues fetch currently in flight. Guards
    /// against overlapping fetches from rapid tab toggling and lets the
    /// Issues tab render a loading state; cleared on `RepoIssuesRefreshed`.
    pub issues_fetch_in_flight: std::collections::HashSet<String>,
    /// Repo identities with an on-demand open-PR fetch currently in flight.
    /// Guards against overlapping fetches and lets the Create worktree modal's
    /// GitHub tab render a loading state; cleared on `RepoPrsRefreshed`.
    pub prs_fetch_in_flight: std::collections::HashSet<String>,
    /// Cached local branches per repo identity
    /// (`GitSpaceMetadata.repo_identity`). Written by on-demand background
    /// fetches; read by the Create worktree modal's Branch tab.
    pub repo_branches: std::collections::HashMap<String, crate::workspace::RepoBranches>,
    /// Repo identities with a branch fetch currently in flight. Guards against
    /// overlapping fetches; cleared on `RepoBranchesRefreshed`.
    pub branches_fetch_in_flight: std::collections::HashSet<String>,
    /// Terminal runtimes that should be shut down by the app/runtime layer
    /// after state has detached their terminal metadata.
    pub(crate) terminal_runtime_shutdowns: Vec<crate::terminal::TerminalId>,
    /// Set when a layout change (e.g. sidebar/right-panel toggle) reflows
    /// pane content without changing the outer terminal's cols/rows. Bridged
    /// into a per-client repaint request by the headless render loop, since
    /// dimension-keyed full-repaint heuristics would otherwise miss it.
    pub(crate) force_full_repaint: bool,
}

impl AppState {
    pub(crate) fn mark_session_dirty(&mut self) {
        self.session_dirty = true;
    }

    pub(crate) fn request_full_repaint(&mut self) {
        self.force_full_repaint = true;
    }
    /// Reload the sidebar's TODOS/NOTES snapshots for `slug` from the stores
    /// (bora-s3y.3). Callers: the six todo/scratchpad verb handlers
    /// (post-mutation) and the projects reload path — never render, so the
    /// two store reads stay off the multiplicative path.
    pub(crate) fn refresh_project_todos_notes(&mut self, slug: &str) {
        let todos = crate::persist::todos::read_todos(slug).unwrap_or_default();
        self.project_todos.insert(
            slug.to_string(),
            crate::persist::todos::TodosSummary::from_todos(&todos),
        );
        let notes = crate::persist::scratchpads::list_docs(slug).unwrap_or_default();
        self.project_notes.insert(slug.to_string(), notes);
    }

    /// Sidebar hide key for a single workspace (non-persisted presentation state).
    pub(crate) fn workspace_hide_key(ws: &crate::workspace::Workspace) -> String {
        format!("ws:{}", ws.id)
    }

    /// Whether `key` is currently hidden: present and not yet expired.
    pub(crate) fn is_hidden(&self, key: &str) -> bool {
        self.hidden_space_keys
            .get(key)
            .is_some_and(|expiry| *expiry > std::time::Instant::now())
    }

    /// Earliest hide expiry, if any, for scheduling a render wakeup.
    pub(crate) fn next_hide_expiry(&self) -> Option<std::time::Instant> {
        self.hidden_space_keys.values().copied().min()
    }

    /// Drop hides whose expiry has passed. Returns whether anything changed.
    pub(crate) fn sweep_expired_hides(&mut self, now: std::time::Instant) -> bool {
        let before = self.hidden_space_keys.len();
        self.hidden_space_keys.retain(|_, expiry| *expiry > now);
        self.hidden_space_keys.len() != before
    }

    pub(crate) fn remove_alias_shadowed_by_new_pane(&mut self, pane_id: PaneId) {
        self.pane_id_aliases.remove(&pane_id.raw());
    }

    pub fn sound_enabled(&self) -> bool {
        self.sound.enabled
    }

    pub fn toast_delivery(&self) -> ToastDelivery {
        self.toast_config.delivery
    }

    pub fn agent_border_labels_enabled(&self) -> bool {
        self.show_agent_labels_on_pane_borders
    }

    pub fn view_mode(&self) -> crate::config::ViewMode {
        self.view_mode
    }

    /// True for any mode that visually groups workspaces (`Repo` and
    /// `Project` — `Project` renders like `Repo` until bora-49p.3 lands its
    /// own entry model). Only `Flat` disables grouping.
    pub(crate) fn groups_workspaces(&self) -> bool {
        self.view_mode != crate::config::ViewMode::Flat
    }

    pub(crate) fn pane_exposes_host_cursor(
        &self,
        _ws_idx: usize,
        _pane_id: crate::layout::PaneId,
    ) -> bool {
        true
    }

    pub(crate) fn integration_updates_available(&self) -> bool {
        self.integration_recommendations
            .iter()
            .any(|item| item.state == crate::integration::IntegrationStatusKind::Outdated)
    }

    pub(crate) fn refresh_agent_manifest_summaries(&mut self) {
        self.agent_manifest_summaries = crate::detect::manifest::manifest_summaries();
    }

    pub(crate) fn global_menu_attention_badge_visible(&self) -> bool {
        self.update_available.is_some() || self.integration_updates_available()
    }

    pub(crate) fn global_menu_item_has_badge(&self, item: &str) -> bool {
        (item == "update ready" && self.update_available.is_some())
            || (item == "settings" && self.integration_updates_available())
    }

    pub(crate) fn settings_section_has_badge(&self, section: SettingsSection) -> bool {
        section == SettingsSection::Integrations && self.integration_updates_available()
    }

    pub(crate) fn app_surface_pane_ids(&self) -> std::collections::HashSet<PaneId> {
        let mut pane_ids = std::collections::HashSet::new();
        if let Some(popup) = &self.popup_pane {
            pane_ids.insert(popup.pane_id);
        }
        let Some(tab) = self
            .active
            .and_then(|ws_idx| self.workspaces.get(ws_idx))
            .and_then(crate::workspace::Workspace::active_tab)
        else {
            return pane_ids;
        };
        if tab.zoomed {
            pane_ids.insert(tab.layout.focused());
        } else {
            pane_ids.extend(tab.panes.keys().copied());
        }
        pane_ids
    }

    pub(crate) fn focused_pane_requests_mouse_capture_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        self.mode == Mode::Terminal
            && self
                .active
                .and_then(|idx| self.focused_runtime_in_workspace(terminal_runtimes, idx))
                .is_some_and(crate::terminal::TerminalRuntime::mouse_reporting_enabled)
    }

    pub(crate) fn should_capture_host_mouse_from(
        &self,
        terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ) -> bool {
        self.mouse_capture
            || self.popup_pane.is_some()
            || self.focused_pane_requests_mouse_capture_from(terminal_runtimes)
    }

    pub fn is_prefix_key(&self, key: &crate::input::TerminalKey) -> bool {
        crate::config::terminal_key_matches_combo(key, (self.prefix_code, self.prefix_mods))
    }

    pub fn estimate_pane_size(&self) -> (u16, u16) {
        if let Some(info) = self.view.pane_infos.first() {
            (info.rect.height, info.rect.width)
        } else {
            (self.headless_size.1, self.headless_size.0)
        }
    }

    /// Returns true when the given (workspace, tab, pane) refers to the
    /// currently focused pane in the active workspace's active tab.
    pub(crate) fn runtime_for_pane_in_workspace<'a>(
        &'a self,
        terminal_runtimes: &'a crate::terminal::TerminalRuntimeRegistry,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&'a crate::terminal::TerminalRuntime> {
        #[cfg(test)]
        if let Some(runtime) = self.workspaces.get(ws_idx)?.test_runtimes.get(&pane_id) {
            return Some(runtime);
        }
        #[cfg(test)]
        if let Some(runtime) = self
            .workspaces
            .get(ws_idx)?
            .tabs
            .iter()
            .find_map(|tab| tab.runtimes.get(&pane_id))
        {
            return Some(runtime);
        }
        let terminal_id = self.workspaces.get(ws_idx)?.terminal_id(pane_id)?;
        terminal_runtimes.get(terminal_id)
    }

    #[cfg(test)]
    pub(crate) fn runtime_for_pane<'a>(
        &'a self,
        terminal_runtimes: &'a crate::terminal::TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
    ) -> Option<&'a crate::terminal::TerminalRuntime> {
        self.workspaces.iter().find_map(|ws| {
            #[cfg(test)]
            if let Some(runtime) = ws.test_runtimes.get(&pane_id) {
                return Some(runtime);
            }
            #[cfg(test)]
            if let Some(runtime) = ws.tabs.iter().find_map(|tab| tab.runtimes.get(&pane_id)) {
                return Some(runtime);
            }
            let terminal_id = ws.terminal_id(pane_id)?;
            terminal_runtimes.get(terminal_id)
        })
    }

    pub(crate) fn focused_runtime_in_workspace<'a>(
        &'a self,
        terminal_runtimes: &'a crate::terminal::TerminalRuntimeRegistry,
        ws_idx: usize,
    ) -> Option<&'a crate::terminal::TerminalRuntime> {
        let ws = self.workspaces.get(ws_idx)?;
        let pane_id = ws.focused_pane_id()?;
        self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
    }

    pub fn is_active_pane(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> bool {
        let Some(active_ws_idx) = self.active else {
            return false;
        };
        if ws_idx != active_ws_idx {
            return false;
        }
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return false;
        };
        if tab_idx != ws.active_tab_index() {
            return false;
        }
        ws.active_tab().map(|tab| tab.layout.focused()) == Some(pane_id)
    }
}

#[cfg(test)]
pub fn key_matches(
    key: &crossterm::event::KeyEvent,
    expected_code: KeyCode,
    expected_mods: KeyModifiers,
) -> bool {
    crate::config::terminal_key_matches_combo(
        &crate::input::TerminalKey::from(*key),
        (expected_code, expected_mods),
    )
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
impl AppState {
    /// Create an AppState for testing — no channels, no PTYs.
    pub fn test_new() -> Self {
        Self {
            terminals: std::collections::HashMap::new(),
            direct_attach_resize_locks: std::collections::HashSet::new(),
            pane_id_aliases: std::collections::HashMap::new(),
            public_pane_id_aliases: std::collections::HashMap::new(),
            workspaces: Vec::new(),
            active: None,
            previous_pane_focus: None,
            selected: 0,
            mode: Mode::Navigate,
            should_quit: false,
            detach_exits: false,
            detach_requested: false,
            request_new_workspace: false,
            request_new_tab: false,
            request_new_linked_worktree: None,
            request_open_existing_worktree: None,
            request_new_workspace_cwd: None,
            request_remove_linked_worktree: None,
            request_merge_worktree_to_main: None,
            request_open_worktree_pr: None,
            request_sync_workspace_git: None,
            request_submit_worktree_create: false,
            request_submit_worktree_open: false,
            request_submit_worktree_remove: false,
            request_submit_worktree_merge: false,
            request_reload_config: false,
            request_plugin_action: None,
            request_client_config_reload: false,
            request_clipboard_write: None,
            request_open_url: None,
            request_open_pr_worktree: None,
            request_flow_run: None,
            request_open_create_worktree: None,
            request_section_worktree_create: None,
            pending_bora_command: None,
            bora_port_override: None,
            creating_new_tab: false,
            requested_new_tab_name: None,
            pending_workspace_create_cwd: None,
            rename_pane_target: None,
            project_name_target: None,
            worktree_create: None,
            worktree_open: None,
            worktree_remove: None,
            worktree_directory: std::path::PathBuf::from("/tmp/herdr-worktrees"),
            flow_command_template: None,
            agent_commands: crate::config::AgentsConfig::default(),
            collapsed_space_keys: std::collections::HashSet::new(),
            hidden_space_keys: std::collections::HashMap::new(),
            hidden_section_expanded: false,
            request_complete_onboarding: false,
            name_input: String::new(),
            name_input_replace_on_type: false,
            release_notes: None,
            product_announcement: None,
            keybind_help: KeybindHelpState::default(),
            navigator: NavigatorState::default(),
            copy_mode: None,
            workspace_scroll: 0,
            agent_panel_scroll: 0,
            tab_scroll: 0,
            tab_scroll_follow_active: true,
            mobile_switcher_scroll: 0,
            view: ViewState {
                layout: ViewLayout::Desktop,
                sidebar_rect: Rect::default(),
                workspace_card_areas: Vec::new(),
                workspace_group_header_areas: Vec::new(),
                project_row_areas: Vec::new(),
                worktree_new_hit_areas: Vec::new(),
                tab_bar_rect: Rect::default(),
                tab_hit_areas: Vec::new(),
                tab_scroll_left_hit_area: Rect::default(),
                tab_scroll_right_hit_area: Rect::default(),
                new_tab_hit_area: Rect::default(),
                terminal_area: Rect::default(),
                mobile_header_rect: Rect::default(),
                mobile_menu_hit_area: Rect::default(),
                mobile_prev_tab_hit_area: Rect::default(),
                mobile_next_tab_hit_area: Rect::default(),
                toast_hit_area: Rect::default(),
                pane_infos: Vec::new(),
                split_borders: Vec::new(),
                right_panel_rect: Rect::default(),
            },
            chat: ChatViewState::default(),
            request_open_chat: false,
            drag: None,
            workspace_presses: std::collections::HashMap::new(),
            tab_presses: std::collections::HashMap::new(),
            selection: None,
            selection_autoscroll: None,
            context_menu: None,
            update_available: None,
            update_install_command: "bora update".into(),
            latest_release_notes_available: false,
            update_dismissed: false,
            config_diagnostic: None,
            toast: None,
            pending_agent_notifications: std::collections::HashMap::new(),
            copy_feedback: None,
            outer_terminal_focus: None,
            prefix_code: KeyCode::Char('b'),
            prefix_mods: KeyModifiers::CONTROL,
            headless_size: (
                crate::config::DEFAULT_HEADLESS_COLS,
                crate::config::DEFAULT_HEADLESS_ROWS,
            ),
            default_sidebar_width: 26,
            sidebar_width: 26,
            sidebar_min_width: 18,
            sidebar_max_width: 36,
            mobile_width_threshold: crate::config::DEFAULT_MOBILE_WIDTH_THRESHOLD,
            sidebar_width_source: SidebarWidthSource::ConfigDefault,
            sidebar_width_auto: false,
            sidebar_collapsed: false,
            sidebar_collapsed_mode: crate::config::SidebarCollapsedModeConfig::Compact,
            sidebar_section_split: 0.5,
            // Mirrors the agent-manifest idiom in `App::new`: unit tests get an
            // inert store so `test_new()` never reads the operator's real
            // `~/.config/bora/projects.yml`.
            #[cfg(not(test))]
            projects: crate::persist::projects::ProjectsStore::load(),
            #[cfg(test)]
            projects: crate::persist::projects::ProjectsStore::empty(),
            project_todos: std::collections::HashMap::new(),
            project_notes: std::collections::HashMap::new(),
            right_panel_collapsed: true,
            right_panel_width: 30,
            right_panel_min_width: 20,
            right_panel_max_width: 50,
            right_panel_active_tab: RightPanelTab::default(),
            right_panel_scroll: 0,
            right_panel_selected_file: None,
            right_panel_diff_requested: false,
            right_panel_checks_requested: false,
            right_panel_issues_requested: false,
            right_panel_prs_requested: false,
            agent_panel_sort: AgentPanelSort::Spaces,
            status_indicators: crate::config::StatusIndicatorStyle::Dots,
            agent_view_override: None,
            sidebar_agents: crate::config::AgentsSidebarConfig::default(),
            sidebar_spaces: crate::config::SpacesSidebarConfig::default(),
            sidebar_project: crate::config::ProjectSidebarConfig::default(),
            next_agent_state_change_seq: 0,
            mouse_capture: true,
            copy_on_select: true,
            right_click_passthrough_modifiers: None,
            right_click_passthrough: None,
            redraw_on_focus_gained: true,
            mouse_scroll_lines: crate::config::DEFAULT_MOUSE_SCROLL_LINES,
            confirm_close: true,
            prompt_new_tab_name: true,
            prompt_new_workspace_name: false,
            pane_borders: true,
            pane_outer_borders: true,
            pane_scrollbars: true,
            pane_gaps: false,
            show_agent_labels_on_pane_borders: false,
            view_mode: crate::config::ViewMode::Repo,
            show_pane_ids_on_pane_borders: false,
            channel_group_name: "channels".to_string(),
            chat_view: false,
            chat_name: "you".to_string(),
            channel_burst_messages: 8,
            channel_burst_window: std::time::Duration::from_secs(600),
            hide_tab_bar_when_single_tab: false,
            tab_bar_position: TabBarPositionConfig::Top,
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: " ".into(),
            pane_history_persistence: false,
            reveal_hidden_cursor_for_cjk_ime: false,
            cjk_ime_agent_filter_configured: false,
            cjk_ime_agents: Vec::new(),
            cjk_ime_cursor_shape: 2, // steady_block
            switch_ascii_input_source_in_prefix: false,
            kitty_graphics_enabled: false,
            default_shell: String::new(),
            shell_mode: crate::config::ShellModeConfig::Auto,
            new_terminal_cwd: NewTerminalCwdConfig::Follow,
            pane_scrollback_limit_bytes: crate::config::DEFAULT_SCROLLBACK_LIMIT_BYTES,
            accent: Color::Cyan,
            sound: SoundConfig {
                enabled: false,
                ..SoundConfig::default()
            },
            local_sound_playback: false,
            toast_config: ToastConfig::default(),
            keybinds: Keybinds::default(),
            spinner_tick: 0,
            palette: Palette::catppuccin(),
            theme_name: "catppuccin".to_string(),
            theme_runtime: ThemeRuntimeConfig {
                manual_name: "catppuccin".to_string(),
                dark_name: "catppuccin".to_string(),
                light_name: "catppuccin-latte".to_string(),
                auto_switch: false,
                custom: None,
                legacy_accent: None,
            },
            host_terminal_appearance: None,
            host_terminal_appearance_explicit: false,
            settings: SettingsState {
                section: SettingsSection::Theme,
                list: SelectionListState::new(0),
                original_palette: None,
                original_theme: None,
            },
            integration_recommendations: Vec::new(),
            agent_manifest_summaries: Vec::new(),
            agent_manifest_update_status:
                crate::detect::manifest_update::ManifestUpdateStatus::default(),
            integration_install_messages: Vec::new(),
            installed_plugins: std::collections::HashMap::new(),
            plugin_panes: std::collections::HashMap::new(),
            popup_pane: None,
            plugin_command_logs: Vec::new(),
            next_plugin_command_log_id: 1,
            plugin_commands_in_flight: 0,
            global_menu: MenuListState::new(0),
            host_terminal_theme: TerminalTheme::default(),
            host_cell_size: crate::kitty_graphics::HostCellSize::default(),
            host_mouse_pixels: None,
            session_dirty: false,
            repo_open_prs: std::collections::HashMap::new(),
            worktree_inventory: std::collections::HashMap::new(),
            repo_issues: std::collections::HashMap::new(),
            issues_fetch_in_flight: std::collections::HashSet::new(),
            prs_fetch_in_flight: std::collections::HashSet::new(),
            repo_branches: std::collections::HashMap::new(),
            branches_fetch_in_flight: std::collections::HashSet::new(),
            terminal_runtime_shutdowns: Vec::new(),
            force_full_repaint: false,
        }
    }

    /// Populate missing `TerminalState` entries for every pane so tests that
    /// read or write terminal metadata don't need to manually create them.
    pub fn ensure_test_terminals(&mut self) {
        use crate::terminal::TerminalState;
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                for pane in tab.panes.values() {
                    if !self.terminals.contains_key(&pane.attached_terminal_id) {
                        let cwd = ws.identity_cwd.clone();
                        self.terminals.insert(
                            pane.attached_terminal_id.clone(),
                            TerminalState::new(pane.attached_terminal_id.clone(), cwd),
                        );
                    }
                }
            }
        }
    }

    pub fn test_with_adversarial_identity_state() -> Self {
        let mut state = Self::test_new();
        state.workspaces = vec![crate::workspace::Workspace::test_adversarial_identity_state()];
        state.active = Some(0);
        state.selected = 0;
        state.ensure_test_terminals();
        state
    }

    pub fn assert_invariants_for_test(&self) {
        if self.workspaces.is_empty() {
            assert!(
                self.active.is_none(),
                "empty app state must not have active workspace {:?}",
                self.active
            );
            assert_eq!(
                self.selected, 0,
                "empty app state should keep selected workspace at 0"
            );
            assert!(
                self.pane_id_aliases.is_empty(),
                "empty app state must not keep raw pane aliases"
            );
            assert!(
                self.public_pane_id_aliases.is_empty(),
                "empty app state must not keep public pane aliases"
            );
            assert!(
                self.previous_pane_focus.is_none(),
                "empty app state must not keep previous pane focus"
            );
            assert!(
                self.plugin_panes.is_empty(),
                "empty app state must not keep plugin pane records"
            );
            assert!(
                self.pending_agent_notifications.is_empty(),
                "empty app state must not keep pending agent notifications"
            );
            assert!(
                self.copy_mode.is_none(),
                "empty app state must not keep copy mode"
            );
            assert!(
                self.rename_pane_target.is_none(),
                "empty app state must not keep rename pane target"
            );
            assert!(
                self.selection.is_none(),
                "empty app state must not keep text selection"
            );
            assert!(
                self.selection_autoscroll.is_none(),
                "empty app state must not keep selection autoscroll"
            );
            if let Some(toast) = &self.toast {
                assert!(
                    toast.target.is_none(),
                    "empty app state must not keep pane-targeted toast"
                );
            }
            assert!(
                self.right_click_passthrough.is_none(),
                "empty app state must not keep right-click passthrough gesture"
            );
            assert!(
                self.drag.is_none(),
                "empty app state must not keep drag state"
            );
            assert!(
                self.workspace_presses.is_empty(),
                "empty app state must not keep workspace press state"
            );
            assert!(
                self.tab_presses.is_empty(),
                "empty app state must not keep tab press state"
            );
            assert!(
                self.context_menu.is_none(),
                "empty app state must not keep context menu"
            );
            assert!(
                self.host_mouse_pixels.is_none(),
                "empty app state must not keep host mouse pixel provenance"
            );
            return;
        }

        assert!(
            self.selected < self.workspaces.len(),
            "selected workspace {} out of bounds for {} workspaces",
            self.selected,
            self.workspaces.len()
        );
        let active = self
            .active
            .expect("non-empty app state must have active workspace");
        assert!(
            active < self.workspaces.len(),
            "active workspace {} out of bounds for {} workspaces",
            active,
            self.workspaces.len()
        );

        let mut workspace_ids = std::collections::HashSet::new();
        let mut workspace_id_to_idx = std::collections::HashMap::new();
        let mut pane_ids = std::collections::HashSet::new();
        let mut attached_terminal_ids = std::collections::HashSet::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            assert!(
                workspace_ids.insert(ws.id.clone()),
                "duplicate workspace id {} at workspace index {}",
                ws.id,
                ws_idx
            );
            workspace_id_to_idx.insert(ws.id.clone(), ws_idx);
            ws.assert_invariants_for_test();

            for tab in &ws.tabs {
                for (pane_id, pane) in &tab.panes {
                    assert!(
                        pane_ids.insert(*pane_id),
                        "pane {:?} appears in more than one workspace",
                        pane_id
                    );
                    assert!(
                        attached_terminal_ids.insert(pane.attached_terminal_id.clone()),
                        "terminal {} is attached to more than one app pane",
                        pane.attached_terminal_id
                    );
                    assert!(
                        self.terminals.contains_key(&pane.attached_terminal_id),
                        "pane {:?} is attached to missing terminal {}",
                        pane_id,
                        pane.attached_terminal_id
                    );
                }
            }
        }

        let assert_live_pane = |pane_id: PaneId, context: &str| {
            assert!(
                pane_ids.contains(&pane_id),
                "{context} references missing pane {:?}",
                pane_id
            );
        };
        let assert_workspace_pane = |workspace_id: &str, pane_id: PaneId, context: &str| {
            let ws_idx = workspace_id_to_idx
                .get(workspace_id)
                .copied()
                .unwrap_or_else(|| panic!("{context} references missing workspace {workspace_id}"));
            assert!(
                self.workspaces[ws_idx].pane_state(pane_id).is_some(),
                "{context} references pane {:?} outside workspace {}",
                pane_id,
                workspace_id
            );
        };
        let assert_workspace_index = |ws_idx: usize, context: &str| {
            assert!(
                ws_idx < self.workspaces.len(),
                "{context} references workspace index {} out of bounds for {} workspaces",
                ws_idx,
                self.workspaces.len()
            );
        };
        let assert_tab_index = |ws_idx: usize, tab_idx: usize, context: &str| {
            assert_workspace_index(ws_idx, context);
            assert!(
                tab_idx < self.workspaces[ws_idx].tabs.len(),
                "{context} references tab index {} out of bounds for workspace {} with {} tabs",
                tab_idx,
                ws_idx,
                self.workspaces[ws_idx].tabs.len()
            );
        };

        for (&raw, &pane_id) in &self.pane_id_aliases {
            assert_live_pane(pane_id, &format!("raw pane alias {raw}"));
        }
        for (public_id, &pane_id) in &self.public_pane_id_aliases {
            assert_live_pane(pane_id, &format!("public pane alias {public_id}"));
        }
        if let Some(focus) = &self.previous_pane_focus {
            assert_workspace_pane(&focus.workspace_id, focus.pane_id, "previous pane focus");
        }
        if let Some(toast) = &self.toast {
            if let Some(target) = &toast.target {
                assert_workspace_pane(&target.workspace_id, target.pane_id, "toast target");
            }
        }
        for (&pane_id, notification) in &self.pending_agent_notifications {
            assert_eq!(
                pane_id, notification.pane_id,
                "pending agent notification map key must match payload pane id"
            );
            assert_workspace_pane(
                &notification.workspace_id,
                notification.pane_id,
                "pending agent notification",
            );
        }
        if let Some(popup) = &self.popup_pane {
            assert!(
                self.terminals.contains_key(&popup.terminal_id),
                "popup {:?} references missing terminal {}",
                popup.pane_id,
                popup.terminal_id
            );
            assert!(
                !attached_terminal_ids.contains(&popup.terminal_id),
                "popup terminal {} must not be attached to a tiled pane",
                popup.terminal_id
            );
        }
        for &pane_id in self.plugin_panes.keys() {
            assert_live_pane(pane_id, "plugin pane record");
        }
        if let Some(copy_mode) = &self.copy_mode {
            assert_live_pane(copy_mode.pane_id, "copy mode");
        }
        if let Some(pane_id) = self.rename_pane_target {
            assert_live_pane(pane_id, "rename pane target");
        }
        if let Some(selection) = &self.selection {
            assert_live_pane(selection.pane_id, "text selection");
        } else {
            assert!(
                self.selection_autoscroll.is_none(),
                "selection autoscroll must not remain without an active text selection"
            );
        }
        if let Some(gesture) = &self.right_click_passthrough {
            assert_live_pane(gesture.pane_info.id, "right-click passthrough gesture");
        }
        if let Some(drag) = &self.drag {
            match &drag.target {
                DragTarget::WorkspaceReorder {
                    source_ws_idx,
                    insert_idx,
                    ..
                } => {
                    assert_workspace_index(*source_ws_idx, "workspace drag source");
                    if let Some(insert_idx) = insert_idx {
                        assert!(
                            *insert_idx <= self.workspaces.len(),
                            "workspace drag insert index {} out of bounds for {} workspaces",
                            insert_idx,
                            self.workspaces.len()
                        );
                    }
                }
                DragTarget::TabReorder {
                    ws_idx,
                    source_tab_idx,
                    insert_idx,
                    ..
                } => {
                    assert_tab_index(*ws_idx, *source_tab_idx, "tab drag source");
                    if let Some(insert_idx) = insert_idx {
                        assert!(
                            *insert_idx <= self.workspaces[*ws_idx].tabs.len(),
                            "tab drag insert index {} out of bounds for workspace {} with {} tabs",
                            insert_idx,
                            ws_idx,
                            self.workspaces[*ws_idx].tabs.len()
                        );
                    }
                }
                DragTarget::PaneScrollbar { pane_id, .. } => {
                    assert_live_pane(*pane_id, "pane scrollbar drag")
                }
                _ => {}
            }
        }
        for press in self.workspace_presses.values() {
            assert_workspace_index(press.ws_idx, "workspace press");
        }
        for press in self.tab_presses.values() {
            assert_tab_index(press.ws_idx, press.tab_idx, "tab press");
        }
        if let Some(menu) = &self.context_menu {
            match menu.kind {
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. } => {
                    assert_workspace_index(ws_idx, "context menu workspace")
                }
                ContextMenuKind::Tab { ws_idx, tab_idx } => {
                    assert_tab_index(ws_idx, tab_idx, "context menu tab")
                }
                ContextMenuKind::Pane {
                    ws_idx,
                    tab_idx,
                    pane_id,
                    source_pane_id,
                    ..
                } => {
                    assert_tab_index(ws_idx, tab_idx, "context menu pane tab");
                    assert!(
                        self.workspaces[ws_idx].tabs[tab_idx]
                            .panes
                            .contains_key(&pane_id),
                        "context menu pane references pane {:?} outside workspace {} tab {}",
                        pane_id,
                        ws_idx,
                        tab_idx
                    );
                    if let Some(source_pane_id) = source_pane_id {
                        assert_live_pane(source_pane_id, "context menu source pane");
                    }
                }
                ContextMenuKind::RepoPr { ws_idx, .. } => {
                    assert_workspace_index(ws_idx, "context menu repo pr")
                }
                // No index to check — the menu carries only the issue number/URL.
                ContextMenuKind::RepoIssue { .. } => {}
                // No workspace index to check — a group header carries only keys.
                ContextMenuKind::GroupHeader { .. } => {}
                // No workspace index to check — the project-assembly kinds
                // carry only slugs, keys, and dirs.
                ContextMenuKind::ProjectHeader { .. }
                | ContextMenuKind::ProjectOrphanPicker { .. }
                | ContextMenuKind::ProjectMemberTargets { .. } => {}
            }
        }
    }

    pub fn insert_test_runtime(
        &mut self,
        pane_id: crate::layout::PaneId,
        runtime: crate::terminal::TerminalRuntime,
    ) {
        if let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|ws| ws.terminal_id(pane_id).is_some())
        {
            ws.insert_test_runtime(pane_id, runtime);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    #[test]
    fn pane_size_estimate_uses_headless_size_before_first_view() {
        let mut state = AppState::test_new();
        state.headless_size = (132, 41);

        assert_eq!(state.estimate_pane_size(), (41, 132));
    }

    #[test]
    fn agent_terminal_keeps_final_child_cursor_exposed() {
        let mut state = AppState::test_new();
        let ws = crate::workspace::Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        state.terminals.insert(
            ws.tabs[0].panes[&pane_id].attached_terminal_id.clone(),
            crate::terminal::TerminalState::new(
                ws.tabs[0].panes[&pane_id].attached_terminal_id.clone(),
                std::path::PathBuf::from("/tmp"),
            ),
        );
        state
            .terminals
            .get_mut(&ws.tabs[0].panes[&pane_id].attached_terminal_id)
            .expect("terminal state")
            .launch_argv = Some(vec!["codex".to_string()]);
        state.workspaces = vec![ws];

        assert!(state.pane_exposes_host_cursor(0, pane_id));
    }

    #[test]
    fn adversarial_identity_state_satisfies_app_invariants_after_mutation() {
        let mut state = AppState::test_with_adversarial_identity_state();
        state.assert_invariants_for_test();

        let ws = &mut state.workspaces[0];
        let active_public = ws.tabs[ws.active_tab].number;
        assert_ne!(ws.active_tab + 1, active_public);
        let new_pane = ws.test_split(ratatui::layout::Direction::Horizontal);
        assert!(ws.public_pane_number(new_pane).is_some());
        state.ensure_test_terminals();

        state.assert_invariants_for_test();
    }

    fn navigator_row_for_display(is_workspace: bool) -> NavigatorRow {
        NavigatorRow {
            target: NavigatorTarget::Workspace { ws_idx: 0 },
            depth: if is_workspace { 0 } else { 1 },
            label: String::new(),
            meta: String::new(),
            status: crate::detect::AgentState::Idle,
            seen: true,
            is_current: false,
            is_workspace,
            is_tab: false,
            expanded: true,
            search_text: String::new(),
            matched: true,
        }
    }

    #[test]
    fn navigator_display_lines_separate_workspace_groups() {
        let rows = vec![
            navigator_row_for_display(true),
            navigator_row_for_display(false),
            navigator_row_for_display(true),
            navigator_row_for_display(false),
        ];
        assert_eq!(
            navigator_display_lines(&rows),
            vec![
                NavigatorDisplayLine::Row(0),
                NavigatorDisplayLine::Row(1),
                NavigatorDisplayLine::Spacer,
                NavigatorDisplayLine::Row(2),
                NavigatorDisplayLine::Row(3),
            ]
        );
    }

    #[test]
    fn navigator_display_lines_have_no_leading_spacer() {
        let rows = vec![
            navigator_row_for_display(true),
            navigator_row_for_display(false),
        ];
        assert_eq!(
            navigator_display_lines(&rows),
            vec![NavigatorDisplayLine::Row(0), NavigatorDisplayLine::Row(1)]
        );
        assert!(navigator_display_lines(&[]).is_empty());
    }

    #[test]
    fn navigator_display_index_maps_row_to_line() {
        let rows = vec![
            navigator_row_for_display(true),
            navigator_row_for_display(false),
            navigator_row_for_display(true),
        ];
        let lines = navigator_display_lines(&rows);
        assert_eq!(navigator_display_index_of_row(&lines, 2), Some(3));
        assert_eq!(navigator_display_index_of_row(&lines, 9), None);
    }

    #[test]
    fn navigator_first_row_skips_spacer_lines() {
        let rows = vec![
            navigator_row_for_display(true),
            navigator_row_for_display(false),
            navigator_row_for_display(true),
        ];
        let lines = navigator_display_lines(&rows);
        // Line 2 is the spacer before the second workspace.
        assert_eq!(navigator_first_row_at_or_after(&lines, 2), Some(2));
        assert_eq!(navigator_first_row_at_or_after(&lines, 4), None);
    }

    fn rgb_luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("expected RGB color, got {color:?}");
        };
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast_ratio(a: Color, b: Color) -> f64 {
        let (lighter, darker) = {
            let a = rgb_luminance(a);
            let b = rgb_luminance(b);
            (a.max(b), a.min(b))
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn built_in_theme_names_resolve() {
        for name in THEME_NAMES {
            assert!(
                Palette::from_name(name).is_some(),
                "theme should resolve: {name}"
            );
        }
    }

    #[test]
    fn built_in_active_rows_remain_visible_with_matching_terminal_backgrounds() {
        for name in THEME_NAMES
            .iter()
            .copied()
            .filter(|name| *name != "terminal")
        {
            let palette = Palette::from_name(name).unwrap();
            let background_contrast = contrast_ratio(palette.panel_bg, palette.active_row_bg);
            assert!(
                background_contrast >= 1.05,
                "active row blends into the matching terminal background for {name}: {background_contrast:.2}:1"
            );

            let text_contrast = contrast_ratio(palette.text, palette.active_row_bg);
            assert!(
                text_contrast >= 3.0,
                "active row text loses contrast for {name}: {text_contrast:.2}:1"
            );
        }
    }

    #[test]
    fn built_in_selection_rows_stay_distinct_from_background_and_active_rows() {
        for name in THEME_NAMES
            .iter()
            .copied()
            .filter(|name| *name != "terminal")
        {
            let palette = Palette::from_name(name).unwrap();
            let background_contrast = contrast_ratio(palette.panel_bg, palette.selection_bg);
            assert!(
                background_contrast >= 1.05,
                "selection row blends into the matching terminal background for {name}: {background_contrast:.2}:1"
            );

            let text_contrast = contrast_ratio(palette.text, palette.selection_bg);
            assert!(
                text_contrast >= 3.0,
                "selection row text loses contrast for {name}: {text_contrast:.2}:1"
            );
            assert_ne!(
                palette.selection_bg, palette.active_row_bg,
                "selection row shares the active row color for {name}"
            );
        }
    }

    #[test]
    fn built_in_themes_leave_sidebar_background_unset() {
        for name in THEME_NAMES {
            let palette = Palette::from_name(name).unwrap();
            assert_eq!(
                palette.sidebar_bg,
                Color::Reset,
                "built-in theme changed the sidebar background: {name}"
            );
        }
    }

    #[test]
    fn custom_sidebar_colors_override_the_defaults() {
        let custom = crate::config::CustomThemeColors {
            sidebar_bg: Some("#181825".to_string()),
            active_row_bg: Some("#313244".to_string()),
            selection_bg: Some("#45475a".to_string()),
            ..Default::default()
        };
        let palette = Palette::catppuccin().with_overrides(&custom);

        assert_eq!(palette.sidebar_bg, Color::Rgb(24, 24, 37));
        assert_eq!(palette.active_row_bg, Color::Rgb(49, 50, 68));
        assert_eq!(palette.selection_bg, Color::Rgb(69, 71, 90));
    }

    #[test]
    fn light_theme_aliases_resolve() {
        for name in ["light", "latte", "tokyo-day", "onelight", "lotus", "dawn"] {
            assert!(
                Palette::from_name(name).is_some(),
                "theme should resolve: {name}"
            );
        }
    }

    #[test]
    fn key_matches_requires_exact_modifiers() {
        assert!(key_matches(
            &KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            KeyCode::Char('b'),
            KeyModifiers::CONTROL,
        ));

        assert!(!key_matches(
            &KeyEvent::new(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            KeyCode::Char('b'),
            KeyModifiers::CONTROL,
        ));
    }

    #[test]
    fn key_matches_letters_case_insensitively() {
        assert!(key_matches(
            &KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT),
            KeyCode::Char('b'),
            KeyModifiers::SHIFT,
        ));
    }

    #[test]
    fn linked_worktree_context_menu_keeps_safe_close_and_explicit_remove() {
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 0,
            is_linked_worktree: true,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };

        assert_eq!(
            menu.items().iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "Rename",
                "Copy path",
                CONTEXT_MENU_SEPARATOR,
                "Merge to main",
                "Open PR",
                "Sync",
                "Refresh status",
                CONTEXT_MENU_SEPARATOR,
                "New group\u{2026}",
                "Remove from group",
                CONTEXT_MENU_SEPARATOR,
                "Close",
                "Delete worktree\u{2026}",
                CONTEXT_MENU_SEPARATOR,
                "Hide 5m",
                "Hide 10m",
                "Hide 15m",
                "Hide 30m",
            ]
        );
    }

    #[test]
    fn git_workspace_context_menu_keeps_remove_for_managed_worktrees_only() {
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 0,
            is_linked_worktree: false,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };

        assert_eq!(
            menu.items().iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "Rename",
                "Copy path",
                CONTEXT_MENU_SEPARATOR,
                "New worktree",
                "Open worktree\u{2026}",
                "Sync",
                "Refresh status",
                CONTEXT_MENU_SEPARATOR,
                "New group\u{2026}",
                "Remove from group",
                CONTEXT_MENU_SEPARATOR,
                "Close",
                CONTEXT_MENU_SEPARATOR,
                "Hide 5m",
                "Hide 10m",
                "Hide 15m",
                "Hide 30m",
            ]
        );
    }

    #[test]
    fn parent_worktree_context_menu_uses_repo_actions() {
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 0,
            is_linked_worktree: false,
            has_worktree_children: true,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        assert_eq!(
            menu.items().iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "Rename",
                "Copy path",
                CONTEXT_MENU_SEPARATOR,
                "New worktree",
                "Open worktree\u{2026}",
                "Sync",
                "Refresh status",
                "Collapse",
                CONTEXT_MENU_SEPARATOR,
                "New group\u{2026}",
                "Remove from group",
                CONTEXT_MENU_SEPARATOR,
                "Close workspace",
                CONTEXT_MENU_SEPARATOR,
                "Hide 5m",
                "Hide 10m",
                "Hide 15m",
                "Hide 30m",
            ]
        );
    }

    fn test_plugin_action(
        plugin_id: &str,
        enabled: bool,
        action_id: &str,
        contexts: Vec<crate::api::schema::PluginActionContext>,
    ) -> crate::api::schema::InstalledPluginInfo {
        crate::api::schema::InstalledPluginInfo {
            plugin_id: plugin_id.to_string(),
            name: plugin_id.to_string(),
            version: "0.1.0".to_string(),
            min_herdr_version: String::new(),
            description: None,
            manifest_path: "/nonexistent".to_string(),
            plugin_root: "/nonexistent".to_string(),
            enabled,
            platforms: None,
            build: vec![],
            startup: vec![],
            actions: vec![crate::api::schema::PluginManifestAction {
                id: action_id.to_string(),
                title: "Do it".to_string(),
                description: None,
                contexts,
                platforms: None,
                command: vec!["true".to_string()],
            }],
            events: vec![],
            panes: vec![],
            link_handlers: vec![],
            source: crate::api::schema::PluginSourceInfo::default(),
            warnings: vec![],
        }
    }

    fn plugin_registry_with(
        plugin: crate::api::schema::InstalledPluginInfo,
    ) -> InstalledPluginRegistry {
        let mut map = InstalledPluginRegistry::new();
        map.insert(plugin.plugin_id.clone(), plugin);
        map
    }

    #[test]
    fn plugin_action_context_matching_action_appears_in_menu() {
        // bora-1e9: an enabled plugin action declaring the menu's own
        // context must appear, appended after a separator the same way
        // `custom_commands` already does.
        let plugins = plugin_registry_with(test_plugin_action(
            "example.tool",
            true,
            "run",
            vec![crate::api::schema::PluginActionContext::Workspace],
        ));
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &plugins,
        );
        assert!(
            items.iter().any(|item| item == "Do it"),
            "matching-context action must appear: {items:?}"
        );
    }

    #[test]
    fn plugin_action_context_non_matching_context_is_skipped() {
        // bora-1e9: an action declared only for Tab must not leak into a
        // Workspace menu.
        let plugins = plugin_registry_with(test_plugin_action(
            "example.tool",
            true,
            "run",
            vec![crate::api::schema::PluginActionContext::Tab],
        ));
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &plugins,
        );
        assert!(
            !items.iter().any(|item| item == "Do it"),
            "non-matching-context action must not appear: {items:?}"
        );
    }

    #[test]
    fn plugin_action_context_unknown_context_is_silent_skip() {
        // bora-1e9, gate G5: an action with no declared contexts (the
        // manifest default) must be a silent skip — absent, never a panic,
        // never an orphan separator with nothing after it.
        let plugins = plugin_registry_with(test_plugin_action("example.tool", true, "run", vec![]));
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &plugins,
        );
        assert!(
            !items.iter().any(|item| item == "Do it"),
            "empty contexts must never match: {items:?}"
        );
        assert_eq!(
            items.last().map(String::as_str),
            Some("Hide 30m"),
            "no orphan separator/plugin section when nothing matched: {items:?}"
        );
    }

    #[test]
    fn plugin_action_context_disabled_plugin_contributes_nothing() {
        // bora-1e9: only enabled installs may contribute a menu entry.
        let plugins = plugin_registry_with(test_plugin_action(
            "example.tool",
            false,
            "run",
            vec![crate::api::schema::PluginActionContext::Workspace],
        ));
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &plugins,
        );
        assert!(
            !items.iter().any(|item| item == "Do it"),
            "disabled plugin must contribute nothing: {items:?}"
        );
    }

    #[test]
    fn plugin_action_context_global_action_appears_in_every_menu_kind() {
        // bora-1e9: "Global actions should be available from every menu,
        // since that is what Global means" — checked across all 7
        // variants, including RepoPr/RepoIssue, which map to Global for
        // lack of a dedicated context (see plugin_menu_context).
        let plugins = plugin_registry_with(test_plugin_action(
            "example.tool",
            true,
            "run",
            vec![crate::api::schema::PluginActionContext::Global],
        ));
        let pane_id = PaneId::alloc();
        let kinds = vec![
            ContextMenuKind::Workspace {
                ws_idx: 0,
                hidden: false,
            },
            ContextMenuKind::GitWorkspace {
                ws_idx: 0,
                is_linked_worktree: false,
                has_worktree_children: false,
                collapsed: false,
                hidden: false,
            },
            ContextMenuKind::GroupHeader {
                name: "g".to_string(),
                collapse_key: "vg:g".to_string(),
                hidden: false,
            },
            ContextMenuKind::Tab {
                ws_idx: 0,
                tab_idx: 0,
            },
            ContextMenuKind::Pane {
                ws_idx: 0,
                tab_idx: 0,
                pane_id,
                source_pane_id: None,
                has_manual_label: false,
                right_click_passthrough: false,
            },
            ContextMenuKind::RepoPr {
                ws_idx: 0,
                number: 1,
                url: "https://example.com/pr/1".to_string(),
                head_ref: "feature".to_string(),
            },
            ContextMenuKind::RepoIssue {
                number: 1,
                url: "https://example.com/issues/1".to_string(),
                flow_available: false,
            },
        ];
        for kind in kinds {
            let items = build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &plugins,
            );
            assert!(
                items.iter().any(|item| item == "Do it"),
                "Global action must appear for {kind:?}: {items:?}"
            );
        }
    }
}
