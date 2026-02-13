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

    // stdin への書き込みを別スレッドで行い、パイプデッドロックを回避する。
    // 大きな diff では stdin パイプバッファが満杯になり write_all がブロックするが、
    // wait_with_output が stdout を並行して読み取ることでデッドロックを防ぐ。
    let mut stdin = child.stdin.take().expect("stdin was configured");
    let diff_bytes = diff.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&diff_bytes);
        // stdin はここで drop → delta に EOF が送られる
    });

    let output = child.wait_with_output()?;
    let _ = writer.join();
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
            // delta がヘッダー周辺に空行を挿入する場合があるため、
            // 固定行数ではなく最初の @@ 行を検出して除去する
            let first_hunk = text.lines.iter().position(|line| {
                line.spans
                    .iter()
                    .find(|s| !s.content.is_empty())
                    .is_some_and(|s| s.content.starts_with("@@"))
            });
            if let Some(idx) = first_hunk {
                text.lines.drain(..idx);
            } else if text.lines.len() > header_line_count {
                // @@ 行が見つからない場合は従来の固定行数で除去
                text.lines.drain(..header_line_count);
            }

            // delta が挿入した余分な空行を除去して patch 行数と一致させる。
            // diff body の各行は必ずプレフィックス（+/-/スペース/@@）を持つため、
            // 全 Span の内容が空の Line は delta が挿入した余分な行と判定できる。
            let expected_count = diff.lines().count();
            while text.lines.len() > expected_count {
                let empty_pos = text.lines.iter().position(|line| {
                    line.spans.is_empty() || line.spans.iter().all(|s| s.content.is_empty())
                });
                if let Some(idx) = empty_pos {
                    text.lines.remove(idx);
                } else {
                    break;
                }
            }

            // whole-file diff では delta 用に追加した先頭スペースを除去。
            // non-delta パス（+/- を完全除去）と幅を一致させ、wrap の不整合を防ぐ。
            if is_whole_file {
                for line in &mut text.lines {
                    let is_hunk = line
                        .spans
                        .iter()
                        .find(|s| !s.content.is_empty())
                        .is_some_and(|s| s.content.starts_with("@@"));
                    if is_hunk {
                        continue;
                    }
                    for span in &mut line.spans {
                        if let Some(trimmed) = span.content.strip_prefix(' ') {
                            span.content = trimmed.to_string().into();
                            break;
                        }
                    }
                }
            }

            text
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 変更パッチの行数が入力と一致することを確認
    #[test]
    fn test_highlight_diff_line_count_matches_patch() {
        if !has_delta() {
            return;
        }

        let patch = "@@ -1,3 +1,3 @@\n context\n-old\n+new";
        let text = highlight_diff(patch, "test.rs", "modified")
            .expect("highlight_diff should return Some when delta is available");

        assert_eq!(
            text.lines.len(),
            patch.lines().count(),
            "行数がパッチと一致すること"
        );
    }

    /// whole-file diff で先頭スペースが除去されていることを確認
    #[test]
    fn test_highlight_diff_whole_file_no_leading_space() {
        if !has_delta() {
            return;
        }

        let patch = "@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3";
        let text = highlight_diff(patch, "test.rs", "added")
            .expect("highlight_diff should return Some when delta is available");

        assert_eq!(
            text.lines.len(),
            patch.lines().count(),
            "行数がパッチと一致すること"
        );

        // @@ 行以外で、最初の非空 span が空白で始まっていないことを確認
        for (i, line) in text.lines.iter().enumerate() {
            let first_nonempty = line.spans.iter().find(|s| !s.content.is_empty());
            if let Some(span) = first_nonempty {
                if span.content.starts_with("@@") {
                    continue;
                }
                assert!(
                    !span.content.starts_with(' '),
                    "行 {} の先頭スペースが除去されていること: {:?}",
                    i,
                    span.content
                );
            }
        }
    }

    /// 各行の幅がパッチ行の幅と一致することを確認
    #[test]
    fn test_highlight_diff_preserves_width() {
        if !has_delta() {
            return;
        }

        let patch = "@@ -1,5 +1,4 @@\n context\n-old\n+new\n-\n ";
        let text = highlight_diff(patch, "test.rs", "modified")
            .expect("highlight_diff should return Some when delta is available");

        use unicode_width::UnicodeWidthStr;

        for (i, (text_line, patch_line)) in text.lines.iter().zip(patch.lines()).enumerate() {
            let text_width: usize = text_line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let patch_width = UnicodeWidthStr::width(patch_line);

            assert_eq!(
                text_width, patch_width,
                "行 {} の幅が一致すること: text={}, patch={}",
                i, text_width, patch_width
            );
        }
    }
}
