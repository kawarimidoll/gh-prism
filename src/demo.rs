use crate::PrMetadata;
use crate::app::PrState;
use crate::github::comments::{IssueComment, ReviewComment, ReviewCommentUser, ReviewThread};
use crate::github::commits::{CommitAuthor, CommitDetail, CommitInfo};
use crate::github::files::{DiffFile, FileStatus};
use crate::github::review::ReviewSummary;
use std::collections::HashMap;

use crate::app::ReviewVerdict;

pub const DEMO_PR_NUMBER: u64 = 42;
pub const DEMO_REPO: &str = "kawarimidoll/gh-prism";
pub const DEMO_CURRENT_USER: &str = "kawarimidoll";

// ── Commit SHAs (定数化して files_map のキーと一致させる) ──

const SHA1: &str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
const SHA2: &str = "b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1";
const SHA3: &str = "c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2";

pub fn demo_metadata() -> PrMetadata {
    PrMetadata {
        pr_title: "Add dark mode support".to_string(),
        pr_body: r#"## Summary

Add dark mode support to the application with automatic OS-level detection.

## Changes

- **ThemeMode enum**: `Light` / `Dark` variants with `Default` derive
- **Color palette**: Semantic color tokens (foreground, background, accent, muted, border)
- **CLI flags**: `--light` / `--dark` to override auto-detection
- **OS detection**: Uses `termbg` crate to query terminal background color

## Screenshots

| Light | Dark |
|-------|------|
| (screenshot) | (screenshot) |

## Test plan

- [x] `cargo test` passes
- [x] Verified auto-detection in iTerm2 and Alacritty
- [x] `--light` and `--dark` flags override detection
"#
        .to_string(),
        pr_author: "octocat".to_string(),
        pr_base_branch: "main".to_string(),
        pr_head_branch: "feature/dark-mode".to_string(),
        pr_created_at: "2025-03-01 10:30 +0900".to_string(),
        pr_state: PrState::Open,
    }
}

pub fn demo_commits() -> Vec<CommitInfo> {
    vec![
        CommitInfo {
            sha: SHA1.to_string(),
            commit: CommitDetail {
                message: "feat: add ThemeMode enum and detection logic\n\nIntroduce ThemeMode::Light and ThemeMode::Dark variants.\nUse termbg crate to auto-detect terminal background color.".to_string(),
                author: Some(CommitAuthor {
                    name: "octocat".to_string(),
                    email: "octocat@github.com".to_string(),
                    date: "2025-03-01T10:30:00Z".to_string(),
                }),
            },
        },
        CommitInfo {
            sha: SHA2.to_string(),
            commit: CommitDetail {
                message: "feat: implement color palette for light/dark themes\n\nAdd semantic color tokens: foreground, background, accent, muted, border.\nEach token resolves to different ANSI colors based on ThemeMode.".to_string(),
                author: Some(CommitAuthor {
                    name: "octocat".to_string(),
                    email: "octocat@github.com".to_string(),
                    date: "2025-03-01T14:00:00Z".to_string(),
                }),
            },
        },
        CommitInfo {
            sha: SHA3.to_string(),
            commit: CommitDetail {
                message: "feat: add --light and --dark CLI flags\n\nAllow users to override auto-detected theme via CLI flags.\nFlags are mutually exclusive (clap conflicts_with).".to_string(),
                author: Some(CommitAuthor {
                    name: "octocat".to_string(),
                    email: "octocat@github.com".to_string(),
                    date: "2025-03-02T09:00:00Z".to_string(),
                }),
            },
        },
    ]
}

