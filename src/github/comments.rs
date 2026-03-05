use color_eyre::Result;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

const REVIEW_THREADS_PAGE_SIZE: u32 = 100;

/// GitHub API が返す reactions オブジェクト
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reactions {
    #[serde(rename = "+1")]
    pub plus_one: u32,
    #[serde(rename = "-1")]
    pub minus_one: u32,
    pub laugh: u32,
    pub hooray: u32,
    pub confused: u32,
    pub heart: u32,
    pub rocket: u32,
    pub eyes: u32,
}

impl Reactions {
    /// カウント > 0 の reaction を (emoji, api_value, count) のリストで返す
    pub fn non_zero(&self) -> Vec<(&str, &str, u32)> {
        if self.is_empty() {
            return Vec::new();
        }
        let all = [
            ("👍", "+1", self.plus_one),
            ("👎", "-1", self.minus_one),
            ("😄", "laugh", self.laugh),
            ("🎉", "hooray", self.hooray),
            ("😕", "confused", self.confused),
            ("🩷", "heart", self.heart),
            ("🚀", "rocket", self.rocket),
            ("👀", "eyes", self.eyes),
        ];
        all.into_iter().filter(|(_, _, c)| *c > 0).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.plus_one == 0
            && self.minus_one == 0
            && self.laugh == 0
            && self.hooray == 0
            && self.confused == 0
            && self.heart == 0
            && self.rocket == 0
            && self.eyes == 0
    }

    /// リアクション行を表示用文字列として返す（カウント0のものは除外）
    pub fn display_line(&self, indent: &str) -> Option<String> {
        let pairs = self.non_zero();
        if pairs.is_empty() {
            return None;
        }
        let reaction_str: String = pairs
            .iter()
            .map(|(emoji, _, count)| {
                let pad = " ".repeat(2_usize.saturating_sub(emoji.width()));
                format!("{}{} {}", emoji, pad, count)
            })
            .collect::<Vec<_>>()
            .join("  ");
        Some(format!("{}{}", indent, reaction_str))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewThread {
    pub node_id: String,
    pub is_resolved: bool,
    pub root_comment_database_id: u64,
}

/// ReviewComment のスレッドのルートコメント ID を返す。
/// `comments` の最初の要素がルートコメント（`in_reply_to_id` が `None`）または
/// リプライ（`in_reply_to_id` がルート ID を指す）であることを前提とする。
pub fn root_comment_id(comments: &[ReviewComment]) -> Option<u64> {
    comments.first().map(|c| c.in_reply_to_id.unwrap_or(c.id))
}

/// GraphQL API で PR のレビュースレッド一覧を取得する（`gh api graphql` 経由）。
/// 最大 100 スレッドまで取得。超過分はページネーション未実装のため取得されない。
pub fn fetch_review_threads(owner: &str, repo: &str, pr_number: u64) -> Result<Vec<ReviewThread>> {
    let query = format!(
        r#"query($owner: String!, $repo: String!, $pr: Int!) {{
  repository(owner: $owner, name: $repo) {{
    pullRequest(number: $pr) {{
      reviewThreads(first: {}) {{
        nodes {{
          id
          isResolved
          comments(first: 1) {{
            nodes {{
              databaseId
            }}
          }}
        }}
      }}
    }}
  }}
}}"#,
        REVIEW_THREADS_PAGE_SIZE
    );

    let output = std::process::Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("repo={repo}"),
            "-F",
            &format!("pr={pr_number}"),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "GraphQL query failed: {}",
            stderr.trim()
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let nodes = json["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut threads = Vec::new();
    for node in nodes {
        let node_id = node["id"].as_str().unwrap_or_default().to_string();
        let is_resolved = node["isResolved"].as_bool().unwrap_or(false);
        let db_id = node["comments"]["nodes"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|c| c["databaseId"].as_u64())
            .unwrap_or(0);
        if db_id > 0 && !node_id.is_empty() {
            threads.push(ReviewThread {
                node_id,
                is_resolved,
                root_comment_database_id: db_id,
            });
        }
    }

    Ok(threads)
}

/// GraphQL mutation でレビュースレッドの resolve 状態を変更する共通ヘルパー。
/// 戻り値は実際の isResolved 値。
fn toggle_review_thread(thread_node_id: &str, resolve: bool) -> Result<bool> {
    let (mutation_name, response_key) = if resolve {
        ("resolveReviewThread", "resolveReviewThread")
    } else {
        ("unresolveReviewThread", "unresolveReviewThread")
    };

    let query = format!(
        r#"mutation($threadId: ID!) {{
  {mutation_name}(input: {{threadId: $threadId}}) {{
    thread {{
      isResolved
    }}
  }}
}}"#
    );

    let output = std::process::Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("threadId={thread_node_id}"),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "{mutation_name} failed: {}",
            stderr.trim()
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    json["data"][response_key]["thread"]["isResolved"]
        .as_bool()
        .ok_or_else(|| color_eyre::eyre::eyre!("Unexpected response format"))
}

