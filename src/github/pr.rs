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
