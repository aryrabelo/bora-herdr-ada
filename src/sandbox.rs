//! Composes the sandboxed launch profile for a project's declared
//! `orchestrator` (`.local/prd/sidebar-design.md`, "Orchestrator" section;
//! bora-1le.2). Every function here is pure: it takes already-resolved
//! paths and strings and returns data, never touching the filesystem or
//! spawning anything. `src/app/agents.rs`'s `orchestrator_launch_for_start`
//! is the one caller — it is also the one place that touches disk
//! (re-reading the project file, writing the `srt` settings JSON
//! [`SandboxProfile::to_srt_settings`] renders) and the one place that
//! wires the composed command line into `App::start_agent`.
//!
//! That split is deliberate, not an oversight. `agent.start` does not
//! `exec` (AGENTS.md, dated rule) — it TYPES a composed command line into
//! a pane's already-running interactive shell. So proving "the
//! orchestrator cannot write a member file" without actually spawning a
//! sandboxed agent inside the operator's live bora session — which this
//! repo's test rules forbid — means asserting on the composed profile
//! data itself; proving the wiring itself means asserting on the literal
//! bytes `agent.start` types (see `src/app/agents.rs`'s tests).
//!
//! srt (`@anthropic-ai/sandbox-runtime`,
//! <https://github.com/anthropic-experimental/sandbox-runtime>) wraps an
//! arbitrary command in an OS-level sandbox (`sandbox-exec` on macOS,
//! `bubblewrap` on Linux) driven by a JSON settings file, normally
//! `~/.srt-settings.json`, overridable with `--settings <path>`. Its
//! filesystem schema (from the project README's "Filesystem
//! Configuration" section):
//!
//! - **Read is deny-then-allow**: everything is readable by default.
//!   `denyRead` denies a region; `allowRead` re-opens specific paths
//!   inside it. As a general rule `allowRead` wins over `denyRead` — the
//!   opposite of write, below — but the README's own examples qualify
//!   that: a `denyRead` entry that is MORE SPECIFIC than the `allowRead`
//!   region it falls inside still stays denied (specificity beats the
//!   allow/deny type; only ties resolve in `allowRead`'s favor).
//!   [`SandboxProfile::deny_read`] leans on exactly that qualification: it
//!   always starts with the operator's whole home directory (broad,
//!   coarse) — chosen over `/` because denying `/` would also swallow the
//!   system paths (`/usr`, `/lib`, the agent binaries themselves) the
//!   README's own "workspace-only" recipe promises stay readable when
//!   only `/Users`/`/home` is denied, and because the home directory is
//!   the actual asset worth confining (SSH keys, other repos, shell
//!   dotfiles) — then, per member, one more specific entry denying
//!   `<member>/.env`, which stays denied precisely because it is more
//!   specific than that member's own `allowRead` entry.
//! - **Write is allow-only**: `denyWrite` wins over `allowWrite`, and an
//!   empty `allowWrite` means no writes anywhere.
//!   [`SandboxProfile::allow_write`] is exactly the orchestrator's own
//!   run-file directory — see [`compose_orchestrator_launch`].
//! - **Network is allow-only** (`network.allowedDomains`): an empty list
//!   means no network access at all. This module always renders it empty
//!   — see "no network, on purpose" below for why that no longer
//!   contradicts the injected instruction.
//! - **`allowUnixSockets`** lives under `network` in srt's schema, not
//!   `filesystem`, despite reading like a filesystem-adjacent knob.
//!   [`SandboxProfile::allow_unix_sockets`] is exactly bora's own API
//!   socket — the orchestrator's one actuator.
//!
//! **Linux platform gap: `allowUnixSockets` is ignored there.** Per the
//! README's Unix Socket Settings table, `allowUnixSockets` is a real
//! per-path allowlist on macOS but is *ignored* on Linux, where socket
//! creation is blocked or allowed wholesale by a seccomp filter that
//! cannot discriminate by path. Concretely, on Linux, either: (a) the
//! default holds and the sandboxed orchestrator cannot open *any* new
//! unix socket — including bora's own API socket, which breaks the "can
//! post to channel" actuator this profile exists to grant — or (b)
//! `allowAllUnixSockets: true` is set, which permits every unix socket on
//! the box (docker.sock, an ssh-agent socket, anything else listening),
//! not just bora's. There is no middle ground; scoping to one path is a
//! macOS-only guarantee. This profile does not set `allowAllUnixSockets`
//! — trading a broken actuator for an unscoped one on Linux is not a
//! compensation, it is dropping this fence — so today the socket fence
//! means "blocked" on Linux, not "scoped to bora," until srt grows real
//! path-based Linux enforcement. Whoever wires the Linux launch path must
//! know this before assuming parity with macOS.
//!
//! **No network, on purpose (not a stale gap).** The injected instruction
//! used to promise "research over the network is allowed within this
//! sandbox's domain allowlist" while `allowed_domains` stayed empty — a
//! live contradiction, not a narrowing in progress. No existing bora
//! subsystem names which domains an orchestrator's "research" would
//! actually need, and inventing a list (or reaching for `*`) would be
//! exactly the kind of speculative widening this fence exists to refuse.
//! So the contradiction is resolved on the instruction's side instead:
//! [`compose_orchestrator_launch`]'s instruction now tells the
//! orchestrator that research, like editing, is asked of a worker agent
//! over the project channel rather than reached for directly — the same
//! delegation-by-construction shape the write fence already uses.
//!
//! Wiring: `src/app/agents.rs`'s `orchestrator_launch_for_start` decides,
//! per `agent.start` call, whether the target pane is the checkout a
//! project declares as its `orchestrator.member` and the requested kind
//! is that orchestrator's declared agent. When it matches,
//! `App::start_agent` writes [`SandboxProfile::to_srt_settings`]'s JSON to
//! disk and types [`OrchestratorLaunch::command_line`]'s srt-wrapped
//! command instead of the bare agent command. Any pane/kind that doesn't
//! match a declared orchestrator is never wrapped.