/// GraphQL mutation でレビュースレッドを resolve する。
/// 戻り値は実際の isResolved 値。
pub fn resolve_review_thread(thread_node_id: &str) -> Result<bool> {
    toggle_review_thread(thread_node_id, true)
}

/// GraphQL mutation でレビュースレッドを unresolve する。
/// 戻り値は実際の isResolved 値。
pub fn unresolve_review_thread(thread_node_id: &str) -> Result<bool> {
    toggle_review_thread(thread_node_id, false)
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewCommentUser {
    pub login: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub body: String,
    pub path: String,
    pub line: Option<usize>,
    pub start_line: Option<usize>,
    pub side: Option<String>,
    pub start_side: Option<String>,
    pub commit_id: String,
    pub user: ReviewCommentUser,
    pub created_at: String,
    pub in_reply_to_id: Option<u64>,
    pub pull_request_review_id: Option<u64>,
    pub reactions: Option<Reactions>,
}

pub async fn fetch_review_comments(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<ReviewComment>> {
    let url = format!("/repos/{}/{}/pulls/{}/comments", owner, repo, pr_number);
    let comments: Vec<ReviewComment> = client.get(url, None::<&()>).await?;
    Ok(comments)
}

/// PR（Issue）への一般コメント（Conversation タブに表示されるもの）
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: Option<String>,
    pub user: ReviewCommentUser,
    pub created_at: String,
    pub reactions: Option<Reactions>,
}

/// Pull Request Review Comments API で既存コメントスレッドに返信を投稿
pub async fn post_reply_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    body: &str,
    in_reply_to: u64,
) -> Result<ReviewComment> {
    let url = format!("/repos/{}/{}/pulls/{}/comments", owner, repo, pr_number);
    let comment: ReviewComment = client
        .post(
            url,
            Some(&serde_json::json!({
                "body": body,
                "in_reply_to": in_reply_to,
            })),
        )
        .await?;
    Ok(comment)
}

/// Issue Comments API で PR に一般コメントを投稿
pub async fn post_issue_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    body: &str,
) -> Result<IssueComment> {
    let url = format!("/repos/{}/{}/issues/{}/comments", owner, repo, pr_number);
    let comment: IssueComment = client
        .post(url, Some(&serde_json::json!({ "body": body })))
        .await?;
    Ok(comment)
}

/// Issue comment にリアクションを追加
pub async fn post_reaction_to_issue_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    content: &str,
) -> Result<()> {
    let url = format!(
        "/repos/{}/{}/issues/comments/{}/reactions",
        owner, repo, comment_id
    );
    let _: serde_json::Value = client
        .post(url, Some(&serde_json::json!({ "content": content })))
        .await?;
    Ok(())
}

/// Review comment にリアクションを追加
pub async fn post_reaction_to_review_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    content: &str,
) -> Result<()> {
    let url = format!(
        "/repos/{}/{}/pulls/comments/{}/reactions",
        owner, repo, comment_id
    );
    let _: serde_json::Value = client
        .post(url, Some(&serde_json::json!({ "content": content })))
        .await?;
    Ok(())
}

