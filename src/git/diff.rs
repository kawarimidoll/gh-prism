use color_eyre::Result;
use ratatui::text::Text;
use std::io::Write;
use std::process::{Command, Stdio};

/// delta コマンドが利用可能かチェック
pub fn has_delta() -> bool {
    Command::new("delta")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// delta を使って diff をシンタックスハイライト
/// 戻り値は ANSI エスケープシーケンスを含む文字列
pub fn highlight_with_delta(diff: &str) -> Result<String> {
    let mut child = Command::new("delta")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(diff.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// ANSI エスケープシーケンスを含む文字列を ratatui の Text に変換
pub fn ansi_to_text(ansi_str: &str) -> Result<Text<'static>> {
    use ansi_to_tui::IntoText;
    let text = ansi_str.into_text()?;
    Ok(text)
}

/// diff をハイライト付きで Text に変換
/// delta が利用可能なら使用、なければ None を返す
pub fn highlight_diff(diff: &str) -> Option<Text<'static>> {
    if !has_delta() {
        return None;
    }

    highlight_with_delta(diff)
        .ok()
        .and_then(|highlighted| ansi_to_text(&highlighted).ok())
}
