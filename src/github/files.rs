use color_eyre::Result;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

/// ファイルの変更種別
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    #[default]
    Modified,
    Removed,
    Deleted,
    Renamed,
    #[serde(other)]
    Unknown,
}

impl FileStatus {
    pub fn status_char(&self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Removed | FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Unknown => '?',
        }
    }

    pub fn is_whole_file(&self) -> bool {
        matches!(
            self,
            FileStatus::Added | FileStatus::Removed | FileStatus::Deleted
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffFile {
    pub filename: String,
    pub status: FileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_filename: Option<String>,
}

impl DiffFile {
    /// ステータスに応じた表示用文字を返す
    pub fn status_char(&self) -> char {
        self.status.status_char()
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
