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

    /// Attribution — this test asserted `FEATURE-X` and `HOTFIX` appeared
    /// UPPERCASE on their own section rows. A3 (`SectionRow.repo_shown`)
    /// changed that on purpose: every workspace in this fixture is a
    /// worktree of the SAME repo, so only the FIRST prints the repo name and
    /// the siblings render a `───────` rule in its place. Captured before /
    /// after, same fixture:
    ///
    ///   before  row 09 |▾ FEATURE-X ⎇ feature/x                  |
    ///                  |  ╰ w1p2                                 |
    ///   after   row 09 |▾ ─────── ⎇ feature/x                     |
    ///                  |  feature-x  ⠁                           |
    ///
    /// So the identity moved from an uppercase repo name on line 1 to the
    /// workspace's own unique name on line 2 — which is the whole point of
    /// the round: line 1 stops repeating what did not change. The uppercase
    /// assertion survives, aimed at the one row that still carries a name.
    #[test]
    fn v3_first_row_of_a_repo_names_it_siblings_get_the_a3_rule() {
        let (_isolated, _checkout, app) = multi_workspace_fixture();
        let text = capture_sidebar(&app, FIXTURE_WIDTH, FIXTURE_HEIGHT);
        assert!(
            text.contains("MAIN"),
            "G2: the first row of the repo still renders its name UPPERCASE: {text}"
        );
        assert!(
            text.contains("▾ ───────"),
            "A3: a sibling worktree of the same repo renders the rule instead \
             of repeating the name: {text}"
        );
        assert!(
            text.contains("feature-x"),
            "the workspace's own unique name now lives on its dots row: {text}"
        );
        assert!(
            text.contains("feature/x"),
            "G2/G3: branch stays lowercase (dim): {text}"
        );
        assert!(
            text.contains('⌗'),
            "G4: a worktree checkout gets the ⌗ marker: {text}"
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
}
