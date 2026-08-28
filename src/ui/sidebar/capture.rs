//! Deterministic text+style capture of the rendered sidebar.
//!
//! This is a measurement instrument, not a feature. It exists so that a sidebar
//! rendering can be compared against another *commit's* rendering of the same
//! fixture at the same width — which is the only honest way to judge a visual
//! change, since a test that asserts against itself cannot tell you whether the
//! result got better.
//!
//! It is a child module of `ui::sidebar` on purpose: that gives it access to the
//! private `render_workspace_list` without widening anything's visibility for
//! the sake of a test.
//!
//! ## Serialization format (bora-capture-harness G3)
//!
//! Every existing text-flattener in this repo joins `Cell::symbol()` into one
//! string and throws the style away — useless for judging a *visual* change,
//! where the bug is often "same text, wrong color/weight". This capture keeps
//! both, as two lines per row:
//!
//! ```text
//! row 07 text  |├── feature/x                                         |
//! row 07 style 0..4=default 4..14=fg:Yellow,bg:Reset,mod:BOLD 14..56=default
//! ```
//!
//! The text line is the row's glyphs verbatim, for a human skimming the shape
//! of the sidebar. The style line is a run-length encoding of `(fg, bg,
//! modifier)` spans across the row — consecutive cells that share a style
//! collapse into one `start..end` span, and a span exactly matching
//! `Color::Reset`/`Color::Reset`/`Modifier::empty()` prints as the literal
//! word `default` instead of spelling out three no-op fields. Two properties
//! fall out of that choice, and they are the reason it was chosen over
//! anything alternative (a full per-cell dump, or a single JSON blob per
//! frame):
//!
//! - **Diff-friendly**: every row is exactly two lines, always in the same
//!   order, so `diff` aligns them by line number for free. A change that
//!   makes one row bold touches exactly that row's `style` line — one line
//!   changes out of `2 * height`, not the whole capture.
//! - **Human-readable**: both lines are plain text a reviewer can read
//!   without decoding an escape sequence or a serialized `Style` struct; the
//!   `text` line alone already shows most content bugs, and the `style` line
//!   only needs a glance when a `text` line matches but something still looks
//!   wrong.
//!
//! ## Determinism (bora-capture-harness G7)
//!
//! The capture must depend only on the `AppState` and the `width`/`height`
//! passed in — never on wall-clock time, real git state, `$HOME`, environment
//! variables, filesystem contents, or `HashMap` iteration order. Sources
//! traced, and how each is excluded:
//!
//! - **Wall-clock time**: `capture_sidebar` reads no clock. The fixture
//!   builder below does not either — the one place upstream `TerminalId`
//!   generation (`terminal::id::TerminalId::alloc`) mixes in
//!   `SystemTime::now()`, that value is never printed: pane badges render the
//!   workspace's public id and the pane's public number, never the raw
//!   `TerminalId`, so the wall-clock bytes never reach the captured buffer.
//! - **Real git state / filesystem contents**: the fixture never calls the
//!   real `git` binary and never reads the operator's checkout. It hand-builds
//!   every `GitSpaceMetadata` as a plain struct literal (same pattern
//!   `ui::sidebar`'s own `git_space_member` test helper already uses), so the
//!   *rendered* branch/repo text is fixture-controlled data, not a disk read.
//!   The one real filesystem touch is `persist::projects::ProjectsStore`,
//!   which has no in-memory constructor for a *declared* project (needed for
//!   a non-empty COMMANDS/CHECKS band — see `multi_workspace_fixture`'s doc).
//!   That path is exercised through a fixed-path, fixture-owned fake `.git`
//!   directory (content written by this file, byte-identical every run) and
//!   an `IsolatedDirs`-scoped `projects.yml` — never the operator's real
//!   `~/.config/bora/projects.yml` or real repositories. Nothing rendered
//!   derives from the fake checkout's absolute path: `repo_identity` comes
//!   from a fixed `origin` URL this file writes, and every workspace's own
//!   `GitSpaceMetadata` (what actually renders) is hand-built, not resolved
//!   from that checkout.
//! - **`$HOME` / environment variables**: `IsolatedDirs` redirects
//!   `XDG_CONFIG_HOME`/`XDG_STATE_HOME` to a fixture-owned temp directory for
//!   the fixture's duration and restores the previous values on drop — the
//!   existing sanctioned mechanism the rest of this repo's tests already use
//!   so `AppState::test_new()` never touches the operator's real config.
//!   `Workspace::identity_cwd` is likewise force-set on every fixture
//!   workspace, since `Workspace::test_new` otherwise defaults it to the real
//!   `std::env::current_dir()`.
//! - **`HashMap` iteration order**: traced through the render path.
//!   `Workspace::aggregate_state`/`aggregate_display_state`
//!   (`workspace/aggregate.rs`) fold over `tab.panes.values()` — a `HashMap`
//!   — with `max_by_key`, which resolves ties (two panes whose
//!   `attention_priority`/`display_priority` are numerically equal, e.g. two
//!   `Working` panes that differ only in `seen`) by returning the *last*
//!   maximal element in iteration order. That order is stable within one
//!   process but not across two separate `cargo test` invocations (Rust's
//!   default hasher reseeds per process), so a workspace that hit that tie
//!   would make this instrument lie exactly on the axis it exists to trust.
//!   This is a real bug in that fold, out of this leaf's file ownership to
//!   fix (`workspace/aggregate.rs`) — reported to the lead rather than
//!   patched here. The fixture below avoids it structurally instead of
//!   relying on it staying unhit: every fixture workspace has exactly one
//!   pane, so the fold in `aggregate_state` always has zero or one element to
//!   choose from and a tie is not reachable from this capture.
//!   Separately, `Workspace::id` defaults to a value from a
//!   process-lifetime-global `AtomicU64` counter (`generate_workspace_id`) —
//!   its value depends on how many other workspaces this test binary
//!   constructed before it ran, which varies with test execution order and
//!   is not `HashMap` iteration but has the same shape of bug (a hidden
//!   global counter standing in for wall-clock/process state). Every fixture
//!   workspace below overrides `id` explicitly after construction rather
//!   than trusting the generated one.

use std::fmt::Write as _;
use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::app::state::{AppState, Mode};
use crate::bora_config::{BoraCommand, BoraCommandMode};
use crate::config::IsolatedDirs;
use crate::detect::{Agent, AgentState};
use crate::persist::projects::{
    Member, Project, ProjectsFile, ProjectsStore, Sections, WorktreesScope,
};
use crate::terminal::TerminalRuntimeRegistry;
use crate::workspace::{CheckRun, GitSpaceMetadata, PrSummary, Workspace, WorkspaceCheckStatus};