// `mcp_fence_argv` renders the orchestrator's own `bora mcp serve
// --channels <slug>` self-registration argv; wiring that into the
// orchestrator's own MCP client config (as opposed to `agent.start`'s
// launch line, which this module's other exports now feed) is later work
// outside bora-1le.2's scope, so it stays unused by production code today.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::persist::projects::Project;

/// The filesystem/network/socket boundary srt enforces around the
/// orchestrator process. See the module doc for how `deny_read` and
/// `allow_read` map onto srt's real deny-then-allow read schema, and for
/// the Linux caveat on `allow_unix_sockets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProfile {
    /// The operator's whole home directory, then one `<member>/.env`
    /// entry per member directory. The home entry is the broad denied
    /// region `allow_read` re-opens; the `.env` entries stay denied
    /// despite sitting inside an `allow_read` region because a more
    /// specific `denyRead` wins over a coarser `allowRead` — see the
    /// module doc's read-precedence paragraph.
    pub deny_read: Vec<PathBuf>,
    /// Every member directory of the project, resolved to an absolute
    /// path — re-allowed inside `deny_read`'s denied home region. This
    /// is the orchestrator's entire read surface, by design intent.
    pub allow_read: Vec<PathBuf>,
    /// Exactly the run file's containing `.bora/` directory. Nothing else
    /// is ever writable — see [`compose_orchestrator_launch`].
    pub allow_write: Vec<PathBuf>,
    /// Exactly bora's own API socket. srt blocks unix sockets by default;
    /// this is the one allowlisted exception on macOS. On Linux this
    /// allowlist is ignored outright — see the module doc.
    pub allow_unix_sockets: Vec<PathBuf>,
    /// `network.allowedDomains` — always empty today. See the module
    /// doc's "no network, on purpose" paragraph: the instruction was
    /// rewritten to match this policy rather than the policy widened to
    /// match a stale instruction.
    pub allowed_domains: Vec<String>,
}

impl SandboxProfile {
    /// Renders this profile as an `srt` settings document — see the
    /// module doc for the schema. Writing it to disk is the caller's job
    /// (`src/app/agents.rs`), same as this module never writes anything
    /// itself.
    pub fn to_srt_settings(&self) -> serde_json::Value {
        serde_json::json!({
            "filesystem": {
                "denyRead": path_strings(&self.deny_read),
                "allowRead": path_strings(&self.allow_read),
                "allowWrite": path_strings(&self.allow_write),
                "denyWrite": Vec::<String>::new(),
            },
            "network": {
                "allowedDomains": self.allowed_domains,
                "deniedDomains": Vec::<String>::new(),
                "allowUnixSockets": path_strings(&self.allow_unix_sockets),
            },
        })
    }
}

