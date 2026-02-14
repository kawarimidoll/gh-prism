use color_eyre::Result;
use octocrab::Octocrab;
use serde::Deserialize;

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
