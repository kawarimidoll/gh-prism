use super::*;

/// PR body から画像 URL のみを軽量に収集する。
/// `preprocess_pr_body` と異なり、テキスト置換は行わない。
/// 対象パターン: `![alt](url)` および `<img src="url" ...>`
pub fn collect_image_urls(body: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for line in body.lines() {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            // Markdown image: ![alt](url)
            if bytes[pos] == b'!'
                && pos + 1 < bytes.len()
                && bytes[pos + 1] == b'['
                && let Some((_alt, url, end)) = parse_markdown_image(line, pos)
            {
                urls.push(url);
                pos = end;
                continue;
            }
            // HTML <img> tag
            if bytes[pos] == b'<' {
                let rest = &line[pos..];
                let lower_rest = rest.to_lowercase();
                if (lower_rest.starts_with("<img ") || lower_rest.starts_with("<img>"))
                    && let Some((_alt, url, end_offset)) = parse_html_img(rest)
                {
                    urls.push(url);
                    pos += end_offset;
                    continue;
                }
            }
            pos += 1;
        }
    }
    urls
}

/// PR body 中のメディア参照を検出し、プレースホルダーに置換する。
/// 戻り値: (置換済みテキスト, 検出されたメディア一覧)
pub fn preprocess_pr_body(body: &str) -> (String, Vec<MediaRef>) {
    let mut refs: Vec<MediaRef> = Vec::new();
    let mut result_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();

        // --- Pattern 4: HTML <video> tag ---
        if let Some(processed) = try_parse_html_video(trimmed) {
            result_lines.push(String::new());
            result_lines.push("[🎬 Video]".to_string());
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Video,
                url: processed,
                alt: "Video".to_string(),
            });
            continue;
        }

        // --- Pattern 3: Bare video URL on its own line ---
        if let Some(url) = try_parse_bare_video_url(trimmed) {
            result_lines.push(String::new());
            result_lines.push("[🎬 Video]".to_string());
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Video,
                url,
                alt: "Video".to_string(),
            });
            continue;
        }

        // --- Pattern 2: HTML <img> tag ---
        // --- Pattern 1: Markdown image ![alt](url) ---
        // These can appear inline, so we process within the line
        let processed = process_inline_media(line, &mut refs, &mut result_lines);
        if !processed {
            result_lines.push(line.to_string());
        }
    }

    // 前後の空行の重複を除去する
    let output = collapse_blank_lines(&result_lines);
    (output, refs)
}

/// 連続する空行を最大1つに縮小する
fn collapse_blank_lines(lines: &[String]) -> String {
    let mut result = String::new();
    let mut prev_blank = false;
    for (i, line) in lines.iter().enumerate() {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        if i > 0 {
            result.push('\n');
        }
        result.push_str(line);
        prev_blank = is_blank;
    }
    result
}

/// HTML <video> タグを検出し、src URL を返す
fn try_parse_html_video(line: &str) -> Option<String> {
    // <video で始まるかチェック
    let lower = line.to_lowercase();
    if !lower.contains("<video") {
        return None;
    }
    // src="..." を抽出
    extract_html_attr(line, "src")
}

/// 行が動画ベア URL かどうかチェック。
/// GitHub user-attachments URL は拡張子なし（UUID のみ）の場合がある。
/// Markdown 画像 `![](url)` でラップされていないベア URL は動画と推定する。
fn try_parse_bare_video_url(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let is_asset_url = trimmed.starts_with("https://github.com/user-attachments/assets/")
        || trimmed.starts_with("https://private-user-images.githubusercontent.com/");
    if !is_asset_url {
        return None;
    }
    // 明示的な動画拡張子があれば動画確定
    let url_path = trimmed.split('?').next().unwrap_or(trimmed);
    if url_path.ends_with(".mp4") || url_path.ends_with(".mov") || url_path.ends_with(".webm") {
        return Some(trimmed.to_string());
    }
    // 拡張子なしのアセット URL がベア URL として出現する場合、
    // 動画の可能性が高い（画像は通常 ![alt](url) でラップされるため）
    Some(trimmed.to_string())
}

/// 行内の Markdown 画像と HTML img タグを処理する。
/// 置換が発生した場合は true を返し、result_lines に追加済み。
pub(super) fn process_inline_media(
    line: &str,
    refs: &mut Vec<MediaRef>,
    result_lines: &mut Vec<String>,
) -> bool {
    let mut replaced = String::new();
    let mut had_match = false;
    let mut pos = 0;
    let bytes = line.as_bytes();

    while pos < bytes.len() {
        // Try Markdown image: ![alt](url)
        if bytes[pos] == b'!'
            && pos + 1 < bytes.len()
            && bytes[pos + 1] == b'['
            && let Some((alt, url, end)) = parse_markdown_image(line, pos)
        {
            had_match = true;
            let display_alt = if alt.is_empty() {
                "Image".to_string()
            } else {
                alt.clone()
            };
            // 前のテキストがあれば先に追加
            if !replaced.is_empty() {
                result_lines.push(replaced.clone());
                replaced.clear();
            }
            result_lines.push(String::new());
            result_lines.push(format!("[🖼 {}]", display_alt));
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Image,
                url,
                alt: display_alt,
            });
            pos = end;
            continue;
        }

        // Try HTML <img> tag
        if bytes[pos] == b'<' {
            let rest = &line[pos..];
            let lower_rest = rest.to_lowercase();
            if (lower_rest.starts_with("<img ") || lower_rest.starts_with("<img>"))
                && let Some((alt, url, end_offset)) = parse_html_img(rest)
            {
                had_match = true;
                let display_alt = if alt.is_empty() {
                    "Image".to_string()
                } else {
                    alt
                };
                if !replaced.is_empty() {
                    result_lines.push(replaced.clone());
                    replaced.clear();
                }
                result_lines.push(String::new());
                result_lines.push(format!("[🖼 {}]", display_alt));
                result_lines.push(String::new());
                refs.push(MediaRef {
                    media_type: MediaType::Image,
                    url,
                    alt: display_alt,
                });
                pos += end_offset;
                continue;
            }
        }

        // マルチバイト文字に対応するため、文字単位で処理する
        let ch = line[pos..].chars().next().unwrap();
        replaced.push(ch);
        pos += ch.len_utf8();
    }

    if had_match {
        // 残りのテキストがあれば追加
        let trimmed = replaced.trim();
        if !trimmed.is_empty() {
            result_lines.push(replaced);
        }
        true
    } else {
        false
    }
}

