use std::time::{Duration, Instant};

/// ターミナルのカラーテーマ
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Panel {
    PrDescription,
    CommitList,
    FileTree,
    DiffView,
}

/// アプリケーションのモード
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AppMode {
    #[default]
    Normal,
    LineSelect,
    CommentInput,
    CommentView,
    ReviewSubmit,
    ReviewBodyInput,
    QuitConfirm,
    Help,
    MediaViewer,
}

/// レビューイベントタイプ
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

impl ReviewEvent {
    pub const ALL: [ReviewEvent; 3] = [
        ReviewEvent::Comment,
        ReviewEvent::Approve,
        ReviewEvent::RequestChanges,
    ];

    pub fn as_api_str(&self) -> &str {
        match self {
            ReviewEvent::Comment => "COMMENT",
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        }
    }

    pub fn label(&self) -> &str {
        match self {
            ReviewEvent::Comment => "Comment",
            ReviewEvent::Approve => "Approve",
            ReviewEvent::RequestChanges => "Request Changes",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusLevel {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct StatusMessage {
    pub body: String,
    pub level: StatusLevel,
    pub created_at: Instant,
}

impl StatusMessage {
    pub fn info(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            level: StatusLevel::Info,
            created_at: Instant::now(),
        }
    }

    pub fn error(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            level: StatusLevel::Error,
            created_at: Instant::now(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= Duration::from_secs(3)
    }
}

/// 行選択の状態（アンカー位置を保持）
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineSelection {
    /// 選択開始位置（v を押した時のカーソル位置）
    pub anchor: usize,
}

impl LineSelection {
    /// 選択範囲を取得（常に start <= end）
    pub fn range(&self, cursor: usize) -> (usize, usize) {
        if self.anchor <= cursor {
            (self.anchor, cursor)
        } else {
            (cursor, self.anchor)
        }
    }

    /// 選択行数を取得
    pub fn count(&self, cursor: usize) -> usize {
        let (start, end) = self.range(cursor);
        end - start + 1
    }
}

/// メディア種別
#[derive(Debug, Clone, PartialEq)]
pub enum MediaType {
    Image,
    Video,
}

/// PR body 中のメディア参照
#[derive(Debug, Clone)]
pub struct MediaRef {
    pub media_type: MediaType,
    pub url: String,
    pub alt: String,
}
