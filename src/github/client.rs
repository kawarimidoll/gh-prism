use color_eyre::{Result, eyre::eyre};
use octocrab::Octocrab;
use std::process::Command;

fn get_token() -> Result<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        return Ok(token);
    }

    let output = Command::new("gh").args(["auth", "token"]).output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to get GitHub token. Please set GITHUB_TOKEN or run `gh auth login`"
        ));
    }

    let token = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(token)
}

pub fn create_client() -> Result<Octocrab> {
    let token = get_token()?;
    let client = Octocrab::builder().personal_token(token).build()?;
    Ok(client)
}