/// Markdown 画像 `![alt](url)` をパース。成功時は (alt, url, end_pos) を返す。
fn parse_markdown_image(line: &str, start: usize) -> Option<(String, String, usize)> {
    // start は '!' の位置、start+1 は '['
    let after_bang = start + 2; // '[' の次
    let alt_end = line[after_bang..].find(']')?;
    let alt = &line[after_bang..after_bang + alt_end];

    let paren_start = after_bang + alt_end + 1; // ']' の次
    if paren_start >= line.len() || line.as_bytes()[paren_start] != b'(' {
        return None;
    }
    let url_start = paren_start + 1;
    let paren_end = line[url_start..].find(')')?;
    let url = &line[url_start..url_start + paren_end];

    Some((alt.to_string(), url.to_string(), url_start + paren_end + 1))
}

/// HTML <img ...> タグをパース。成功時は (alt, src_url, end_offset) を返す。
/// end_offset は入力文字列の先頭からの相対位置。
fn parse_html_img(tag_str: &str) -> Option<(String, String, usize)> {
    // タグの終端を探す: "/>" or ">"
    let end_pos = find_tag_end(tag_str)?;
    let tag_content = &tag_str[..end_pos];

    let src = extract_html_attr(tag_content, "src")?;
    let alt = extract_html_attr(tag_content, "alt").unwrap_or_default();

    Some((alt, src, end_pos))
}

/// HTML タグ文字列の終端位置を探す（`/>` or `>` の直後）
fn find_tag_end(s: &str) -> Option<usize> {
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            return Some(i + 2);
        }
        if bytes[i] == b'>' {
            // </video> のような閉じタグも考慮
            // タグ全体の終わりを返す
            // <video ...>...</video> パターンの場合
            let rest = &s[i + 1..];
            let lower_rest = rest.to_lowercase();
            if let Some(close_pos) = lower_rest.find("</video>") {
                return Some(i + 1 + close_pos + 8); // 8 = "</video>".len()
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// HTML 属性値を抽出（例: `src="value"` → `value`）
fn extract_html_attr(tag: &str, attr_name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let search = format!("{}=\"", attr_name);
    let idx = lower.find(&search)?;
    let value_start = idx + search.len();
    let rest = &tag[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

impl App {
    /// メディアビューアモードに入る（メディアがある場合のみ）
    pub(super) fn enter_media_viewer(&mut self) {
        self.ensure_pr_desc_rendered();
        if self.media_refs.is_empty() {
            self.status_message =
                Some(StatusMessage::info("No images or videos in PR description"));
            return;
        }
        self.media_viewer_index = 0;
        self.prepare_media_protocol();
        self.mode = AppMode::MediaViewer;
    }

    /// 完了したバックグラウンドワーカーの結果をキャッシュに回収する。
    pub(super) fn poll_media_protocol_worker(&mut self) {
        if self
            .media_protocol_worker
            .as_ref()
            .is_some_and(|h| h.is_finished())
            && let Some(handle) = self.media_protocol_worker.take()
            && let Ok((url, protocol)) = handle.join()
        {
            self.media_protocol_cache.insert(url, protocol);
        }
    }

    /// 現在の media_viewer_index に対応するメディアのレンダリングプロトコルを準備する。
    /// 既にキャッシュ済みの画像はスキップし、未キャッシュの画像はバックグラウンドで生成する。
    /// 動画の場合はプロトコルを作成しない（サムネイル未対応）。
    /// 別画像のワーカーが実行中でも、現在の画像のためのワーカーを新たに起動する
    /// （古いワーカーは完了時にキャッシュへ回収される）。
    pub(super) fn prepare_media_protocol(&mut self) {
        let info = self
            .media_ref_at(self.media_viewer_index)
            .map(|r| (r.media_type.clone(), r.url.clone()));
        if let Some((media_type, url)) = info {
            if media_type == MediaType::Video || self.media_protocol_cache.contains_key(&url) {
                return;
            }
            if let Some(picker) = self.picker.clone()
                && let Some(img) = self.media_cache.get(&url).cloned()
            {
                // 代入により前のワーカーの JoinHandle が drop → detach される
                self.media_protocol_worker = Some(std::thread::spawn(move || {
                    let protocol = picker.new_resize_protocol(img);
                    (url, protocol)
                }));
            }
        }
    }
}