pub fn demo_files_map() -> HashMap<String, Vec<DiffFile>> {
    let mut map = HashMap::new();

    // Commit 1: ThemeMode enum + detection
    map.insert(
        SHA1.to_string(),
        vec![
            DiffFile {
                filename: "src/theme.rs".to_string(),
                status: FileStatus::Added,
                additions: 28,
                deletions: 0,
                patch: Some(
                    r#"@@ -0,0 +1,28 @@
+use std::fmt;
+
+#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
+pub enum ThemeMode {
+    #[default]
+    Dark,
+    Light,
+}
+
+impl fmt::Display for ThemeMode {
+    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        match self {
+            ThemeMode::Dark => write!(f, "dark"),
+            ThemeMode::Light => write!(f, "light"),
+        }
+    }
+}
+
+const THEME_DETECT_TIMEOUT_MS: u64 = 100;
+
+/// Detect terminal background color and return appropriate theme.
+/// Falls back to Dark mode if detection fails.
+pub fn detect_theme() -> ThemeMode {
+    match termbg::theme(std::time::Duration::from_millis(THEME_DETECT_TIMEOUT_MS)) {
+        Ok(termbg::Theme::Light) => ThemeMode::Light,
+        _ => ThemeMode::Dark,
+    }
+}"#
                    .to_string(),
                ),
            },
            DiffFile {
                filename: "src/main.rs".to_string(),
                status: FileStatus::Modified,
                additions: 3,
                deletions: 0,
                patch: Some(
                    r#"@@ -1,4 +1,7 @@
+mod theme;
+
 use clap::Parser;
+use theme::ThemeMode;

 fn main() {
+    let theme = theme::detect_theme();"#
                        .to_string(),
                ),
            },
        ],
    );

    // Commit 2: color palette
    map.insert(
        SHA2.to_string(),
        vec![
            DiffFile {
                filename: "src/palette.rs".to_string(),
                status: FileStatus::Added,
                additions: 40,
                deletions: 0,
                patch: Some(
                    r#"@@ -0,0 +1,40 @@
+use ratatui::style::Color;
+use crate::theme::ThemeMode;
+
+pub struct Palette {
+    pub foreground: Color,
+    pub background: Color,
+    pub accent: Color,
+    pub muted: Color,
+    pub border: Color,
+}
+
+impl Palette {
+    pub fn for_theme(mode: ThemeMode) -> Self {
+        match mode {
+            ThemeMode::Dark => Self {
+                foreground: Color::White,
+                background: Color::Rgb(30, 30, 46),
+                accent: Color::Rgb(137, 180, 250),
+                muted: Color::Rgb(108, 112, 134),
+                border: Color::Rgb(69, 71, 90),
+            },
+            ThemeMode::Light => Self {
+                foreground: Color::Rgb(76, 79, 105),
+                background: Color::Rgb(239, 241, 245),
+                accent: Color::Rgb(30, 102, 245),
+                muted: Color::Rgb(140, 143, 161),
+                border: Color::Rgb(188, 192, 204),
+            },
+        }
+    }
+}
+
+/// Convenience color for diff additions
+pub fn addition_color(mode: ThemeMode) -> Color {
+    match mode {
+        ThemeMode::Dark => Color::Rgb(166, 227, 161),
+        ThemeMode::Light => Color::Rgb(64, 160, 43),
+    }
+}"#
                    .to_string(),
                ),
            },
            DiffFile {
                filename: "src/theme.rs".to_string(),
                status: FileStatus::Modified,
                additions: 5,
                deletions: 0,
                patch: Some(
                    r#"@@ -26,3 +26,8 @@ pub fn detect_theme() -> ThemeMode {
         _ => ThemeMode::Dark,
     }
 }
+
+impl ThemeMode {
+    pub fn is_dark(self) -> bool {
+        self == ThemeMode::Dark
+    }
+}"#
                    .to_string(),
                ),
            },
        ],
    );

    // Commit 3: CLI flags
    map.insert(
        SHA3.to_string(),
        vec![
            DiffFile {
                filename: "src/main.rs".to_string(),
                status: FileStatus::Modified,
                additions: 15,
                deletions: 2,
                patch: Some(
                    r#"@@ -8,6 +8,15 @@ struct Cli {
     #[arg(short, long)]
     repo: Option<String>,

+    /// Force light theme
+    #[arg(long, conflicts_with = "dark")]
+    light: bool,
+
+    /// Force dark theme
+    #[arg(long, conflicts_with = "light")]
+    dark: bool,
+}
+
 fn main() {
-    let theme = theme::detect_theme();
+    let theme = if cli.light {
+        ThemeMode::Light
+    } else if cli.dark {
+        ThemeMode::Dark
+    } else {
+        theme::detect_theme()
+    };"#
                        .to_string(),
                ),
            },
            DiffFile {
                filename: "tests/cli_test.rs".to_string(),
                status: FileStatus::Added,
                additions: 18,
                deletions: 0,
                patch: Some(
                    r#"@@ -0,0 +1,18 @@
+#[test]
+fn test_light_dark_conflict() {
+    use assert_cmd::Command;
+
+    let result = Command::cargo_bin("prism")
+        .unwrap()
+        .args(["--light", "--dark", "1"])
+        .assert();
+
+    result.failure();
+}
+
+#[test]
+fn test_dark_flag() {
+    // Verify that --dark flag is accepted without error
+    // (actual theme verification requires terminal)
+    assert!(true);
+}"#
                    .to_string(),
                ),
            },
        ],
    );

    map
}