fn path_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// The complete composed launch: the srt-wrapped command line typed into
/// the orchestrator's pane, the injected opening instruction, and the bare
/// channel slug its own `bora mcp serve` fence must scope to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorLaunch {
    pub profile: SandboxProfile,
    pub instruction: String,
    /// Bare channel name, no leading `#` — the exact form `bora mcp serve
    /// --channels a,b` expects (`src/cli/mcp.rs`'s parser splits on `,`
    /// and compares bare names; `CHANNEL_NAME_SCOPED_TOOLS`'s fence in
    /// `src/mcp/tools.rs` does the same).
    pub channel: String,
    /// The orchestrator agent's own argv (currently just its detected
    /// agent id), before srt wraps it. `src/app/agents.rs`'s wiring
    /// appends any caller-supplied `agent.start` args after this.
    pub agent_argv: Vec<String>,
}

impl OrchestratorLaunch {
    /// The literal `bora mcp serve --channels <slug>` argv this
    /// orchestrator's own MCP registration must invoke, so the existing
    /// fence in `src/mcp/tools.rs:37-46` scopes its tool surface to
    /// `channel` — derived from the project every time, never hardcoded.
    pub fn mcp_fence_argv(&self) -> Vec<String> {
        vec![
            "bora".to_string(),
            "mcp".to_string(),
            "serve".to_string(),
            "--channels".to_string(),
            self.channel.clone(),
        ]
    }

    /// The srt-wrapped command line typed into the orchestrator's pane:
    /// `srt --settings <path> <agent argv...>`. This is text typed into
    /// the pane's already-running interactive shell, not an `exec` — so a
    /// local shell function or alias named `srt` (or the agent binary
    /// itself) on the operator's rc files can intercept it before the
    /// sandbox ever applies, exactly as it can for a bare `agent.start`
    /// (AGENTS.md, dated rule). `srt_settings_path` is wherever the
    /// caller wrote `self.profile.to_srt_settings()`'s rendering;
    /// producing that file is deliberately not this pure function's job
    /// (see module doc).
    pub fn command_line(&self, srt_settings_path: &Path) -> Vec<String> {
        let mut argv = vec![
            "srt".to_string(),
            "--settings".to_string(),
            srt_settings_path.display().to_string(),
        ];
        argv.extend(self.agent_argv.iter().cloned());
        argv
    }
}

