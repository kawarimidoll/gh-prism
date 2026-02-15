use super::ThemeMode;
use crate::git::diff::ansi_to_text;
use ratatui::text::{Line, Span};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

static BAT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// bat の可用性を起動時に1回だけチェック（OnceLock でキャッシュ）
fn has_bat() -> bool {
    *BAT_AVAILABLE.get_or_init(|| {
        Command::new("bat")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// テーマに応じた bat のカラースキームを返す
fn bat_theme(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::Dark => "base16",
        ThemeMode::Light => "GitHub",
    }
}

/// bat でマークダウンをシンタックスハイライト
/// パイプデッドロック回避は highlight_with_delta と同じ thread::spawn パターン
fn highlight_with_bat(text: &str, theme: ThemeMode) -> Option<Vec<Line<'static>>> {
    if !has_bat() {
        return None;
    }

    let mut child = Command::new("bat")
        .args([
            "--language=markdown",
            "--color=always",
            "--style=plain",
            "--paging=never",
            &format!("--theme={}", bat_theme(theme)),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take().expect("stdin was configured");
    let text_bytes = text.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        // bat クラッシュ時のみ broken pipe が発生するが、output.status で検知するため無視して良い
        let _ = stdin.write_all(&text_bytes);
    });

    let output = child.wait_with_output().ok()?;
    let _ = writer.join();

    if !output.status.success() {
        return None;
    }

    let ansi_str = String::from_utf8_lossy(&output.stdout);
    let ratatui_text = ansi_to_text(&ansi_str).ok()?;
    Some(
        ratatui_text
            .lines
            .into_iter()
            .map(|line| {
                let mut new_line = Line::from(
                    line.spans
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style))
                        .collect::<Vec<_>>(),
                );
                new_line.style = line.style;
                new_line.alignment = line.alignment;
                new_line
            })
            .collect(),
    )
}

/// マークダウンテキストを ratatui Line に変換する。
/// bat が利用可能なら bat でシンタックスハイライト、なければ生テキストをそのまま表示。
pub(super) fn render_markdown(text: &str, theme: ThemeMode) -> Vec<Line<'static>> {
    if let Some(lines) = highlight_with_bat(text, theme) {
        return lines;
    }
    // bat が利用不可の場合は生テキストをそのまま表示
    text.lines().map(|l| Line::raw(l.to_string())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown_returns_lines() {
        let text = "# Title\n\nSome text\n\n| a | b |\n| - | - |\n| 1 | 2 |";
        let lines = render_markdown(text, ThemeMode::Dark);
        assert!(!lines.is_empty());
        // bat の有無にかかわらず入力行数と出力行数が一致する
        // (bat はハイライトのみで行数を変えない)
        assert_eq!(lines.len(), text.lines().count());
    }

    #[test]
    fn test_render_markdown_preserves_content() {
        let text = "Hello world\n\nSecond line";
        let lines = render_markdown(text, ThemeMode::Dark);
        let text_content: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text_content.contains("Hello world"));
        assert!(text_content.contains("Second line"));
    }
}
