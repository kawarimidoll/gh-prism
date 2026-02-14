use crate::github::comments::ReviewCommentUser;
use crate::github::files::DiffFile;
use color_eyre::{Result, eyre::eyre};
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 保留中のレビューコメント
#[derive(Debug, Clone)]
pub struct PendingComment {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub body: String,
    pub commit_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    #[serde(rename = "LEFT")]
    Left,
    #[serde(rename = "RIGHT")]
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct DiffLineInfo {
    pub file_line: usize,
    pub side: Side,
}

/// patch テキストの各行 → 実ファイル行番号。@@ 行は None。
pub fn parse_patch_line_map(patch: &str) -> Vec<Option<DiffLineInfo>> {
    let mut result = Vec::new();
    let mut old_line: usize = 0;
    let mut new_line: usize = 0;

    for line in patch.lines() {
        if line.starts_with("@@") {
            // @@ -old,len +new,len @@ のパース
            if let Some((old, new)) = parse_hunk_header(line) {
                old_line = old;
                new_line = new;
            }
            result.push(None);
        } else if let Some(_rest) = line.strip_prefix('-') {
            result.push(Some(DiffLineInfo {
                file_line: old_line,
                side: Side::Left,
            }));
            old_line += 1;
        } else if let Some(_rest) = line.strip_prefix('+') {
            result.push(Some(DiffLineInfo {
                file_line: new_line,
                side: Side::Right,
            }));
            new_line += 1;
        } else {
            // コンテキスト行
            result.push(Some(DiffLineInfo {
                file_line: new_line,
                side: Side::Right,
            }));
            old_line += 1;
            new_line += 1;
        }
    }

    result
}

/// @@ -old,len +new,len @@ からold開始行とnew開始行を抽出
pub fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    // 形式: @@ -old_start[,old_len] +new_start[,new_len] @@
    let line = line.strip_prefix("@@ ")?;
    let at_end = line.find(" @@")?;
    let range_part = &line[..at_end];

    let mut parts = range_part.split_whitespace();
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;

    let old_start: usize = old_part.split(',').next()?.parse().ok()?;
    let new_start: usize = new_part.split(',').next()?.parse().ok()?;

    Some((old_start, new_start))
}

/// PR レビュー概要（APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED）
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewSummary {
    pub id: u64,
    pub user: ReviewCommentUser,
    pub body: Option<String>,
    pub state: String,
    pub submitted_at: Option<String>,
}

/// PR Reviews API でレビュー一覧を取得
pub async fn fetch_reviews(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<ReviewSummary>> {
    let url = format!("/repos/{}/{}/pulls/{}/reviews", owner, repo, pr_number);
    let reviews: Vec<ReviewSummary> = client.get(url, None::<&()>).await?;
    Ok(reviews)
}

#[derive(Debug, Serialize)]
struct ReviewComment {
    path: String,
    body: String,
    line: usize,
    side: Side,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_side: Option<Side>,
}

#[derive(Serialize)]
struct CreateReviewRequest {
    commit_id: String,
    body: String,
    event: String,
    comments: Vec<ReviewComment>,
}

/// PendingComment から ReviewComment を構築
fn build_review_comment(pending: &PendingComment, files: &[DiffFile]) -> Result<ReviewComment> {
    let file = files
        .iter()
        .find(|f| f.filename == pending.file_path)
        .ok_or_else(|| eyre!("File not found: {}", pending.file_path))?;

    let patch = file
        .patch
        .as_deref()
        .ok_or_else(|| eyre!("No patch for file: {}", pending.file_path))?;

    let line_map = parse_patch_line_map(patch);

    let end_info = line_map
        .get(pending.end_line)
        .and_then(|info| info.as_ref())
        .ok_or_else(|| {
            eyre!(
                "Cannot comment on hunk header line (end_line={})",
                pending.end_line
            )
        })?;

    if pending.start_line == pending.end_line {
        // single-line コメント
        Ok(ReviewComment {
            path: pending.file_path.clone(),
            body: pending.body.clone(),
            line: end_info.file_line,
            side: end_info.side,
            start_line: None,
            start_side: None,
        })
    } else {
        // multi-line コメント
        let start_info = line_map
            .get(pending.start_line)
            .and_then(|info| info.as_ref())
            .ok_or_else(|| {
                eyre!(
                    "Cannot comment on hunk header line (start_line={})",
                    pending.start_line
                )
            })?;

        Ok(ReviewComment {
            path: pending.file_path.clone(),
            body: pending.body.clone(),
            line: end_info.file_line,
            side: end_info.side,
            start_line: Some(start_info.file_line),
            start_side: Some(start_info.side),
        })
    }
}

/// レビュー送信に必要な接続コンテキスト
pub struct ReviewContext<'a> {
    pub client: &'a Octocrab,
    pub owner: &'a str,
    pub repo: &'a str,
    pub pr_number: u64,
}

