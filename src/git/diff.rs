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
///
/// delta を使って diff をシンタックスハイライト
/// --no-gitconfig でユーザー設定を無視し、--color-only で装飾を抑制する。
/// hunk ヘッダーのスタイリングは app.rs 側で独自に行うため、delta には raw 出力させる。
/// 注: app.rs 側で delta 出力をキャッシュするため、ファイル選択変更時のみ呼ばれる。
pub fn highlight_with_delta(diff: &str) -> Result<String> {
    let mut child = Command::new("delta")
        .args([
            "--no-gitconfig",
            "--paging=never",
            "--color-only",
            "--hunk-header-style=raw",
        ])
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

/// diff ヘッダーを生成（delta が言語検出に使用）
fn create_diff_header(filename: &str) -> String {
    format!("diff --git a/{filename} b/{filename}\n--- a/{filename}\n+++ b/{filename}\n")
}

/// diff をハイライト付きで Text に変換
/// delta が利用可能なら使用、なければ None を返す
/// filename を渡すことで delta が言語を検出できる
/// file_status が "added"/"removed"/"deleted" の場合、差分色を抑制してシンタックスハイライトのみ適用
/// 出力はパッチ行のみ（言語検出用に追加した diff ヘッダーは除去済み）
pub fn highlight_diff(diff: &str, filename: &str, file_status: &str) -> Option<Text<'static>> {
    if !has_delta() {
        return None;
    }

    let is_whole_file = matches!(file_status, "added" | "removed" | "deleted");

    // diff ヘッダーを追加してシンタックスハイライトを有効化
    let header = create_diff_header(filename);
    let header_line_count = header.lines().count();

    let body = if is_whole_file {
        // +/- を空白（context 行）に変換して diff 色を回避しつつシンタックスハイライトを維持。
        // @@ 行はそのまま保持して行数を一致させる。
        diff.lines()
            .map(|l| {
                if l.starts_with('+') || l.starts_with('-') {
                    format!(" {}", &l[1..])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        diff.to_string()
    };

    let full_diff = format!("{}{}", header, body);

    highlight_with_delta(&full_diff)
        .ok()
        .and_then(|highlighted| ansi_to_text(&highlighted).ok())
        .map(|mut text| {
            // 言語検出用に追加した diff ヘッダー行を除去
            if text.lines.len() > header_line_count {
                text.lines.drain(..header_line_count);
            }
            text
        })
}