use super::render_workspace_list;

// ── G1 + G2: the instrument ────────────────────────────────────────────────

/// Renders `app`'s sidebar workspace list at `width x height` and returns a
/// deterministic text+style capture (see the module doc for the format and
/// the determinism argument). Pure with respect to `app`/`width`/`height`:
/// reads no clock, no environment, no filesystem.
fn capture_sidebar(app: &AppState, width: u16, height: u16) -> String {
    let runtimes = TerminalRuntimeRegistry::new();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            render_workspace_list(app, &runtimes, frame, Rect::new(0, 0, width, height), false)
        })
        .expect("workspace list should render");
    serialize_buffer(terminal.backend().buffer(), width, height)
}

fn serialize_buffer(buffer: &Buffer, width: u16, height: u16) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "bora sidebar capture {width}x{height}");
    for y in 0..height {
        let row: Vec<&Cell> = (0..width).map(|x| &buffer[(x, y)]).collect();
        let text: String = row.iter().map(|cell| cell.symbol()).collect();
        let _ = writeln!(out, "row {y:02} text  |{text}|");
        let _ = writeln!(out, "row {y:02} style {}", style_runs(&row));
    }
    out
}

/// Run-length-encodes `row` into `start..end=<style>` spans; see the module
/// doc's "Serialization format" section for why this shape.
fn style_runs(row: &[&Cell]) -> String {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for i in 1..=row.len() {
        if i == row.len() || !same_style(row[i - 1], row[i]) {
            spans.push(format_span(start, i, row[start]));
            start = i;
        }
    }
    spans.join(" ")
}

fn same_style(a: &Cell, b: &Cell) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.modifier == b.modifier
}

fn format_span(start: usize, end: usize, cell: &Cell) -> String {
    if cell.fg == Color::Reset && cell.bg == Color::Reset && cell.modifier == Modifier::empty() {
        format!("{start}..{end}=default")
    } else {
        format!(
            "{start}..{end}=fg:{:?},bg:{:?},mod:{:?}",
            cell.fg, cell.bg, cell.modifier
        )
    }
}

// ── G5: multi-workspace, multi-band fixture ────────────────────────────────

const FIXTURE_REPO_IDENTITY: &str = "github.com/oss-team/bora";
const FIXTURE_REPO_ORIGIN_URL: &str = "git@github.com:oss-team/bora.git";
const FIXTURE_REPO_NAME: &str = "bora";

/// RAII guard for the on-disk fake `.git` checkout `multi_workspace_fixture`
/// needs so `persist::projects::ProjectsStore` can resolve a *declared*
/// project member (see the module doc's determinism section for why this is
/// the one real filesystem touch this instrument makes). Content is fixed
/// and rewritten on every `create`, and the directory is fixture-owned and
/// removed on drop, so repeated runs never accumulate or diverge.
struct FakeGitCheckout {
    dir: PathBuf,
}

impl FakeGitCheckout {
    fn create(origin_url: &str) -> Self {
        // Keyed by pid + a process-local atomic counter — never by
        // wall-clock. The counter (not just the pid) matters because
        // `cargo test`'s default runner puts every test in this file on its
        // own thread within ONE process: two fixture builds racing on the
        // same pid-only path would `remove_dir_all` out from under each
        // other's `create`/`Drop` (this file's own mutation-testing session
        // hit exactly that before the counter was added). Never printed —
        // this only has to avoid collisions, not be reproducible text.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ordinal = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bora-capture-harness-fixture-{}-{ordinal}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("create fake .git dir");
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write fake HEAD");
        std::fs::write(
            dir.join(".git/config"),
            format!("[remote \"origin\"]\n\turl = {origin_url}\n"),
        )
        .expect("write fake git config");
        Self { dir }
    }
}

impl Drop for FakeGitCheckout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture_git_space(checkout: &str, is_linked_worktree: bool) -> GitSpaceMetadata {
    GitSpaceMetadata {
        key: format!("fake-key-{checkout}"),
        repo_identity: FIXTURE_REPO_IDENTITY.to_string(),
        checkout_key: format!("fake-checkout-{checkout}"),
        repo_name: FIXTURE_REPO_NAME.to_string(),
        repo_root: PathBuf::from(format!("/fake/{FIXTURE_REPO_NAME}/{checkout}")),
        is_linked_worktree,
    }
}

fn fixture_workspace(
    name: &str,
    id: &str,
    branch: &str,
    checkout: &str,
    is_linked_worktree: bool,
) -> Workspace {
    let mut ws = Workspace::test_new(name);
    // Overrides the process-global-counter id `Workspace::test_new` assigns
    // — see the module doc's determinism section for why that default is
    // not safe to keep here.
    ws.id = id.to_string();
    // `Workspace::test_new` defaults this to the real `current_dir()`.
    ws.identity_cwd = PathBuf::from(format!("/fake/cwd/{checkout}"));
    ws.cached_git_branch = Some(branch.to_string());
    ws.cached_git_space = Some(fixture_git_space(checkout, is_linked_worktree));
    if is_linked_worktree {
        // The SectionRow ⌗ marker reads `worktree_space()` (the membership
        // field), not `cached_git_space` — a linked checkout needs both.
        ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: format!("fake-key-{checkout}"),
            label: FIXTURE_REPO_NAME.to_string(),
            repo_root: PathBuf::from(format!("/fake/{FIXTURE_REPO_NAME}")),
            checkout_path: PathBuf::from(format!("/fake/{FIXTURE_REPO_NAME}/{checkout}")),
            is_linked_worktree: true,
        });
    }
    ws
}

/// Sets the sole pane's detected agent/state/seen-ness for `app.workspaces[ws_idx]`.
/// Every fixture workspace has exactly one pane (default from `Workspace::test_new`),
/// which is also what keeps `aggregate_state`'s `HashMap` fold tie-free (module doc).
fn set_pane_agent(
    app: &mut AppState,
    ws_idx: usize,
    state: AgentState,
    seen: bool,
    agent: Option<Agent>,
) {
    let pane_id = app.workspaces[ws_idx].tabs[0].root_pane;
    let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    if let Some(pane) = app.workspaces[ws_idx].tabs[0].panes.get_mut(&pane_id) {
        pane.seen = seen;
    }
    let terminal = app
        .terminals
        .get_mut(&terminal_id)
        .expect("ensure_test_terminals created the terminal");
    terminal.state = state;
    terminal.detected_agent = agent;
}

