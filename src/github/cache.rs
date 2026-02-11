use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::commits::CommitInfo;
use super::files::DiffFile;

#[derive(Debug, Serialize, Deserialize)]
pub struct PrCache {
    pub head_sha: String,
    pub pr_title: String,
    pub pr_body: String,
    pub pr_author: String,
    pub commits: Vec<CommitInfo>,
    pub files_map: HashMap<String, Vec<DiffFile>>,
}

fn cache_dir(owner: &str, repo: &str) -> PathBuf {
    std::env::temp_dir().join("gh-prism").join(owner).join(repo)
}

fn cache_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir(owner, repo).join(format!("pr-{}.json", pr_number))
}

pub fn read_cache(owner: &str, repo: &str, pr_number: u64) -> Option<PrCache> {
    let path = cache_path(owner, repo, pr_number);
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn write_cache(owner: &str, repo: &str, pr_number: u64, cache: &PrCache) {
    let path = cache_path(owner, repo, pr_number);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("Warning: failed to create cache directory: {}", e);
        return;
    }
    match serde_json::to_string(cache) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("Warning: failed to write cache file: {}", e);
            }
        }
        Err(e) => {
            eprintln!("Warning: failed to serialize cache: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::commits::CommitDetail;

    #[test]
    fn test_cache_round_trip() {
        let owner = "test-owner";
        let repo = "test-repo";
        let pr_number = 99999;

        let cache = PrCache {
            head_sha: "abc1234".to_string(),
            pr_title: "Test PR".to_string(),
            pr_body: "Test body".to_string(),
            pr_author: "test-author".to_string(),
            commits: vec![CommitInfo {
                sha: "abc1234".to_string(),
                commit: CommitDetail {
                    message: "test commit".to_string(),
                },
            }],
            files_map: {
                let mut m = HashMap::new();
                m.insert(
                    "abc1234".to_string(),
                    vec![DiffFile {
                        filename: "test.rs".to_string(),
                        status: "modified".to_string(),
                        additions: 1,
                        deletions: 0,
                        patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
                    }],
                );
                m
            },
        };

        write_cache(owner, repo, pr_number, &cache);
        let loaded = read_cache(owner, repo, pr_number);
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.head_sha, "abc1234");
        assert_eq!(loaded.pr_title, "Test PR");
        assert_eq!(loaded.pr_author, "test-author");
        assert_eq!(loaded.commits.len(), 1);
        assert_eq!(loaded.files_map.len(), 1);

        // cleanup
        let _ = std::fs::remove_file(cache_path(owner, repo, pr_number));
    }

    #[test]
    fn test_read_cache_missing_file() {
        let result = read_cache("nonexistent", "repo", 0);
        assert!(result.is_none());
    }
}
