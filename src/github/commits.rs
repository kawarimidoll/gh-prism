use color_eyre::Result;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub commit: CommitDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub email: String,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitDetail {
    pub message: String,
    pub author: Option<CommitAuthor>,
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

    /// コミットの author を `name <email>` 形式で返す
    pub fn author_line(&self) -> String {
        match &self.commit.author {
            Some(a) => format!("{} <{}>", a.name, a.email),
            None => "unknown".to_string(),
        }
    }

    /// コミットの author date を返す
    pub fn author_date(&self) -> &str {
        self.commit
            .author
            .as_ref()
            .map(|a| a.date.as_str())
            .unwrap_or("")
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