/// Builds the multi-workspace, multi-band Project-view fixture G5 requires:
/// six workspaces under one declared project (so each gets its own
/// `SectionRow`, one of them a linked worktree for the ⌗ marker), one
/// workspace carrying a non-empty COMMANDS and CHECKS band, and one agent in
/// each of the four `AgentState` variants (plus the `Idle` seen/unseen split
/// `attention_priority` treats differently).
///
/// Declaring a real *project* (as opposed to leaving every workspace an
/// orphan) is required to reach a band at all: `project_view`'s COMMANDS and
/// CHECKS builders both return early when the owning project's
/// `sections.commands`/`sections.checks` is empty
/// (`push_commands_section`/`push_checks_section`), and the orphans group
/// (unmatched workspaces) always passes those as empty slices — there is no
/// way to reach a populated band without a declared project. `ProjectsStore`
/// only exposes `load()` (real `~/.config/bora/projects.yml`) or `empty()`
/// (no projects at all) — no in-memory constructor for a declared one — so
/// this goes through the fixture-owned `FakeGitCheckout` + `IsolatedDirs`
/// combination instead; see the module doc's determinism section for why
/// that stays reproducible.
///
/// Returns both RAII guards alongside the `AppState`: dropping either one
/// removes state the render depends on, so callers must keep them alive for
/// as long as they call `capture_sidebar` on the returned state.
fn multi_workspace_fixture() -> (IsolatedDirs, FakeGitCheckout, AppState) {
    let checkout = FakeGitCheckout::create(FIXTURE_REPO_ORIGIN_URL);

    let isolated = IsolatedDirs::new("capture-harness");
    let mut file = ProjectsFile::default();
    file.projects.insert(
        "bora".to_string(),
        Project {
            name: Some("Bora".to_string()),
            channel: None,
            members: vec![Member {
                dir: checkout.dir.display().to_string(),
                worktrees: WorktreesScope::All,
                template: None,
            }],
            orchestrator: None,
            sections: Some(Sections {
                checks: Some(vec!["gh".to_string()]),
                commands: Some(vec!["dev".to_string()]),
                order: None,
            }),
            layout: None,
            auto_join: true,
        },
    );
    crate::persist::projects::write_projects_file(&file).expect("write fixture projects.yml");

    let mut app = AppState::test_new();
    app.view_mode = crate::config::ViewMode::Project;
    app.mode = Mode::Terminal;
    app.projects = ProjectsStore::load();

    let mut main = fixture_workspace("main", "wfix1", "main", "main", false);
    main.cached_commands = Some(vec![BoraCommand {
        label: "dev".to_string(),
        command: "npm run dev".to_string(),
        mode: BoraCommandMode::Pane,
        branch: None,
    }]);
    main.cached_check_status = Some(WorkspaceCheckStatus {
        pr: Some(PrSummary {
            number: 42,
            title: "feat: sidebar capture harness".to_string(),
            state: "OPEN".to_string(),
            url: "https://example.invalid/pr/42".to_string(),
            mergeable: None,
        }),
        checks: vec![
            CheckRun {
                name: "build".to_string(),
                status: "COMPLETED".to_string(),
                conclusion: Some("SUCCESS".to_string()),
            },
            CheckRun {
                name: "clippy".to_string(),
                status: "COMPLETED".to_string(),
                conclusion: Some("FAILURE".to_string()),
            },
        ],
        error: None,
    });

    let feature_x = fixture_workspace("feature-x", "wfix2", "feature/x", "feature-x", false);
    let feature_y = fixture_workspace("feature-y", "wfix3", "feature/y", "feature-y", false);
    let cleanup = fixture_workspace("cleanup", "wfix4", "cleanup", "cleanup", false);
    let scratch = fixture_workspace("scratch", "wfix5", "scratch", "scratch", false);
    // bora-c1h G4: a linked-worktree workspace, so the capture exercises the
    // ⌗ marker and a non-zero ahead/behind cluster (main's fixture above
    // only carries a PR + checks, never ahead/behind).
    let mut hotfix = fixture_workspace("hotfix", "wfix6", "hotfix/urgent", "hotfix", true);
    hotfix.cached_git_ahead_behind = Some((2, 1));

    app.workspaces = vec![main, feature_x, feature_y, cleanup, scratch, hotfix];
    app.active = Some(0);
    app.ensure_test_terminals();

    // Blocked: an agent asking a question — the most urgent state either
    // ordering ranks first (see `detect::attention_priority`'s doc).
    set_pane_agent(&mut app, 0, AgentState::Blocked, true, Some(Agent::Claude));
    // Idle + unseen: finished while the user was elsewhere — "waiting on
    // you". `attention_priority` ranks this above `Working`; `display_priority`
    // does not — the one deliberate difference between the two orderings.
    set_pane_agent(&mut app, 1, AgentState::Idle, false, Some(Agent::Claude));
    // Working: actively processing, not waiting on anyone.
    set_pane_agent(&mut app, 2, AgentState::Working, true, Some(Agent::Claude));
    // Idle + seen: finished and already acknowledged.
    set_pane_agent(&mut app, 3, AgentState::Idle, true, Some(Agent::Claude));
    // ws 4 "scratch" is left at its `TerminalState::new` default
    // (`AgentState::Unknown`, `detected_agent: None`) — a plain shell pane.

    (isolated, checkout, app)
}

const FIXTURE_WIDTH: u16 = 56;
const FIXTURE_HEIGHT: u16 = 40;

// ── F0 (bora-79l.1): the executable contract ───────────────────────────────
//
// The alvo (target) transcription below is the machine-extracted text of the
// capture grid in `.local/prd/sidebar-project-view-anatomy.html` ("A
// captura", 2026-08-27), normalized at extraction time: right-aligned state
// clusters and counters pinned to column 56, PaneDotsRow l2 dot lines given
// the single leading space the design mandates ("um espaço dentro da
// section"), LIVRE separators blank. The HTML grid stays the human-readable
// mock; THIS const is the contract — P4-A compares against it and the
// generated preview renders its alvo column from it, so they can never
// drift apart.

const ALVO_CAPTURE: &str = r#"                                                 project
 Bora                                                8/8

 ⎇ main ··········································PR42 ✗
   main
   ◆
   main-review
   ○

 ⎇ feature/x
   feature-x
   ⠋
   research-feature-x
   ⠋

 ⎇ feature/y
   feature-y
   ⠋

 ⎇ cleanup
   cleanup
   ○

 ⎇ scratch
   scratch
   ○

 ⌗ ⎇ hotfix/urgent ························+916 −2 ↑2 ↓1
   hotfix
   ●

 ≡ COMANDO ··········································0/1
   · dev
 ≡ CHECKS ···········································1/2
   ✗ clippy

"#;

fn alvo_lines() -> Vec<&'static str> {
    ALVO_CAPTURE.lines().collect()
}

