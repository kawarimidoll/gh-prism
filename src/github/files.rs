use color_eyre::Result;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffFile {
    pub filename: String,
    pub status: String, // "added", "modified", "deleted", "renamed"
    pub additions: usize,
    pub deletions: usize,
    pub patch: Option<String>,
}

impl DiffFile {
    /// ステータスに応じた表示用文字を返す
    pub fn status_char(&self) -> char {
        match self.status.as_str() {
            "added" => 'A',
            "modified" => 'M',
            "removed" | "deleted" => 'D',
            "renamed" => 'R',
            _ => '?',
        }
    }
}

/// 特定のコミットの変更ファイル一覧を取得
pub async fn fetch_commit_files(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    sha: &str,
) -> Result<Vec<DiffFile>> {
    let url = format!("/repos/{}/{}/commits/{}", owner, repo, sha);

    // コミット詳細を取得（filesフィールドを含む）
    #[derive(Deserialize)]
    struct CommitResponse {
        files: Option<Vec<DiffFile>>,
    }

    let response: CommitResponse = client.get(url, None::<&()>).await?;
    Ok(response.files.unwrap_or_default())
}
