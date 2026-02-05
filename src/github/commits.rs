use color_eyre::Result;
use octocrab::Octocrab;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub commit: CommitDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitDetail {
    pub message: String,
}

impl CommitInfo {
    /// 短いSHA（7文字）を返す
    pub fn short_sha(&self) -> &str {
        &self.sha[..7.min(self.sha.len())]
    }

    /// コミットメッセージの1行目を返す
    pub fn message_summary(&self) -> &str {
        self.commit.message.lines().next().unwrap_or("")
    }
}

pub async fn fetch_commits(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<CommitInfo>> {
    let url = format!("/repos/{}/{}/pulls/{}/commits", owner, repo, pr_number);
    let commits: Vec<CommitInfo> = client.get(url, None::<&()>).await?;
    Ok(commits)
}
