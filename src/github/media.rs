use image::DynamicImage;
use std::collections::HashMap;

/// ダウンロード済み画像のキャッシュ（URL → デコード済み画像）
pub struct MediaCache {
    images: HashMap<String, DynamicImage>,
}

impl MediaCache {
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
        }
    }

    pub fn insert(&mut self, url: String, image: DynamicImage) {
        self.images.insert(url, image);
    }

    pub fn get(&self, url: &str) -> Option<&DynamicImage> {
        self.images.get(url)
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

/// GitHub トークンを取得する（環境変数 or gh auth token）
fn get_token() -> Option<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        return Some(token);
    }
    std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

/// 複数の画像URLを並列ダウンロードしてMediaCacheを返す
/// ダウンロード失敗した画像は無視する（致命的エラーにしない）
pub async fn download_media(urls: Vec<String>) -> MediaCache {
    use futures::stream::{FuturesUnordered, StreamExt};

    let mut cache = MediaCache::new();
    if urls.is_empty() {
        return cache;
    }

    let token = get_token();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let futs: FuturesUnordered<_> = urls
        .into_iter()
        .map(|url| {
            let token = token.clone();
            let client = client.clone();
            async move {
                let result = download_single_image(&client, &url, token.as_deref()).await;
                (url, result)
            }
        })
        .collect();

    futures::pin_mut!(futs);
    while let Some((url, result)) = futs.next().await {
        if let Ok(img) = result {
            cache.insert(url, img);
        }
    }

    cache
}

/// 単一画像のダウンロードとデコード
async fn download_single_image(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> Result<DynamicImage, Box<dyn std::error::Error + Send + Sync>> {
    let mut request = client.get(url).header("User-Agent", "gh-prism");

    // private-user-images や user-attachments は認証が必要な場合がある
    if let Some(token) = token {
        if url.contains("private-user-images") || url.contains("user-attachments") {
            request = request.header("Authorization", format!("token {}", token));
        }
    }

    let response = request.send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    let img = image::load_from_memory(&bytes)?;
    Ok(img)
}
