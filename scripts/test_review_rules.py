from __future__ import annotations

import unittest

from scripts.review_rules import (
    Commit,
    check_allow_justification,
    check_generated_paths,
    check_issue_closing_keywords,
    check_version_bump,
    extract_package_version,
    parse_added_lines,
    parse_commits,
    parse_name_status,
    render_report,
)

CARGO_TOML = """\
[package]
name = "bora"
version = "0.20.1"

[dependencies.foo]
version = "9.9.9"
"""


class ExtractPackageVersionTests(unittest.TestCase):
    def test_reads_the_package_table_version(self) -> None:
        self.assertEqual(extract_package_version(CARGO_TOML), "0.20.1")

    def test_ignores_a_version_key_in_a_dependency_table(self) -> None:
        # A naive "last version = line in the file" parse would return the
        # dependency's "9.9.9" here instead of the package's "0.20.1".
        toml_without_package_version = """\
[package]
name = "bora"

[dependencies.foo]
version = "9.9.9"
"""
        self.assertIsNone(extract_package_version(toml_without_package_version))


class VersionBumpCheckTests(unittest.TestCase):
    def test_flags_unbumped_version_when_rust_source_changes(self) -> None:
        findings = check_version_bump(["src/main.rs"], CARGO_TOML, CARGO_TOML)
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].severity, "high")
        self.assertEqual(findings[0].location, "Cargo.toml")

    def test_does_not_flag_when_package_version_is_bumped(self) -> None:
        head_toml = CARGO_TOML.replace('version = "0.20.1"', 'version = "0.20.2"')
        findings = check_version_bump(["src/main.rs"], CARGO_TOML, head_toml)
        self.assertEqual(findings, [])

    def test_does_not_flag_a_docs_only_change(self) -> None:
        findings = check_version_bump(
            ["docs/next/website/src/content/docs/foo.md"], CARGO_TOML, CARGO_TOML
        )
        self.assertEqual(findings, [])

    def test_dependency_table_bump_alone_still_flags_missing_package_bump(self) -> None:
        # Only the dependency's version changed; [package].version is unchanged.
        # A checker confused about which "version" it read would wrongly pass this.
        head_toml = CARGO_TOML.replace('version = "9.9.9"', 'version = "9.9.10"')
        findings = check_version_bump(["Cargo.toml"], CARGO_TOML, head_toml)
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].severity, "high")


class GeneratedPathsCheckTests(unittest.TestCase):
    def test_flags_hand_edits_to_generated_or_published_paths(self) -> None:
        findings = check_generated_paths(
            [
                "docs/versions/0.20.0/index.md",
                "docs/preview/website/index.html",
                "website/src/content/docs/guide.md",
                "website/latest.json",
                "website/preview.json",
            ]
        )
        self.assertEqual(len(findings), 5)
        self.assertTrue(all(f.severity == "critical" for f in findings))

    def test_does_not_flag_draft_docs_or_unrelated_website_files(self) -> None:
        findings = check_generated_paths(
            ["docs/next/website/src/content/docs/guide.md", "website/README.md"]
        )
        self.assertEqual(findings, [])


class AllowJustificationCheckTests(unittest.TestCase):
    def test_flags_allow_with_no_justification(self) -> None:
        added = {"src/lib.rs": [(10, "#[allow(dead_code)]")]}
        head_lines = {"src/lib.rs": [f"line {n}" for n in range(1, 9)] + ["fn unused() {}", "#[allow(dead_code)]"]}
        findings = check_allow_justification(added, head_lines)
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].severity, "medium")
        self.assertEqual(findings[0].location, "src/lib.rs")

    def test_does_not_flag_allow_with_trailing_comment(self) -> None:
        added = {"src/lib.rs": [(5, "#[allow(dead_code)] // kept for the FFI shim")]}
        findings = check_allow_justification(added, {"src/lib.rs": []})
        self.assertEqual(findings, [])

    def test_does_not_flag_allow_with_preceding_comment_line(self) -> None:
        added = {"src/lib.rs": [(5, "#[allow(dead_code)]")]}
        head_lines = {
            "src/lib.rs": [
                "fn a() {}",
                "fn b() {}",
                "fn c() {}",
                "// kept for the FFI shim",
                "#[allow(dead_code)]",
            ]
        }
        findings = check_allow_justification(added, head_lines)
        self.assertEqual(findings, [])


