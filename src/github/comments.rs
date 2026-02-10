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
