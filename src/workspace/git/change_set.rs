use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceChangeSet {
    pub sections: Vec<ChangeSection>,
    pub base_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSection {
    pub kind: ChangeSectionKind,
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeSectionKind {
    Unstaged,
    Staged,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub status: ChangeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Like `git_trimmed_stdout` but returns `Some("")` on empty-success instead of `None`.
/// Returns `None` only when the process itself fails (non-zero exit or spawn error).
fn git_stdout_or_empty(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(stdout.trim().to_string())
}

/// Resolve a rename path produced by `git diff --numstat`.
///
/// Two forms:
/// - Arrow:  `old/path => new/path`  → `new/path`
/// - Brace:  `dir/{old => new}/file` → `dir/new/file`
fn resolve_rename_path(raw: &str) -> String {
    // Brace form: something/{a => b}/something
    if let Some(open) = raw.find('{') {
        if let Some(close) = raw[open..].find('}') {
            let close = open + close;
            let prefix = &raw[..open];
            let suffix = &raw[close + 1..];
            let inner = &raw[open + 1..close]; // "a => b"
            if let Some((_old, new)) = inner.split_once("=>") {
                let new = new.trim();
                return format!("{}{}{}", prefix, new, suffix);
            }
        }
    }
    // Arrow form: old => new
    if let Some((_old, new)) = raw.split_once("=>") {
        return new.trim().to_string();
    }
    raw.to_string()
}

// ── Parsers ───────────────────────────────────────────────────────────────────

/// Parse `git diff --numstat` output.
///
/// Returns `(added, removed, path)` per line.  
/// `-` in added/removed means binary → `None`.
pub(super) fn parse_numstat(input: &str) -> Vec<(Option<u32>, Option<u32>, String)> {
    let mut out = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(added_raw), Some(removed_raw), Some(path_raw)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let added = if added_raw == "-" {
            None
        } else {
            added_raw.parse::<u32>().ok()
        };
        let removed = if removed_raw == "-" {
            None
        } else {
            removed_raw.parse::<u32>().ok()
        };
        let path = resolve_rename_path(path_raw.trim());
        out.push((added, removed, path));
    }
    out
}

/// Parse `git status --porcelain=v1` output.
///
/// Returns `(path, ChangeStatus)` per entry.
pub(super) fn parse_porcelain_status(input: &str) -> Vec<(String, ChangeStatus)> {
    let mut out = Vec::new();
    for line in input.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let rest = &line[3..]; // skip "XY "

        let x = xy.chars().next().unwrap_or(' ');
        let y = xy.chars().nth(1).unwrap_or(' ');

        if xy == "??" {
            out.push((rest.to_string(), ChangeStatus::Untracked));
            continue;
        }

        // Renamed: use the new path (after "->")
        if x == 'R' || y == 'R' {
            let path = if let Some((_old, new)) = rest.split_once(" -> ") {
                new.trim().to_string()
            } else {
                rest.to_string()
            };
            out.push((path, ChangeStatus::Renamed));
            continue;
        }

        let status = if x == 'A' || y == 'A' {
            ChangeStatus::Added
        } else if x == 'D' || y == 'D' {
            ChangeStatus::Deleted
        } else {
            ChangeStatus::Modified
        };

        out.push((rest.to_string(), status));
    }
    out
}

/// Merge numstat rows with porcelain status into `ChangedFile` entries.
fn merge_numstat_with_status(
    numstat: Vec<(Option<u32>, Option<u32>, String)>,
    status_map: &std::collections::HashMap<String, ChangeStatus>,
) -> Vec<ChangedFile> {
    numstat
        .into_iter()
        .map(|(added, removed, path)| {
            let status = status_map
                .get(&path)
                .cloned()
                .unwrap_or(ChangeStatus::Modified);
            ChangedFile { path, added, removed, status }
        })
        .collect()
}

/// Build `ChangeSection` list from raw git output strings.
pub(super) fn build_change_sections(
    unstaged_numstat: &str,
    staged_numstat: &str,
    committed_numstat: Option<&str>,
    porcelain: &str,
) -> Vec<ChangeSection> {
    let status_pairs = parse_porcelain_status(porcelain);
    let status_map: std::collections::HashMap<String, ChangeStatus> =
        status_pairs.into_iter().collect();

    let mut sections = Vec::new();

    let unstaged_files =
        merge_numstat_with_status(parse_numstat(unstaged_numstat), &status_map);
    if !unstaged_files.is_empty() {
        sections.push(ChangeSection {
            kind: ChangeSectionKind::Unstaged,
            files: unstaged_files,
        });
    }

    let staged_files = merge_numstat_with_status(parse_numstat(staged_numstat), &status_map);
    if !staged_files.is_empty() {
        sections.push(ChangeSection {
            kind: ChangeSectionKind::Staged,
            files: staged_files,
        });
    }

    if let Some(committed_raw) = committed_numstat {
        let committed_files: Vec<ChangedFile> = parse_numstat(committed_raw)
            .into_iter()
            .map(|(added, removed, path)| {
                // ponytail: upgrade path — use git diff --name-status <base>...HEAD for accurate committed status
                ChangedFile {
                    path,
                    added,
                    removed,
                    status: ChangeStatus::Modified,
                }
            })
            .collect();
        if !committed_files.is_empty() {
            sections.push(ChangeSection {
                kind: ChangeSectionKind::Committed,
                files: committed_files,
            });
        }
    }

    sections
}

// ── Acquisition ───────────────────────────────────────────────────────────────

pub(super) fn compute_change_set(
    cwd: &Path,
    upstream_ref: Option<&str>,
) -> Option<WorkspaceChangeSet> {
    let unstaged = git_stdout_or_empty(cwd, &["diff", "--numstat"]).unwrap_or_else(|| {
        tracing::debug!("change_set: git diff --numstat failed for {:?}", cwd);
        String::new()
    });

    let staged =
        git_stdout_or_empty(cwd, &["diff", "--cached", "--numstat"]).unwrap_or_else(|| {
            tracing::debug!("change_set: git diff --cached --numstat failed for {:?}", cwd);
            String::new()
        });

    let committed: Option<String> = upstream_ref.and_then(|uref| {
        let range = format!("{}...HEAD", uref);
        git_stdout_or_empty(cwd, &["diff", "--numstat", &range]).or_else(|| {
            tracing::debug!(
                "change_set: git diff --numstat {}...HEAD failed for {:?}",
                uref,
                cwd
            );
            None
        })
    });

    let porcelain = match git_stdout_or_empty(cwd, &["status", "--porcelain=v1"]) {
        Some(s) => s,
        None => {
            tracing::debug!("change_set: git status --porcelain=v1 failed for {:?}", cwd);
            // All git commands effectively failed; distinguish total failure:
            // if unstaged and staged are both empty because git isn't available,
            // we still have a cwd that isn't a repo — return None.
            // ponytail: we only hard-fail on porcelain since it's the lightest probe
            return None;
        }
    };

    let sections = build_change_sections(
        &unstaged,
        &staged,
        committed.as_deref(),
        &porcelain,
    );

    Some(WorkspaceChangeSet {
        sections,
        base_ref: upstream_ref.map(str::to_string),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_numstat ─────────────────────────────────────────────────────────

    #[test]
    fn test_parse_numstat_basic() {
        let input = "3\t1\tsrc/main.rs\n10\t0\tsrc/new.rs\n0\t5\tsrc/old.rs\n";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], (Some(3), Some(1), "src/main.rs".to_string()));
        assert_eq!(result[1], (Some(10), Some(0), "src/new.rs".to_string()));
        assert_eq!(result[2], (Some(0), Some(5), "src/old.rs".to_string()));
    }

    #[test]
    fn test_parse_numstat_binary() {
        let input = "-\t-\tassets/logo.png\n";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (None, None, "assets/logo.png".to_string()));
    }

    #[test]
    fn test_parse_numstat_rename_arrow() {
        let input = "5\t2\told/path.rs => new/path.rs\n";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].2, "new/path.rs");
    }

    #[test]
    fn test_parse_numstat_rename_brace() {
        let input = "3\t1\tsrc/{old => new}/file.rs\n";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].2, "src/new/file.rs");
    }

    #[test]
    fn test_parse_numstat_empty() {
        assert!(parse_numstat("").is_empty());
        assert!(parse_numstat("   \n\n").is_empty());
    }

    // ── parse_porcelain_status ────────────────────────────────────────────────

    #[test]
    fn test_parse_porcelain_modified() {
        let input = " M src/lib.rs\n";
        let result = parse_porcelain_status(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("src/lib.rs".to_string(), ChangeStatus::Modified));
    }

    #[test]
    fn test_parse_porcelain_added() {
        let input = "A  src/new.rs\n";
        let result = parse_porcelain_status(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("src/new.rs".to_string(), ChangeStatus::Added));
    }

    #[test]
    fn test_parse_porcelain_deleted() {
        let input = "D  src/gone.rs\n";
        let result = parse_porcelain_status(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("src/gone.rs".to_string(), ChangeStatus::Deleted));
    }

    #[test]
    fn test_parse_porcelain_untracked() {
        let input = "?? src/untracked.rs\n";
        let result = parse_porcelain_status(input);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            ("src/untracked.rs".to_string(), ChangeStatus::Untracked)
        );
    }

    #[test]
    fn test_parse_porcelain_renamed() {
        let input = "R  old.rs -> new.rs\n";
        let result = parse_porcelain_status(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("new.rs".to_string(), ChangeStatus::Renamed));
    }

    // ── build_change_sections ─────────────────────────────────────────────────

    #[test]
    fn test_build_sections_combines_numstat_and_porcelain() {
        let unstaged = "3\t1\tsrc/main.rs\n";
        let staged = "5\t0\tsrc/new.rs\n";
        let porcelain = " M src/main.rs\nA  src/new.rs\n";

        let sections = build_change_sections(unstaged, staged, None, porcelain);
        assert_eq!(sections.len(), 2);

        let unstaged_sec = &sections[0];
        assert_eq!(unstaged_sec.kind, ChangeSectionKind::Unstaged);
        assert_eq!(unstaged_sec.files.len(), 1);
        assert_eq!(unstaged_sec.files[0].status, ChangeStatus::Modified);
        assert_eq!(unstaged_sec.files[0].path, "src/main.rs");

        let staged_sec = &sections[1];
        assert_eq!(staged_sec.kind, ChangeSectionKind::Staged);
        assert_eq!(staged_sec.files.len(), 1);
        assert_eq!(staged_sec.files[0].status, ChangeStatus::Added);
        assert_eq!(staged_sec.files[0].path, "src/new.rs");
    }

    #[test]
    fn test_build_sections_empty_input() {
        let sections = build_change_sections("", "", None, "");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_build_sections_committed() {
        let committed = "2\t1\tsrc/lib.rs\n";
        let sections = build_change_sections("", "", Some(committed), "");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].kind, ChangeSectionKind::Committed);
        assert_eq!(sections[0].files[0].status, ChangeStatus::Modified);
    }

    #[test]
    fn test_build_sections_defaults_to_modified_without_porcelain_match() {
        let unstaged = "1\t0\tsrc/mystery.rs\n";
        // porcelain has no entry for this path
        let sections = build_change_sections(unstaged, "", None, "");
        assert_eq!(sections[0].files[0].status, ChangeStatus::Modified);
    }
}
