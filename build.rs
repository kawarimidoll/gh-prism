fn main() {
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let hash = hash.trim();

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap();
    if hash.is_empty() {
        println!("cargo:rustc-env=DEV_VERSION={pkg_version}");
    } else {
        println!("cargo:rustc-env=DEV_VERSION={pkg_version}-{hash}");
    }

    // Rerun when the current commit changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD")
        && let Some(ref_path) = head.strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=.git/{}", ref_path.trim());
    }
}