/// Builds the fixture the alvo describes: 8 workspaces under the "Bora"
/// project in alvo row order — main (◆ falha) + main-review (○) on `main`,
/// feature-x + research-feature-x on `feature/x` (both ⠋), feature-y (⠋),
/// cleanup (○), scratch (○, plain shell), and the linked hotfix worktree
/// (↑2 ↓1, ● esperando VOCÊ). `AgentState` has no dedicated falha variant
/// today, so falha is fixture-mapped to `Blocked` and waiting-on-you to
/// `Idle`+unseen — the glyph convergence itself is F2's leaf, which is
/// exactly why P4-A starts `#[ignore]`d.
fn alvo_fixture() -> (IsolatedDirs, FakeGitCheckout, AppState) {
    let checkout = FakeGitCheckout::create(FIXTURE_REPO_ORIGIN_URL);

    let isolated = IsolatedDirs::new("capture-alvo-fixture");
    let mut file = ProjectsFile::default();
    file.projects.insert(
        "bora".to_string(),
        Project {
            name: Some("Bora".to_string()),
            channel: None,
            members: vec![Member {
                dir: checkout.dir.display().to_string(),
                worktrees: WorktreesScope::All,
                template: None,
            }],
            orchestrator: None,
            sections: Some(Sections {
                checks: Some(vec!["gh".to_string()]),
                commands: Some(vec!["dev".to_string()]),
                order: None,
            }),
            layout: None,
            auto_join: true,
        },
    );
    crate::persist::projects::write_projects_file(&file).expect("write fixture projects.yml");

    let mut app = AppState::test_new();
    app.view_mode = crate::config::ViewMode::Project;
    app.mode = Mode::Terminal;

    // T4 (bora-79l): the SectionRow "+" renders only under mouse capture
    // (the Flat/Repo affordance's own gate), so the alvo fixture runs
    // capture-off to keep the P4-A contract honest — ALVO_CAPTURE predates
    // the "+" and pins the cluster flush at column 56 with no trailing
    // affordance. The hit-area emission is unaffected (it is not gated on
    // capture, same as Flat/Repo); only the painted glyph is hidden here.
    app.mouse_capture = false;
    app.projects = ProjectsStore::load();

    let mut main = fixture_workspace("main", "wfix1", "main", "main", false);
    main.cached_commands = Some(vec![BoraCommand {
        label: "dev".to_string(),
        command: "npm run dev".to_string(),
        mode: BoraCommandMode::Pane,
        branch: None,
    }]);
    main.cached_check_status = Some(WorkspaceCheckStatus {
        pr: Some(PrSummary {
            number: 42,
            title: "feat: sidebar capture harness".to_string(),
            state: "OPEN".to_string(),
            url: "https://example.invalid/pr/42".to_string(),
            mergeable: None,
        }),
        checks: vec![
            CheckRun {
                name: "build".to_string(),
                status: "COMPLETED".to_string(),
                conclusion: Some("SUCCESS".to_string()),
            },
            CheckRun {
                name: "clippy".to_string(),
                status: "COMPLETED".to_string(),
                conclusion: Some("FAILURE".to_string()),
            },
        ],
        error: None,
    });
    let main_review = fixture_workspace("main-review", "wfix7", "main", "main-review", false);
    let feature_x = fixture_workspace("feature-x", "wfix2", "feature/x", "feature-x", false);
    let research = fixture_workspace(
        "research-feature-x",
        "wfix8",
        "feature/x",
        "research-feature-x",
        false,
    );
    let feature_y = fixture_workspace("feature-y", "wfix3", "feature/y", "feature-y", false);
    let cleanup = fixture_workspace("cleanup", "wfix4", "cleanup", "cleanup", false);
    let scratch = fixture_workspace("scratch", "wfix5", "scratch", "scratch", false);
    let mut hotfix = fixture_workspace("hotfix", "wfix6", "hotfix/urgent", "hotfix", true);
    hotfix.cached_git_ahead_behind = Some((2, 1));
    // The alvo pins `+916 −2` on the hotfix branch header; T3 reads the
    // uncommitted diff straight off the cached change set (the same
    // source the right panel's Changes tab reads), so the fixture sets
    // the numstat it renders from.
    hotfix.cached_change_set = Some(crate::workspace::WorkspaceChangeSet {
        sections: vec![crate::workspace::ChangeSection {
            kind: crate::workspace::ChangeSectionKind::Unstaged,
            files: vec![crate::workspace::ChangedFile {
                path: "src/sidebar.rs".to_string(),
                added: Some(916),
                removed: Some(2),
                status: crate::workspace::ChangeStatus::Modified,
            }],
        }],
        base_ref: None,
    });

    app.workspaces = vec![
        main,
        main_review,
        feature_x,
        research,
        feature_y,
        cleanup,
        scratch,
        hotfix,
    ];
    app.active = Some(0);
    app.ensure_test_terminals();

    // Alvo state per block, in fixture order. Semantics documented above.
    set_pane_agent(&mut app, 0, AgentState::Blocked, true, Some(Agent::Claude));
    set_pane_agent(&mut app, 1, AgentState::Idle, true, Some(Agent::Claude));
    set_pane_agent(&mut app, 2, AgentState::Working, true, Some(Agent::Claude));
    set_pane_agent(&mut app, 3, AgentState::Working, true, Some(Agent::Claude));
    set_pane_agent(&mut app, 4, AgentState::Working, true, Some(Agent::Claude));
    set_pane_agent(&mut app, 5, AgentState::Idle, true, Some(Agent::Claude));
    // ws 6 "scratch" stays Unknown — plain shell pane (○ parado).
    set_pane_agent(&mut app, 7, AgentState::Idle, false, Some(Agent::Claude));

    (isolated, checkout, app)
}

// ── F0: capture → HTML "hoje" block exporter ───────────────────────────────

const PREVIEW_BEGIN: &str = "<!-- sidebar-preview:begin -->";
const PREVIEW_END: &str = "<!-- sidebar-preview:end -->";

struct CapturedRow {
    num: usize,
    text: String,
    style: String,
}