/// GraphQL addReaction mutation で Review にリアクションを追加（REST API 非対応のため）
pub fn post_reaction_to_review(node_id: &str, content: &str) -> Result<()> {
    // GraphQL の ReactionContent enum 値に変換（SCREAMING_SNAKE_CASE）
    let gql_content = match content {
        "+1" => "THUMBS_UP",
        "-1" => "THUMBS_DOWN",
        "laugh" => "LAUGH",
        "hooray" => "HOORAY",
        "confused" => "CONFUSED",
        "heart" => "HEART",
        "rocket" => "ROCKET",
        "eyes" => "EYES",
        other => {
            return Err(color_eyre::eyre::eyre!("Unknown reaction content: {other}"));
        }
    };

    let query = format!(
        r#"mutation($id: ID!) {{ addReaction(input: {{subjectId: $id, content: {gql_content}}}) {{ reaction {{ content }} }} }}"#
    );

    let output = std::process::Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("id={node_id}"),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "addReaction failed: {}",
            stderr.trim()
        ));
    }

    Ok(())
}

/// Reactions List API が返す個別リアクションオブジェクト
#[derive(Debug, Clone, Deserialize)]
pub struct ReactionItem {
    pub id: u64,
    pub content: String,
    pub user: ReviewCommentUser,
}

/// Issue comment のリアクション一覧を取得し、指定ユーザーのものだけ (content → reaction_id) で返す
pub async fn fetch_user_reactions_for_issue_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    username: &str,
) -> Result<std::collections::HashMap<String, u64>> {
    let url = format!(
        "/repos/{}/{}/issues/comments/{}/reactions",
        owner, repo, comment_id
    );
    let items: Vec<ReactionItem> = client.get(url, None::<&()>).await?;
    Ok(items
        .into_iter()
        .filter(|r| r.user.login == username)
        .map(|r| (r.content, r.id))
        .collect())
}

/// Review comment のリアクション一覧を取得し、指定ユーザーのものだけ (content → reaction_id) で返す
pub async fn fetch_user_reactions_for_review_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    username: &str,
) -> Result<std::collections::HashMap<String, u64>> {
    let url = format!(
        "/repos/{}/{}/pulls/comments/{}/reactions",
        owner, repo, comment_id
    );
    let items: Vec<ReactionItem> = client.get(url, None::<&()>).await?;
    Ok(items
        .into_iter()
        .filter(|r| r.user.login == username)
        .map(|r| (r.content, r.id))
        .collect())
}

/// DELETE API でリアクションを削除（Issue comment）
pub async fn delete_reaction_from_issue_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    reaction_id: u64,
) -> Result<()> {
    let url = format!(
        "/repos/{}/{}/issues/comments/{}/reactions/{}",
        owner, repo, comment_id, reaction_id
    );
    let resp = client._delete(url, None::<&()>).await?;
    if !resp.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "Delete reaction failed: {}",
            resp.status()
        ));
    }
    Ok(())
}

/// DELETE API でリアクションを削除（Review comment）
pub async fn delete_reaction_from_review_comment(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    comment_id: u64,
    reaction_id: u64,
) -> Result<()> {
    let url = format!(
        "/repos/{}/{}/pulls/comments/{}/reactions/{}",
        owner, repo, comment_id, reaction_id
    );
    let resp = client._delete(url, None::<&()>).await?;
    if !resp.status().is_success() {
        return Err(color_eyre::eyre::eyre!(
            "Delete reaction failed: {}",
            resp.status()
        ));
    }
    Ok(())
}

/// GraphQL removeReaction mutation で Review からリアクションを削除
pub fn remove_reaction_from_review(node_id: &str, content: &str) -> Result<()> {
    let gql_content = match content {
        "+1" => "THUMBS_UP",
        "-1" => "THUMBS_DOWN",
        "laugh" => "LAUGH",
        "hooray" => "HOORAY",
        "confused" => "CONFUSED",
        "heart" => "HEART",
        "rocket" => "ROCKET",
        "eyes" => "EYES",
        other => {
            return Err(color_eyre::eyre::eyre!("Unknown reaction content: {other}"));
        }
    };

    let query = format!(
        r#"mutation($id: ID!) {{ removeReaction(input: {{subjectId: $id, content: {gql_content}}}) {{ reaction {{ content }} }} }}"#
    );

    let output = std::process::Command::new("gh")
        .args([
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("id={node_id}"),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "removeReaction failed: {}",
            stderr.trim()
        ));
    }

    Ok(())
}

/// Issue Comments API で PR の一般コメントを取得
pub async fn fetch_issue_comments(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<IssueComment>> {
    let url = format!("/repos/{}/{}/issues/{}/comments", owner, repo, pr_number);
    let comments: Vec<IssueComment> = client.get(url, None::<&()>).await?;
    Ok(comments)
}
