use super::*;

use unicode_width::UnicodeWidthStr;

/// ISO 8601 日時文字列をシステムタイムゾーンのローカル時刻に変換して返す
/// 入力例: "2024-01-15T09:30:00Z" → "2024-01-15 18:30 +0900"（JST の場合）
pub(super) fn format_datetime(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M %z")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

impl App {
    /// @@ hunk header を整形表示用の Line に変換
    /// `@@ -10,5 +12,7 @@ fn main()` → `─── L10-14 → L12-18 ─── fn main() ────`
    pub(super) fn format_hunk_header(raw: &str, width: u16, style: Style) -> Line<'static> {
        let width = width as usize;

        let (range_text, context) = if let Some(rest) = raw.strip_prefix("@@ ") {
            if let Some(at_pos) = rest.find(" @@") {
                let range_part = &rest[..at_pos];
                let ctx = rest[at_pos + 3..].trim();

                let mut parts = range_part.split_whitespace();
                let old = parts
                    .next()
                    .and_then(|p| p.strip_prefix('-'))
                    .unwrap_or("0");
                let new = parts
                    .next()
                    .and_then(|p| p.strip_prefix('+'))
                    .unwrap_or("0");

                let format_range = |r: &str| -> String {
                    let mut iter = r.split(',');
                    let start: usize = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let len: usize = iter.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                    if len <= 1 {
                        format!("L{start}")
                    } else {
                        format!("L{}-{}", start, start + len - 1)
                    }
                };

                (
                    format!("{} → {}", format_range(old), format_range(new)),
                    ctx.to_string(),
                )
            } else {
                (String::new(), String::new())
            }
        } else {
            (String::new(), String::new())
        };

        let mut content = String::from("─── ");
        if !range_text.is_empty() {
            content.push_str(&range_text);
            content.push(' ');
        }
        if !context.is_empty() {
            content.push_str("─── ");
            content.push_str(&context);
            content.push(' ');
        }

        // content が width を超える場合はトランケート（wrap で折り返されるのを防止）
        let content_width = UnicodeWidthStr::width(content.as_str());
        if content_width >= width {
            content = truncate_str(&content, width.saturating_sub(1));
            content.push('─');
        } else {
            let fill_count = width - content_width;
            for _ in 0..fill_count {
                content.push('─');
            }
        }

        Line::styled(content, style)
    }
}

/// リアクション行をスタイル付き Line として構築する
/// `user_reaction_ids` に含まれる（自分がリアクション済みの）絵文字は Yellow + Bold でハイライト
pub(super) fn build_reaction_line<'a>(
    reactions: &crate::github::comments::Reactions,
    user_reaction_ids: &std::collections::HashMap<String, u64>,
    indent: &str,
) -> Option<Line<'a>> {
    let pairs = reactions.non_zero();
    if pairs.is_empty() {
        return None;
    }

    let dim = Style::default().fg(Color::DarkGray);
    let highlight = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'a>> = vec![Span::raw(indent.to_string())];
    for (i, (emoji, api_val, count)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", dim));
        }
        let pad = " ".repeat(2_usize.saturating_sub(UnicodeWidthStr::width(*emoji)));
        let text = format!("{}{} {}", emoji, pad, count);
        let style = if user_reaction_ids.contains_key(*api_val) {
            highlight
        } else {
            dim
        };
        spans.push(Span::styled(text, style));
    }
    Some(Line::from(spans))
}

/// URL をシステムのデフォルトブラウザで開く
pub(super) fn open_url_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}

/// 文字列を最大表示幅に収まるように末尾を省略する（unicode-width 対応）
/// 例: "prism - repo#1: Long PR title" → "prism - repo#1: Lo…"
pub(super) fn truncate_str(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0;
    let mut result = String::new();
    let ellipsis_width = 1; // "…" is 1 column wide
    let target = max_width.saturating_sub(ellipsis_width);
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > target {
            break;
        }
        width += cw;
        result.push(ch);
    }
    result.push('…');
    result
}

/// リネームされたファイルの "old → new" 表示を最大幅に収める。
/// 幅が足りない場合は各パスの先頭を省略する。
/// 例: "src/types/profile_types.rs → src/domain/profile.rs"
///   → ".../profile_types.rs → .../profile.rs"
pub(super) fn truncate_rename(old: &str, new: &str, max_width: usize) -> String {
    let arrow = " → ";
    let arrow_len = arrow.len(); // 5 bytes but display width matters; ASCII arrow is 5 columns
    let full = format!("{old}{arrow}{new}");
    if full.len() <= max_width {
        return full;
    }
    // arrow 分を引いて残りを半分ずつ割り当て
    let path_budget = max_width.saturating_sub(arrow_len);
    let half = path_budget / 2;
    let old_budget = half;
    let new_budget = path_budget - old_budget; // 奇数なら new に1多く割り当て
    format!(
        "{}{}{}",
        truncate_path(old, old_budget),
        arrow,
        truncate_path(new, new_budget),
    )
}

/// パスを最大幅に収まるように先頭を省略する（ASCII パスを前提）
/// 例: "src/components/MyComponent/index.tsx" → ".../MyComponent/index.tsx"
pub(super) fn truncate_path(path: &str, max_width: usize) -> String {
    if path.len() <= max_width {
        return path.to_string();
    }
    if max_width < 4 {
        // "..." すら収まらない幅ではそのまま切り詰める
        return path[..max_width].to_string();
    }
    // "..." prefix = 3 chars
    let available = max_width - 3;
    // パスの後ろから available 文字分を取り、最初の '/' 以降を使う
    let tail = &path[path.len() - available..];
    if let Some(pos) = tail.find('/') {
        format!("...{}", &tail[pos..])
    } else {
        format!("...{}", tail)
    }
}
