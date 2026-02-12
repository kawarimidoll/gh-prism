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

/// レビュー・コメント関連の状態
#[derive(Debug, Default)]
pub struct ReviewState {
    pub comment_input: String,
    pub pending_comments: Vec<crate::github::review::PendingComment>,
    pub review_comments: Vec<crate::github::comments::ReviewComment>,
    pub viewing_comments: Vec<crate::github::comments::ReviewComment>,
    pub viewing_comment_scroll: u16,
    pub comment_view_max_scroll: u16,
    pub review_event_cursor: usize,
    pub review_body_input: String,
    pub needs_submit: Option<ReviewEvent>,
    pub quit_after_submit: bool,
}

/// DiffView パネルの表示状態
#[derive(Debug)]
pub struct DiffViewState {
    pub scroll: u16,
    pub cursor_line: usize,
    pub view_height: u16,
    pub view_width: u16,
    pub wrap: bool,
    pub show_line_numbers: bool,
    pub visual_offsets: Option<Vec<usize>>,
    pub highlight_cache: Option<(usize, usize, ratatui::text::Text<'static>)>,
}

impl Default for DiffViewState {
    fn default() -> Self {
        Self {
            scroll: 0,
            cursor_line: 0,
            view_height: 20,
            view_width: 80,
            wrap: false,
            show_line_numbers: false,
            visual_offsets: None,
            highlight_cache: None,
        }
    }
}