pub fn demo_review_comments() -> Vec<ReviewComment> {
    vec![
        // Thread 1: unresolved - palette question (root)
        ReviewComment {
            id: 1001,
            body: "Should we use Catppuccin colors here? The current palette looks custom.\nIt might be better to align with an established color scheme for consistency."
                .to_string(),
            path: "src/palette.rs".to_string(),
            line: Some(16),
            start_line: None,
            side: None,
            start_side: None,
            commit_id: SHA2.to_string(),
            user: ReviewCommentUser {
                login: "reviewer-alice".to_string(),
            },
            created_at: "2025-03-02T10:00:00Z".to_string(),
            in_reply_to_id: None,
            pull_request_review_id: Some(5001),
        },
        // Thread 1: reply from author
        ReviewComment {
            id: 1002,
            body: "Good idea! These are actually Catppuccin Mocha values. I'll add a comment to document the source.".to_string(),
            path: "src/palette.rs".to_string(),
            line: Some(16),
            start_line: None,
            side: None,
            start_side: None,
            commit_id: SHA2.to_string(),
            user: ReviewCommentUser {
                login: "octocat".to_string(),
            },
            created_at: "2025-03-02T11:00:00Z".to_string(),
            in_reply_to_id: Some(1001),
            pull_request_review_id: None,
        },
        // Thread 2: unresolved - timeout concern (root)
        ReviewComment {
            id: 1003,
            body: "100ms timeout might be too short for some terminal emulators.\nHave you tested this on slower SSH connections?".to_string(),
            path: "src/theme.rs".to_string(),
            line: Some(19),
            start_line: None,
            side: None,
            start_side: None,
            commit_id: SHA1.to_string(),
            user: ReviewCommentUser {
                login: "reviewer-bob".to_string(),
            },
            created_at: "2025-03-02T12:00:00Z".to_string(),
            in_reply_to_id: None,
            pull_request_review_id: Some(5002),
        },
        // Thread 3: resolved - naming convention (root)
        ReviewComment {
            id: 1004,
            body: "Nit: `is_dark` could just be `!matches!(self, Self::Light)` for future extensibility.".to_string(),
            path: "src/theme.rs".to_string(),
            line: Some(30),
            start_line: None,
            side: None,
            start_side: None,
            commit_id: SHA2.to_string(),
            user: ReviewCommentUser {
                login: "reviewer-alice".to_string(),
            },
            created_at: "2025-03-02T10:30:00Z".to_string(),
            in_reply_to_id: None,
            pull_request_review_id: Some(5001),
        },
        // Thread 3: reply + resolved
        ReviewComment {
            id: 1005,
            body: "Fixed in the latest push. Thanks!".to_string(),
            path: "src/theme.rs".to_string(),
            line: Some(30),
            start_line: None,
            side: None,
            start_side: None,
            commit_id: SHA2.to_string(),
            user: ReviewCommentUser {
                login: "octocat".to_string(),
            },
            created_at: "2025-03-02T13:00:00Z".to_string(),
            in_reply_to_id: Some(1004),
            pull_request_review_id: None,
        },
    ]
}

pub fn demo_issue_comments() -> Vec<IssueComment> {
    vec![
        IssueComment {
            id: 2001,
            body: Some(
                "Is this going to support system-level theme detection on macOS/Windows as well, or just terminal-level?"
                    .to_string(),
            ),
            user: ReviewCommentUser {
                login: "curious-user".to_string(),
            },
            created_at: "2025-03-01T15:00:00Z".to_string(),
        },
        IssueComment {
            id: 2002,
            body: Some(
                "For now it's terminal-level only using the `termbg` crate (OSC 11 query). System-level detection could be a follow-up via `dark-light` crate."
                    .to_string(),
            ),
            user: ReviewCommentUser {
                login: "octocat".to_string(),
            },
            created_at: "2025-03-01T16:00:00Z".to_string(),
        },
    ]
}

pub fn demo_reviews() -> Vec<ReviewSummary> {
    vec![
        ReviewSummary {
            id: 5001,
            user: ReviewCommentUser {
                login: "reviewer-alice".to_string(),
            },
            body: Some(
                "Nice work overall! A few minor suggestions on the palette and naming.".to_string(),
            ),
            state: ReviewVerdict::ChangesRequested,
            submitted_at: Some("2025-03-02T10:00:00Z".to_string()),
        },
        ReviewSummary {
            id: 5002,
            user: ReviewCommentUser {
                login: "reviewer-bob".to_string(),
            },
            body: Some(
                "LGTM after the timeout discussion is resolved. The overall approach is solid."
                    .to_string(),
            ),
            state: ReviewVerdict::Approved,
            submitted_at: Some("2025-03-02T12:00:00Z".to_string()),
        },
    ]
}

pub fn demo_review_threads() -> Vec<ReviewThread> {
    vec![
        // Thread 1: unresolved (palette question)
        ReviewThread {
            node_id: "RT_demo_001".to_string(),
            is_resolved: false,
            root_comment_database_id: 1001,
        },
        // Thread 2: unresolved (timeout concern)
        ReviewThread {
            node_id: "RT_demo_002".to_string(),
            is_resolved: false,
            root_comment_database_id: 1003,
        },
        // Thread 3: resolved (naming nit)
        ReviewThread {
            node_id: "RT_demo_003".to_string(),
            is_resolved: true,
            root_comment_database_id: 1004,
        },
    ]
}
