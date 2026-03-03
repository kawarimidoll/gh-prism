use color_eyre::Result;
use octocrab::Octocrab;
use octocrab::models::pulls::PullRequest;

pub async fn fetch_pr(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<PullRequest> {
    let pr = client.pulls(owner, repo).get(pr_number).await?;
    Ok(pr)
}

/// PR をマージする（PUT /repos/{owner}/{repo}/pulls/{number}/merge）
pub async fn merge_pr(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    merge_method: &str,
) -> Result<String> {
    let route = format!("/repos/{owner}/{repo}/pulls/{pr_number}/merge");
    let body = serde_json::json!({ "merge_method": merge_method });
    let response: serde_json::Value = client.put(route, Some(&body)).await?;
    let message = response["message"].as_str().unwrap_or("Merged").to_string();
    Ok(message)
}

/// PR の state を変更する（PATCH /repos/{owner}/{repo}/pulls/{number}）
pub async fn update_pr_state(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &str,
) -> Result<()> {
    let route = format!("/repos/{owner}/{repo}/pulls/{pr_number}");
    let body = serde_json::json!({ "state": state });
    let _response: serde_json::Value = client.patch(route, Some(&body)).await?;
    Ok(())
}