/// Composes a project's orchestrator launch. `None` when the project has
/// no declared `orchestrator`.
///
/// - `resolved_member_dirs`: every member directory of `project`, already
///   resolved to absolute paths (not raw `dir:` strings) — becomes
///   `allow_read` verbatim, and seeds one `deny_read` entry per member.
/// - `run_file_path`: `orchestrator.run_file` already joined onto the
///   orchestrator's member checkout and resolved to an absolute path.
///   Its parent directory becomes the sole `allow_write` entry.
/// - `socket_path`: bora's own API socket (`crate::api::socket_path()`)
///   — the sole `allow_unix_sockets` entry.
/// - `home_dir`: the operator's home directory, already resolved — the
///   sole broad `deny_read` region. See the module doc for why home, not
///   `/`.
pub fn compose_orchestrator_launch(
    project: &Project,
    slug: &str,
    resolved_member_dirs: &[PathBuf],
    run_file_path: &Path,
    socket_path: &Path,
    home_dir: &Path,
) -> Option<OrchestratorLaunch> {
    let orchestrator = project.orchestrator.as_ref()?;

    let run_file_dir = run_file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| run_file_path.to_path_buf());

    let mut deny_read = vec![home_dir.to_path_buf()];
    deny_read.extend(resolved_member_dirs.iter().map(|dir| dir.join(".env")));

    let profile = SandboxProfile {
        deny_read,
        allow_read: resolved_member_dirs.to_vec(),
        allow_write: vec![run_file_dir],
        allow_unix_sockets: vec![socket_path.to_path_buf()],
        allowed_domains: Vec::new(),
    };

    let channel = project
        .effective_channel(slug)
        .trim_start_matches('#')
        .to_string();

    let instruction = format!(
        "Read everything in this project. This sandbox grants no network access — if \
         research is needed, ask a worker agent to do it and report back over #{channel}, \
         the same way you must ask a worker to edit. Orchestrate via #{channel}: dispatch \
         work to member agents, collect their results, and report progress there. To edit \
         anything, ask a worker agent — this sandbox's filesystem write access is empty \
         everywhere except its own run file, so editing is delegation by construction, \
         not convention."
    );

    Some(OrchestratorLaunch {
        profile,
        instruction,
        channel,
        agent_argv: vec![orchestrator.agent.clone()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::projects::Orchestrator;

    fn project_with(channel: Option<&str>, run_file: &str, agent: &str) -> Project {
        Project {
            name: None,
            channel: channel.map(str::to_string),
            members: Vec::new(),
            orchestrator: Some(Orchestrator {
                agent: agent.to_string(),
                member: "~/Sites/cnb_hono".to_string(),
                run_file: run_file.to_string(),
            }),
            sections: None,
            auto_join: true,
        }
    }

    fn member_dirs() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/Users/ary/Sites/cnb_hono"),
            PathBuf::from("/Users/ary/Sites/cnb_hono/packages/landing"),
        ]
    }

    fn home_dir() -> PathBuf {
        PathBuf::from("/Users/ary")
    }

    // Value that would still satisfy this test with the change removed:
    // hard-coding `allow_read` to any fixed list equal to `member_dirs()`
    // by coincidence — but it would break the moment a caller passes a
    // different member set, which the exact-equality assertion pins down.
    #[test]
    fn allow_read_is_every_resolved_member_dir_and_nothing_else() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert_eq!(launch.profile.allow_read, members);
    }

    // Value that would still satisfy this test with the change removed:
    // an implementation that put BOTH the run file's own path and its
    // parent dir in `allow_write` — the exact-equality assertion on the
    // whole vector, not a `contains`, rules that out.
    #[test]
    fn allow_write_contains_only_the_run_files_bora_dir() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert_eq!(
            launch.profile.allow_write,
            vec![PathBuf::from("/Users/ary/Sites/cnb_hono/.bora")]
        );
    }

    // Value that would still satisfy this test with the change removed:
    // none by construction — this is the bead's first acceptance clause
    // ("cannot write a member file") stated as its own explicit negative,
    // so a profile that widened `allow_write` to include a member root
    // (and thereby also passed a looser version of the previous test)
    // still gets caught here.
    #[test]
    fn allow_write_never_admits_a_member_directory() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        for member_dir in &members {
            assert!(
                !launch.profile.allow_write.contains(member_dir),
                "member directory {member_dir:?} must never be writable"
            );
        }
    }

    // Value that would still satisfy this test with the change removed:
    // a profile that also allowlisted some other, unrelated socket path
    // alongside bora's — the exact-equality assertion on the whole
    // vector rules that out, not just a `contains` check.
    #[test]
    fn unix_socket_allowlist_is_exactly_boras_socket_and_nothing_else() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert_eq!(launch.profile.allow_unix_sockets, vec![socket]);
    }

    // Gap 1's acceptance in one shot: the WHOLE profile, not one field.
    // Value that would still satisfy this test with the change removed:
    // an `allow_read`-only profile (today's bug) that happens to also
    // carry an unrelated, empty `deny_read` — asserting the full struct
    // by exact equality, including `deny_read`'s exact contents and
    // order, rules that out; the read fence is a no-op unless `deny_read`
    // actually denies the home region and the per-member `.env` files.
    #[test]
    fn whole_profile_denies_the_operators_home_and_reallows_exactly_the_member_dirs() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert_eq!(
            launch.profile,
            SandboxProfile {
                deny_read: vec![
                    PathBuf::from("/Users/ary"),
                    PathBuf::from("/Users/ary/Sites/cnb_hono/.env"),
                    PathBuf::from("/Users/ary/Sites/cnb_hono/packages/landing/.env"),
                ],
                allow_read: members,
                allow_write: vec![PathBuf::from("/Users/ary/Sites/cnb_hono/.bora")],
                allow_unix_sockets: vec![socket],
                allowed_domains: Vec::new(),
            },
            "the read fence must deny the operator's whole home directory, re-allow \
             exactly the member dirs, and additionally deny each member's own .env \
             (more specific than that member's allow_read entry, so it stays denied); \
             write must allow only the run file's .bora dir; sockets must allow only \
             bora's own; network must stay empty"
        );
    }

    // Gap 2's acceptance: the instruction must never promise a capability
    // the profile denies. Value that would still satisfy this test with
    // the change removed: an instruction that dropped the network
    // sentence entirely without saying anything about delegation — the
    // substring assertion on "no network access" plus the negative
    // assertion on the old promised phrasing together require the
    // instruction to actually state the real, current policy.
    #[test]
    fn instruction_and_network_policy_agree_on_no_network_access() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert!(
            launch.profile.allowed_domains.is_empty(),
            "network policy must stay the most restrictive value (no access) until a \
             real domain allowlist is decided"
        );
        assert!(
            !launch
                .instruction
                .contains("research over the network is allowed"),
            "the instruction must never promise network access the profile denies, \
             got: {}",
            launch.instruction
        );
        assert!(
            launch.instruction.contains("no network access"),
            "the instruction must state the sandbox's real (no-network) policy, got: {}",
            launch.instruction
        );
    }

    // Value that would still satisfy this test with the change removed:
    // an instruction that mentions workers or delegation in passing
    // without ever saying editing must be asked for — the substring
    // assertion requires the actual clause, not just adjacent vocabulary.
    #[test]
    fn instruction_tells_the_orchestrator_to_ask_a_worker_to_edit() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        assert!(
            launch.instruction.contains("ask a worker"),
            "instruction was: {}",
            launch.instruction
        );
    }

    // Gap 3's shell-safety acceptance: `command_line` is typed verbatim
    // into a live shell (see module doc), so any path inside it that
    // could contain a space must survive as ONE argument, not split in
    // two. Value that would still satisfy this test with the change
    // removed: quoting only the executable name (`srt`) or only the
    // flag (`--settings`) while leaving the settings path itself
    // unquoted — the assertion on the exact quoted substring rules that
    // out, it is not enough for *some* token to be quoted.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_settings_path_inside_a_spaced_member_checkout_survives_composition_as_one_shell_word() {
        let project = project_with(Some("cnb"), ".bora/run.json", "claude");
        let members = vec![PathBuf::from("/Users/ary/Sites/my project/cnb_hono")];
        let run_file = PathBuf::from("/Users/ary/Sites/my project/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let launch =
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .expect("orchestrator present");

        // The settings file's natural home is the same `.bora/` dir the
        // profile already allow-writes — `allow_write[0]` by
        // construction (see `compose_orchestrator_launch`) — which sits
        // inside the spaced member checkout used above.
        let settings_path = launch.profile.allow_write[0].join("srt-settings.json");
        let command_line = launch.command_line(&settings_path);
        let typed = crate::platform::interactive_shell_command(&command_line, "bash")
            .expect("a posix shell command must render on linux/macos");

        assert!(
            typed.contains("'/Users/ary/Sites/my project/cnb_hono/.bora/srt-settings.json'"),
            "the space-containing settings path must be single-quoted as one shell \
             argument, got: {typed}"
        );
    }

    // Value that would still satisfy this test with the change removed:
    // a hardcoded `--channels cnb` that happens to match the FIRST
    // project's channel — asserting against a SECOND project with a
    // different explicit `channel:` override catches a hardcoded slug.
    #[test]
    fn mcp_fence_argv_derives_channels_flag_from_the_projects_own_channel() {
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        let default_channel_project = project_with(None, ".bora/run.json", "claude");
        let default_launch = compose_orchestrator_launch(
            &default_channel_project,
            "cnb",
            &members,
            &run_file,
            &socket,
            &home,
        )
        .expect("orchestrator present");
        assert_eq!(
            default_launch.mcp_fence_argv(),
            vec!["bora", "mcp", "serve", "--channels", "cnb"]
        );

        let overridden_channel_project =
            project_with(Some("#totally-different"), ".bora/run.json", "claude");
        let overridden_launch = compose_orchestrator_launch(
            &overridden_channel_project,
            "cnb",
            &members,
            &run_file,
            &socket,
            &home,
        )
        .expect("orchestrator present");
        assert_eq!(
            overridden_launch.mcp_fence_argv(),
            vec!["bora", "mcp", "serve", "--channels", "totally-different"]
        );
    }

    #[test]
    fn returns_none_when_the_project_has_no_declared_orchestrator() {
        let project = Project {
            name: None,
            channel: None,
            members: Vec::new(),
            orchestrator: None,
            sections: None,
            auto_join: true,
        };
        let members = member_dirs();
        let run_file = PathBuf::from("/Users/ary/Sites/cnb_hono/.bora/run.json");
        let socket = PathBuf::from("/tmp/bora.sock");
        let home = home_dir();

        assert!(
            compose_orchestrator_launch(&project, "cnb", &members, &run_file, &socket, &home)
                .is_none()
        );
    }
}