class IssueClosingKeywordCheckTests(unittest.TestCase):
    def test_flags_closing_keyword_bound_to_an_issue_number(self) -> None:
        commit = Commit(sha="abc123", subject="fixes #123", body="")
        findings = check_issue_closing_keywords([commit])
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].severity, "high")
        self.assertEqual(findings[0].location, "abc123")

    def test_flags_closing_keyword_bound_to_a_full_issue_url(self) -> None:
        commit = Commit(
            sha="def456",
            subject="cleanup",
            body="Closes https://github.com/herdrdev/herdr/issues/45",
        )
        findings = check_issue_closing_keywords([commit])
        self.assertEqual(len(findings), 1)

    def test_does_not_flag_conventional_fix_subject(self) -> None:
        commit = Commit(sha="aaa111", subject="fix: handle pane focus", body="")
        self.assertEqual(check_issue_closing_keywords([commit]), [])

    def test_does_not_flag_bare_refs(self) -> None:
        commit = Commit(sha="bbb222", subject="fix: handle pane focus", body="refs #82")
        self.assertEqual(check_issue_closing_keywords([commit]), [])


class ParserTests(unittest.TestCase):
    def test_parse_name_status_uses_new_path_for_renames(self) -> None:
        text = "M\tsrc/lib.rs\nR100\told/path.rs\tnew/path.rs\nA\tdocs/next/foo.md\n"
        self.assertEqual(
            parse_name_status(text),
            [("M", "src/lib.rs"), ("R100", "new/path.rs"), ("A", "docs/next/foo.md")],
        )

    def test_parse_added_lines_tracks_new_file_line_numbers(self) -> None:
        diff_text = (
            "diff --git a/src/lib.rs b/src/lib.rs\n"
            "index 111..222 100644\n"
            "--- a/src/lib.rs\n"
            "+++ b/src/lib.rs\n"
            "@@ -8,0 +9,2 @@ fn old_context() {\n"
            "+#[allow(dead_code)]\n"
            "+fn unused() {}\n"
        )
        self.assertEqual(
            parse_added_lines(diff_text),
            {"src/lib.rs": [(9, "#[allow(dead_code)]"), (10, "fn unused() {}")]},
        )

    def test_parse_commits_splits_nul_and_record_separated_log(self) -> None:
        log_text = "\x1eabc\x1ffix: handle pane focus\x1frefs #82\n\x1edef\x1ffixes #9\x1f\n"
        commits = parse_commits(log_text)
        self.assertEqual(
            commits,
            [
                Commit(sha="abc", subject="fix: handle pane focus", body="refs #82"),
                Commit(sha="def", subject="fixes #9", body=""),
            ],
        )


class RenderReportTests(unittest.TestCase):
    def test_reports_no_findings(self) -> None:
        report = render_report([])
        self.assertIn("No findings.", report)
        self.assertEqual(report.splitlines()[-1], "VERDICT: 0 critical, 0 high, 0 medium, 0 low")

    def test_reports_findings_grouped_with_matching_verdict_counts(self) -> None:
        findings = check_generated_paths(["website/latest.json"]) + check_issue_closing_keywords(
            [Commit(sha="abc123", subject="fixes #1", body="")]
        )
        report = render_report(findings)
        self.assertIn("CRITICAL - website/latest.json:", report)
        self.assertIn("HIGH - abc123:", report)
        self.assertEqual(report.splitlines()[-1], "VERDICT: 1 critical, 1 high, 0 medium, 0 low")


if __name__ == "__main__":
    unittest.main()