fn capture_rows(capture: &str) -> Vec<CapturedRow> {
    let mut rows: Vec<CapturedRow> = Vec::new();
    for line in capture.lines() {
        let Some(rest) = line.strip_prefix("row ") else {
            continue;
        };
        let Some((num, rest)) = rest.split_once(' ') else {
            continue;
        };
        let Ok(num) = num.parse::<usize>() else {
            continue;
        };
        if let Some(inner) = rest.strip_prefix("text  |") {
            let text = inner.strip_suffix('|').unwrap_or(inner);
            rows.push(CapturedRow {
                num,
                text: text.to_string(),
                style: String::new(),
            });
        } else if let Some(style) = rest.strip_prefix("style ") {
            if let Some(row) = rows.iter_mut().find(|r| r.num == num) {
                row.style = style.to_string();
            }
        }
    }
    rows
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Maps one serialized style segment (`default`, or the Debug render
/// `fg:…,bg:…,mod:…`) to CSS declarations.
fn style_attrs(seg: &str) -> Vec<String> {
    if seg == "default" {
        return Vec::new();
    }
    let mut attrs = Vec::new();
    let mut parts = seg.splitn(2, ",mod:");
    let colors = parts.next().unwrap_or("");
    let mods = parts.next().unwrap_or("");
    if let Some((fg, bg)) = colors.split_once(",bg:") {
        if let Some(css) = color_css(fg.strip_prefix("fg:").unwrap_or(fg)) {
            attrs.push(format!("color:{css}"));
        }
        if let Some(css) = color_css(bg) {
            attrs.push(format!("background-color:{css}"));
        }
    }
    for modifier in mods.split('|') {
        match modifier {
            "BOLD" => attrs.push("font-weight:700".to_string()),
            "ITALIC" => attrs.push("font-style:italic".to_string()),
            "UNDERLINED" => attrs.push("text-decoration:underline".to_string()),
            "DIM" => attrs.push("opacity:.65".to_string()),
            // ponytail: rare modifiers (REVERSED, …) render unstyled in the
            // preview; add a mapping when a capture actually contains one.
            _ => {}
        }
    }
    attrs
}

// ponytail: named ANSI cells fall back to lowercase CSS color keywords; the
// themed sidebar cells are Rgb in practice (bora-capture-harness G2).
fn color_css(value: &str) -> Option<String> {
    if value == "Reset" {
        return None;
    }
    if let Some(inner) = value.strip_prefix("Rgb(").and_then(|s| s.strip_suffix(')')) {
        let channels: Vec<&str> = inner.split(',').map(str::trim).collect();
        if channels.len() == 3 {
            return Some(format!(
                "rgb({}, {}, {})",
                channels[0], channels[1], channels[2]
            ));
        }
    }
    Some(value.to_lowercase())
}

fn push_span(out: &mut String, text: &str, start: usize, end: usize, attrs: &str) {
    let chunk: String = text
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    if chunk.is_empty() {
        return;
    }
    let escaped = escape_html(&chunk);
    if attrs.is_empty() {
        let _ = write!(out, "{escaped}");
    } else {
        let _ = write!(out, r#"<span style="{attrs}">{escaped}</span>"#);
    }
}

/// Renders one captured row as HTML, walking the style line's run-length
/// spans; uncovered ranges fall back to unstyled text.
fn row_html(text: &str, style: &str) -> String {
    // The style line separates spans with spaces, but the Debug render of
    // `Color::Rgb(r, g, b)` contains spaces too — a bare split(' ') would
    // shred spans mid-tuple. Only tokens whose `=`-left side contains `..`
    // start a span; every other token continues the previous span.
    let mut spans: Vec<String> = Vec::new();
    for token in style.split(' ').filter(|s| !s.is_empty()) {
        if token
            .split_once('=')
            .is_some_and(|(range, _)| range.contains(".."))
        {
            spans.push(token.to_string());
        } else if let Some(last) = spans.last_mut() {
            last.push(' ');
            last.push_str(token);
        }
    }
    let mut out = String::new();
    let mut cursor = 0usize;
    for span in &spans {
        let Some((range, seg)) = span.split_once('=') else {
            continue;
        };
        let Some((start, end)) = range.split_once("..") else {
            continue;
        };
        let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) else {
            continue;
        };
        if start > cursor {
            push_span(&mut out, text, cursor, start, "");
        }
        let attrs = style_attrs(seg).join(";");
        push_span(&mut out, text, start, end, &attrs);
        cursor = end;
    }
    let len = text.chars().count();
    if cursor < len {
        push_span(&mut out, text, cursor, len, "");
    }
    out
}

/// Builds the injected region: ONE capture grid with two content columns —
/// the real, colorized "hoje" on the left and the alvo contract on the
/// right — so row N of the code sits on the same visual row as row N of
/// the contract. Two separate `.cap2` grids inside the flex `.tipwrap`
/// exceeded `main`'s 980px and wrapped the alvo BELOW the capture (the
/// owner read that as "the alvo is missing"), which is exactly the failure
/// this single-grid layout prevents. Both columns render from code, so the
/// HTML pair and the P4-A comparison share one source of truth.
fn export_preview_block(capture: &str) -> String {
    let rows = capture_rows(capture);
    let content_rows = rows
        .iter()
        .rposition(|r| !r.text.trim().is_empty())
        .map_or(0, |i| i + 1);
    let alvo = alvo_lines();
    let total = content_rows.max(alvo.len());
    let mut out = String::new();
    out.push_str(
        "<h2 id=\"contrato-1b\">1b. Contrato executável — hoje (código, gerado) vs alvo</h2>\n",
    );
    out.push_str("<p class=\"sub\">Gerado por <code>just sidebar-preview</code> · fonte: o fixture do próprio contrato (<code>ui::sidebar::capture</code>). Mesma linha = mesmo row: à esquerda o que o código produz agora (cores reais), à direita o contrato que o P4-A cobra.</p>\n");
    // Two 56-char content columns must fit `main`'s 980px: 12.5px monospace
    // keeps each column ~420px wide; the fixed 3-col template aligns rows.
    out.push_str(
        "<div class=\"cap2\" style=\"font-size:12.5px;grid-template-columns:3ch 1fr 1fr\">",
    );
    out.push_str(
        "<div class=\"num\">&nbsp;</div><div><b>hoje</b> · captura real</div><div><b>alvo</b> · contrato (P4-A)</div>",
    );
    for i in 0..total {
        let hoje = rows
            .get(i)
            .filter(|_| i < content_rows)
            .map(|r| row_html(&r.text, &r.style))
            .unwrap_or_default();
        let alvo_cell = alvo.get(i).map(|l| alvo_row_html(l)).unwrap_or_default();
        let _ = write!(
            out,
            "<div class=\"num\">{i:02}</div><div>{hoje}</div><div>{alvo_cell}</div>"
        );
    }
    out.push_str("</div>\n");
    out
}

/// Replaces the region between the preview markers in the contract HTML.
/// Errors loudly when the markers are missing — silently appending would
/// grow the file on every run.
fn write_preview_into(html: &str, block: &str) -> Result<String, String> {
    let begin = html
        .find(PREVIEW_BEGIN)
        .ok_or("marcador sidebar-preview:begin ausente no HTML-contrato")?;
    let end = html
        .find(PREVIEW_END)
        .ok_or("marcador sidebar-preview:end ausente no HTML-contrato")?;
    if begin >= end {
        return Err("marcadores sidebar-preview fora de ordem no HTML-contrato".to_string());
    }
    let mut out = String::with_capacity(html.len() + block.len() + 2);
    out.push_str(&html[..begin]);
    out.push_str(PREVIEW_BEGIN);
    out.push('\n');
    out.push_str(block);
    out.push('\n');
    out.push_str(PREVIEW_END);
    out.push_str(&html[end + PREVIEW_END.len()..]);
    Ok(out)
}

fn escape_span(class: &str, text: &str) -> String {
    format!(r#"<span class="{class}">{}</span>"#, escape_html(text))
}

/// R1 color budget, one meaning per hue: green = pronto, yellow = esperando
/// VOCÊ, red = falha real, gray = everything else; mauve belongs to the
/// ProjectRow and blue to the selection edge — neither appears here.
fn dot_class(dot: &str) -> Option<&'static str> {
    match dot {
        "◆" => Some("rd b"),
        "●" => Some("yw b"),
        "○" => Some("o0"),
        "⠋" => Some("o1 b"),
        _ => None,
    }
}

/// Splits `head ····tail` at the dotted leader run; no leader → no split.
fn split_leader(body: &str) -> (&str, &str, &str) {
    let Some(i) = body.find(" ·") else {
        return (body, "", "");
    };
    let head = &body[..i];
    let rest = &body[i + 1..];
    let dots = rest.len() - rest.trim_start_matches('·').len();
    (head, &rest[..dots], &rest[dots..])
}

/// Branch-header head: `⌗`/`⎇` glyphs in .o1, the branch name in .o1 .b —
/// one treatment, no bold+dim+italic stack.
fn alvo_header_head_html(head: &str) -> String {
    let mut out = String::new();
    let mut rest = head;
    loop {
        let (glyph, after) = if let Some(a) = rest.strip_prefix("⌗") {
            ("⌗", a)
        } else if let Some(a) = rest.strip_prefix("⎇") {
            ("⎇", a)
        } else {
            break;
        };
        out.push_str(&escape_span("o1", glyph));
        match after.strip_prefix(' ') {
            Some(next) => {
                out.push(' ');
                rest = next;
            }
            None => return out,
        }
    }
    if !rest.is_empty() {
        out.push_str(&escape_span("o1 b", rest));
    }
    out
}

/// Paints one alvo contract line with the design's own CSS classes.
/// Token rules, not row-number rules, so alvo text edits don't break it.
fn alvo_row_html(line: &str) -> String {
    if line.trim().is_empty() {
        return String::new();
    }
    let spaces = &line[..line.len() - line.trim_start().len()];
    let body = line.trim_start();
    let mut out = String::from(spaces);
    if body == "project" {
        out.push_str(&escape_span("o0 b", body));
        return out;
    }
    if let Some(rest) = body.strip_prefix("Bora") {
        out.push_str(&escape_span("mv b", "Bora"));
        let gap_len = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..gap_len]);
        out.push_str(&escape_span("o0", rest.trim()));
        return out;
    }
    if body.starts_with("≡ ") {
        let (head, leader, tail) = split_leader(body);
        out.push_str(&escape_span("s1 b", head));
        if !leader.is_empty() {
            out.push_str(&escape_span("s1", leader));
            out.push_str(&escape_span("s2", tail));
        }
        return out;
    }
    if body.starts_with("⎇ ") || body.starts_with("⌗ ") {
        let (head, leader, tail) = split_leader(body);
        out.push_str(&alvo_header_head_html(head));
        if !leader.is_empty() {
            out.push_str(&escape_span("s1", leader));
            let class = if tail.contains('✗') { "rd" } else { "o1" };
            out.push_str(&escape_span(class, tail));
        }
        return out;
    }
    if let Some(rest) = body.strip_prefix("· ") {
        out.push_str(&escape_span("s2", "·"));
        out.push(' ');
        out.push_str(&escape_span("o1", rest));
        return out;
    }
    if let Some(rest) = body.strip_prefix("✗ ") {
        out.push_str(&escape_span("rd", "✗"));
        out.push(' ');
        out.push_str(&escape_span("o1", rest));
        return out;
    }
    let dots: Vec<&str> = body.split(' ').collect();
    if !dots.is_empty() && dots.iter().all(|d| dot_class(d).is_some()) {
        for (i, dot) in dots.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&escape_span(dot_class(dot).unwrap(), dot));
        }
        return out;
    }
    out.push_str(&escape_span("o1", body));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── G4: determinism is asserted, not asserted-by-hand ──────────────────

    #[test]
    fn capture_is_byte_identical_across_two_calls_on_the_same_state() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let first = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        let second = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        assert_eq!(
            first, second,
            "capture must be byte-identical across two calls on the same AppState \
             — a capture that varies run to run cannot measure anything"
        );
    }

    /// Stronger than G4's literal ask (same fixture *instance*, captured
    /// twice): this rebuilds the fixture from scratch a second time —
    /// including the on-disk `FakeGitCheckout` + `ProjectsStore::load()`
    /// round trip — and captures that independently-built state too. This is
    /// exactly the property the instrument is FOR (comparing two separate
    /// builds/commits), so it is worth pinning here even though the gate
    /// only requires the same-instance version above. This is also the test
    /// that would have caught the `Workspace::id` global-counter bug the
    /// module doc describes, had the fixture not overridden `id` explicitly:
    /// two fresh builds in the same test binary see different counter
    /// values, so a fixture leaking that id into rendered text would fail
    /// exactly here.
    #[test]
    fn capture_is_byte_identical_across_two_independent_fixture_builds() {
        let (_isolated_a, _checkout_a, app_a) = multi_workspace_fixture();
        let first = capture_sidebar(&app_a, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        drop((_isolated_a, _checkout_a));

        let (_isolated_b, _checkout_b, app_b) = multi_workspace_fixture();
        let second = capture_sidebar(&app_b, FIXTURE_WIDTH, FIXTURE_HEIGHT);

        assert_eq!(
            first, second,
            "two independently built fixtures (fresh on-disk fake checkout, fresh \
             ProjectsStore::load(), fresh AppState) must capture identically — this is \
             the cross-commit comparison the whole instrument exists to make honest"
        );
    }

    // ── G6: obtainable as text by a human running one command ──────────────

    /// Run with:
    /// `cargo test --locked ui::sidebar::capture::tests::print_sidebar_capture -- --exact --nocapture`
    #[test]
    fn print_sidebar_capture() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        println!("{text}");
    }

    // ── Sanity: the fixture actually exercises what G5 asks for ─────────────

    #[test]
    fn fixture_capture_shows_every_worktree_and_every_agent_state() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);

        for branch in [
            "main",
            "feature/x",
            "feature/y",
            "cleanup",
            "scratch",
            "hotfix/urgent",
        ] {
            assert!(
                text.contains(branch),
                "section row for {branch:?} must render: {text}"
            );
        }
        // The declared project's name, and both bands' declared items.
        assert!(text.contains("Bora"), "project row must render: {text}");
        assert!(text.contains("dev"), "COMMANDS item must render: {text}");
        assert!(
            text.contains("clippy"),
            "failing CHECKS item must render: {text}"
        );
    }

    // ── bora-c1h: v3 layout gates (G1/G2/G4/G5/G7) ──────────────────────────

    #[test]
    fn v3_group_header_row_is_underlined_with_no_hexagon() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        assert!(!text.contains('⬢'), "G1: no hexagon glyph anywhere: {text}");
        let lines: Vec<&str> = text.lines().collect();
        let bora_text_idx = lines
            .iter()
            .position(|l| l.starts_with("row ") && l.contains("text") && l.contains("Bora"))
            .expect("project row with 'Bora' must render");
        let style_line = lines[bora_text_idx + 1];
        assert!(
            style_line.contains("UNDERLINED"),
            "G1: the project row's name must be underlined: {style_line}"
        );
    }

    /// Attribution — born asserting `FEATURE-X`/`HOTFIX` UPPERCASE on the
    /// section rows, then re-aimed (A3) at `MAIN` + the `───────` rule.
    /// T3 (bora-79l) removed the name slot entirely: the branch line is a
    /// DECLARED header (`⎇ branch ····· cluster`), the workspace's name
    /// lives only on its `PaneDotsRow` l1. Captured before / after, same
    /// fixture:
    ///
    ///   before  row 09 |▾ ─────── ⎇ feature/x                     |
    ///   after   row 09 | ⎇ feature/x                              |
    ///
    /// The surviving assertions: the branch label renders (lowercase),
    /// every trace of the old identity row is gone (no UPPERCASE name,
    /// no chevron, no A3 rule), the workspace's own name lives on the
    /// dots row, and the ⌗ worktree marker survives the slot reorder.
    #[test]
    fn v3_branch_headers_declare_the_branch_no_name_slot_no_chevron() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        assert!(
            !text.contains("MAIN") && !text.contains("FEATURE-X"),
            "T3: the header has no name slot — nothing uppercase: {text}"
        );
        assert!(
            !text.contains('▾') && !text.contains('▸'),
            "T3: no chevron on a branch header — collapse is the folder's: {text}"
        );
        // The old A3 filler (`───────` in the name slot) is gone — but band
        // rulers legitimately run `─`, so the invariant is per-row: a branch
        // header itself never carries a box-drawing run. (Fica vermelho se o
        // name slot voltar a renderizar antes do glifo de branch.)
        let header_rows: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("text") && l.contains('\u{2387}'))
            .collect();
        assert!(
            !header_rows.is_empty(),
            "T3: at least one branch header must render: {text}"
        );
        for header in &header_rows {
            assert!(
                !header.contains('\u{2500}'),
                "T3: a branch header carries no box-drawing run: {header}"
            );
        }
        assert!(
            text.contains("feature/x"),
            "the branch label is the header's whole text: {text}"
        );
        assert!(
            text.contains("feature-x"),
            "the workspace's own unique name lives on its dots row: {text}"
        );
        assert!(
            text.contains('⌗'),
            "G4: a worktree checkout keeps the ⌗ marker: {text}"
        );
        assert!(
            !text.contains("##"),
            "G4: the old condensed ## worktree row must be gone from Project view: {text}"
        );
        assert!(
            !text.contains('╰'),
            "the pane-row connector (the owner's \"rabinho\") is gone: {text}"
        );
    }

    #[test]
    fn v3_state_cluster_shows_pr_checks_and_ahead_behind() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        assert!(text.contains("PR42"), "G5: main's PR badge renders: {text}");
        assert!(
            text.contains('✗'),
            "G5: main's failing check renders the checks-rollup glyph: {text}"
        );
        assert!(
            text.contains("↑2"),
            "G5: hotfix's ahead count renders: {text}"
        );
        assert!(
            text.contains("↓1"),
            "G5: hotfix's behind count renders: {text}"
        );
    }

    #[test]
    fn v3_row_gap_produces_blank_rows_between_workspace_blocks() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        let row_texts: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("row ") && l.contains("text"))
            .map(|l| {
                let start = l.find('|').unwrap() + 1;
                let end = l.rfind('|').unwrap();
                &l[start..end]
            })
            .collect();
        let has_gap = row_texts
            .windows(3)
            .any(|w| !w[0].trim().is_empty() && w[1].trim().is_empty() && !w[2].trim().is_empty());
        assert!(
            has_gap,
            "G7: at least one blank row must separate two workspace blocks: {row_texts:?}"
        );
    }

    #[test]
    fn one_attribute_change_produces_a_small_diff() {
        // G3: a one-attribute change must touch a SMALL part of the capture,
        // not reflow everything. Simulate it directly on a buffer rather than
        // through a real render, since this is a property of the
        // serialization, not of any particular sidebar row.
        let width = 10u16;
        let height = 2u16;
        let mut before = Buffer::empty(Rect::new(0, 0, width, height));
        for x in 0..width {
            before[(x, 0)].set_symbol("a");
            before[(x, 1)].set_symbol("b");
        }
        let mut after = before.clone();
        after[(3, 0)].modifier |= Modifier::BOLD;

        let before_text = serialize_buffer(&before, width, height);
        let after_text = serialize_buffer(&after, width, height);

        let before_lines: Vec<&str> = before_text.lines().collect();
        let after_lines: Vec<&str> = after_text.lines().collect();
        assert_eq!(before_lines.len(), after_lines.len());
        let changed: Vec<usize> = (0..before_lines.len())
            .filter(|&i| before_lines[i] != after_lines[i])
            .collect();
        assert_eq!(
            changed,
            vec![2],
            "only row 0's style line (index 2: header, row0 text, row0 style, ...) \
             should differ for a single-cell modifier change: {before_text}\n---\n{after_text}"
        );
        // A small diff is not enough: the changed line must SAY what changed.
        // Span boundaries move whenever `same_style` sees a difference, so this
        // test passed even with the modifier omitted from the serialized text
        // entirely — the diff was small but illegible, which defeats the point
        // of capturing style at all. Found by the lead re-running a mutation
        // independently rather than trusting the leaf's own set.
        assert!(
            after_lines[2].contains("BOLD") && !before_lines[2].contains("BOLD"),
            "the changed style line must name the modifier that changed, not merely differ: \
             {}\n---\n{}",
            before_lines[2],
            after_lines[2]
        );
    }

    // ── F0 (bora-79l.1): exporter + P4-A contract ──────────────────────────

    const EXPORTER_SAMPLE_CAPTURE: &str = "\
        bora sidebar capture 8x2\n\
        row 00 text  |<a & b> |\n\
        row 00 style 0..5=fg:Rgb(243, 139, 168),bg:Reset,mod:BOLD 5..8=default\n\
        row 01 text  |  plain  |\n\
        row 01 style 0..8=default\n";

    #[test]
    fn exporter_block_wraps_rows_escapes_html_and_carries_the_alvo_column() {
        let block = export_preview_block(EXPORTER_SAMPLE_CAPTURE);
        // Entities land on separate spans (the sample text is split at
        // column 5), so assert per entity, not one contiguous string.
        assert!(
            block.contains("&lt;") && block.contains("&amp;") && block.contains("&gt;"),
            "HTML specials in captured text must be escaped: {block}"
        );
        assert!(
            block.contains("rgb(243, 139, 168)") && block.contains("font-weight:700"),
            "real capture colors and modifiers must be colorized: {block}"
        );
        assert!(
            block.contains("plain"),
            "every captured row's text must appear: {block}"
        );
        assert!(
            block.contains("alvo") && block.contains("8/8") && block.contains("clippy"),
            "the alvo column must come from the same ALVO_CAPTURE const P4-A uses: {block}"
        );
        assert!(
            block.contains(r#"<span class="rd">PR42 ✗</span>"#)
                && block.contains(r#"<span class="mv b">Bora</span>"#)
                && block.contains(r#"<span class="yw b">●</span>"#),
            "the alvo column must carry the R1 color budget, not plain text: {block}"
        );
        assert!(
            block.contains(r#"<span class="s1">····"#),
            "dotted leaders must render as .s1: {block}"
        );
        assert!(
            block.contains("grid-template-columns:3ch 1fr 1fr"),
            "hoje and alvo must share one aligned grid, not two wrapping ones: {block}"
        );
    }

    #[test]
    fn exporter_writer_replaces_marked_region_idempotently() {
        let html = format!("<p>antes</p>\n{PREVIEW_BEGIN}\nvelho\n{PREVIEW_END}\n<p>depois</p>\n");
        let once = write_preview_into(&html, "BLOCO").expect("markers present");
        assert!(
            once.contains("<p>antes</p>") && once.contains("BLOCO") && !once.contains("velho"),
            "the marked region is replaced wholesale, neighbors untouched: {once}"
        );
        let twice = write_preview_into(&once, "BLOCO").expect("markers survive");
        assert_eq!(
            once, twice,
            "re-running the writer on its own output must be byte-identical"
        );
        assert!(
            write_preview_into("<p>sem marcador</p>", "BLOCO").is_err(),
            "a marker-less contract file must fail loudly, not grow silently"
        );
    }

    #[test]
    fn alvo_const_shapes_the_contract() {
        let alvo = alvo_lines();
        assert_eq!(alvo.len(), 36, "the alvo is exactly 36 rows: {alvo:#?}");
        assert!(alvo[0].ends_with("project"), "row 00 is the view toggle");
        for anchor in ["Bora", "8/8", "hotfix/urgent", "COMANDO", "clippy"] {
            assert!(
                alvo.iter().any(|l| l.contains(anchor)),
                "alvo must contain {anchor:?}: {alvo:#?}"
            );
        }
        for line in &alvo {
            assert!(
                line.chars().count() <= 56,
                "no alvo row may exceed the 56-column capture: {line:?}"
            );
        }
        // Right-pinned clusters sit flush at column 56.
        for i in [1, 3, 27, 31, 33] {
            assert_eq!(
                alvo[i].chars().count(),
                56,
                "cluster row {i:02} must end at column 56: {:?}",
                alvo[i]
            );
        }
    }

    /// P4-A — the contract test the following leaves unlock. Born
    /// `#[ignore]`d on purpose: it compares today's REAL rendering of the
    /// alvo fixture against the ALVO_CAPTURE contract line by line, and
    /// today they differ by design (that difference IS the backlog). Run
    /// with:
    /// `cargo nextest run -E 'test(p4a)' -- --ignored`
    /// F8 removes the `#[ignore]` once the rendering converges.
    #[test]
    #[ignore = "P4-A: the F1..F7 leaves converge the rendering onto ALVO_CAPTURE; F8 unlocks"]
    fn p4a_project_view_capture_matches_alvo_line_by_line() {
        let (_isolated, _checkout, app) = alvo_fixture();
        let capture = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        let got: Vec<String> = capture
            .lines()
            .filter(|l| l.starts_with("row ") && l.contains(" text  |"))
            .map(|l| {
                let start = l.find('|').expect("text row delimiter") + 1;
                let end = l.rfind('|').expect("text row delimiter");
                l[start..end].trim_end().to_string()
            })
            .collect();
        let want: Vec<String> = alvo_lines()
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect();
        assert!(
            got.len() >= want.len(),
            "capture must cover every alvo row: {} rows vs {}",
            got.len(),
            want.len()
        );
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(g, w, "row {i:02} diverges from the contract");
        }
        let trailing_blank = got[want.len()..].iter().all(String::is_empty);
        assert!(
            trailing_blank,
            "rows beyond the alvo must be blank, not layout overflow: {:?}",
            &got[want.len()..]
        );
    }

    /// Regenerates the "hoje" block in the contract HTML. Run via:
    /// `just sidebar-preview`
    /// (`cargo test --locked --bin bora ui::sidebar::capture::tests::write_sidebar_preview -- --exact --ignored --nocapture`)
    #[test]
    #[ignore = "writes .local/prd/sidebar-project-view-anatomy.html — a real side effect, never run by default"]
    fn write_sidebar_preview() {
        let (_isolated, _checkout, app) = alvo_fixture();
        let capture = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        let block = export_preview_block(&capture);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".local/prd/sidebar-project-view-anatomy.html");
        let html =
            std::fs::read_to_string(&path).expect("contract HTML must exist (repo-local, .local/)");
        let updated = write_preview_into(&html, &block).expect("preview markers present");
        std::fs::write(&path, updated).expect("write preview block");
        println!(
            "wrote hoje block ({} rows + alvo column) to {}",
            capture.lines().filter(|l| l.contains(" text  |")).count(),
            path.display()
        );
    }
}