/// 保留中のコメントを GitHub PR Review API に一括送信
pub async fn submit_review(
    ctx: &ReviewContext<'_>,
    head_sha: &str,
    pending_comments: &[PendingComment],
    files_map: &HashMap<String, Vec<DiffFile>>,
    event: &str,
    body: &str,
) -> Result<()> {
    let mut comments = Vec::new();

    for pending in pending_comments {
        let files = files_map
            .get(&pending.commit_sha)
            .ok_or_else(|| eyre!("No files found for commit: {}", pending.commit_sha))?;

        let comment = build_review_comment(pending, files)?;
        comments.push(comment);
    }

    let request = CreateReviewRequest {
        commit_id: head_sha.to_string(),
        body: body.to_string(),
        event: event.to_string(),
        comments,
    };

    let url = format!(
        "/repos/{}/{}/pulls/{}/reviews",
        ctx.owner, ctx.repo, ctx.pr_number
    );
    ctx.client
        .post::<_, serde_json::Value>(url, Some(&request))
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header_basic() {
        let result = parse_hunk_header("@@ -1,5 +1,7 @@");
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn test_parse_hunk_header_different_starts() {
        let result = parse_hunk_header("@@ -10,3 +20,5 @@");
        assert_eq!(result, Some((10, 20)));
    }

    #[test]
    fn test_parse_hunk_header_no_len() {
        // len が省略される場合（1行のみの変更）
        let result = parse_hunk_header("@@ -1 +1 @@");
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn test_parse_hunk_header_with_context() {
        // @@ の後にコンテキスト情報がある場合
        let result = parse_hunk_header("@@ -1,5 +1,7 @@ fn main() {");
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn test_parse_patch_line_map_add_only() {
        let patch = "@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3";
        let map = parse_patch_line_map(patch);

        assert_eq!(map.len(), 4);
        assert!(map[0].is_none()); // hunk header
        assert_eq!(map[1].unwrap().file_line, 1);
        assert_eq!(map[1].unwrap().side, Side::Right);
        assert_eq!(map[2].unwrap().file_line, 2);
        assert_eq!(map[3].unwrap().file_line, 3);
    }

    #[test]
    fn test_parse_patch_line_map_delete_and_add() {
        let patch = "@@ -1,2 +1,2 @@\n-old1\n-old2\n+new1\n+new2";
        let map = parse_patch_line_map(patch);

        assert_eq!(map.len(), 5);
        assert!(map[0].is_none()); // hunk header

        // 削除行: Left side, old_line
        assert_eq!(map[1].unwrap().file_line, 1);
        assert_eq!(map[1].unwrap().side, Side::Left);
        assert_eq!(map[2].unwrap().file_line, 2);
        assert_eq!(map[2].unwrap().side, Side::Left);

        // 追加行: Right side, new_line
        assert_eq!(map[3].unwrap().file_line, 1);
        assert_eq!(map[3].unwrap().side, Side::Right);
        assert_eq!(map[4].unwrap().file_line, 2);
        assert_eq!(map[4].unwrap().side, Side::Right);
    }

    #[test]
    fn test_parse_patch_line_map_with_context() {
        let patch = "@@ -1,3 +1,4 @@\n context\n-old\n+new1\n+new2\n context2";
        let map = parse_patch_line_map(patch);

        assert_eq!(map.len(), 6);
        assert!(map[0].is_none()); // hunk header

        // コンテキスト行: Right side
        assert_eq!(map[1].unwrap().file_line, 1);
        assert_eq!(map[1].unwrap().side, Side::Right);

        // 削除行: Left side
        assert_eq!(map[2].unwrap().file_line, 2);
        assert_eq!(map[2].unwrap().side, Side::Left);

        // 追加行: Right side
        assert_eq!(map[3].unwrap().file_line, 2);
        assert_eq!(map[3].unwrap().side, Side::Right);
        assert_eq!(map[4].unwrap().file_line, 3);
        assert_eq!(map[4].unwrap().side, Side::Right);

        // コンテキスト行
        assert_eq!(map[5].unwrap().file_line, 4);
        assert_eq!(map[5].unwrap().side, Side::Right);
    }

    #[test]
    fn test_parse_patch_line_map_multiple_hunks() {
        let patch = "@@ -1,2 +1,2 @@\n-old1\n+new1\n@@ -10,2 +10,2 @@\n-old10\n+new10";
        let map = parse_patch_line_map(patch);

        assert_eq!(map.len(), 6);
        assert!(map[0].is_none()); // 1st hunk header
        assert_eq!(map[1].unwrap().file_line, 1);
        assert_eq!(map[1].unwrap().side, Side::Left);
        assert_eq!(map[2].unwrap().file_line, 1);
        assert_eq!(map[2].unwrap().side, Side::Right);

        assert!(map[3].is_none()); // 2nd hunk header
        assert_eq!(map[4].unwrap().file_line, 10);
        assert_eq!(map[4].unwrap().side, Side::Left);
        assert_eq!(map[5].unwrap().file_line, 10);
        assert_eq!(map[5].unwrap().side, Side::Right);
    }

    #[test]
    fn test_build_review_comment_single_line() {
        let files = vec![DiffFile {
            filename: "src/main.rs".to_string(),
            status: "modified".to_string(),
            additions: 1,
            deletions: 1,
            patch: Some("@@ -1,2 +1,2 @@\n-old\n+new".to_string()),
        }];

        let pending = PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 2, // +new line
            end_line: 2,
            body: "Nice change!".to_string(),
            commit_sha: "abc123".to_string(),
        };

        let comment = build_review_comment(&pending, &files).unwrap();
        assert_eq!(comment.path, "src/main.rs");
        assert_eq!(comment.body, "Nice change!");
        assert_eq!(comment.line, 1); // file line 1 on RIGHT
        assert_eq!(comment.side, Side::Right);
        assert!(comment.start_line.is_none());
        assert!(comment.start_side.is_none());
    }

    #[test]
    fn test_build_review_comment_multi_line() {
        let files = vec![DiffFile {
            filename: "src/main.rs".to_string(),
            status: "added".to_string(),
            additions: 3,
            deletions: 0,
            patch: Some("@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3".to_string()),
        }];

        let pending = PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 1, // +line1
            end_line: 3,   // +line3
            body: "Good block".to_string(),
            commit_sha: "abc123".to_string(),
        };

        let comment = build_review_comment(&pending, &files).unwrap();
        assert_eq!(comment.line, 3); // end: file line 3
        assert_eq!(comment.side, Side::Right);
        assert_eq!(comment.start_line, Some(1)); // start: file line 1
        assert_eq!(comment.start_side, Some(Side::Right));
    }

    #[test]
    fn test_build_review_comment_hunk_header_error() {
        let files = vec![DiffFile {
            filename: "src/main.rs".to_string(),
            status: "modified".to_string(),
            additions: 1,
            deletions: 0,
            patch: Some("@@ -1,1 +1,2 @@\n line1\n+line2".to_string()),
        }];

        let pending = PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 0, // hunk header (None)
            end_line: 0,
            body: "Comment".to_string(),
            commit_sha: "abc123".to_string(),
        };

        let result = build_review_comment(&pending, &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("hunk header"));
    }

    #[test]
    fn test_build_review_comment_file_not_found() {
        let files = vec![DiffFile {
            filename: "src/main.rs".to_string(),
            status: "modified".to_string(),
            additions: 1,
            deletions: 0,
            patch: Some("@@ -1,1 +1,1 @@\n+line".to_string()),
        }];

        let pending = PendingComment {
            file_path: "nonexistent.rs".to_string(),
            start_line: 1,
            end_line: 1,
            body: "Comment".to_string(),
            commit_sha: "abc123".to_string(),
        };

        let result = build_review_comment(&pending, &files);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("File not found"));
    }
}
