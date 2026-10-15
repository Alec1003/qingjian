//! 一条本地整句候选的异步译文。

use qingjian_core::Translation;

pub(crate) struct SentenceTranslation {
    pub(crate) text: String,

    pub(crate) sequence: u64,

    pub(crate) translation: Option<Translation>,
}

/// 暂存区整句的译文。`translation` 还没回来时是 `None`。
pub(in crate::dispatch) struct StagingGloss {
    pub(in crate::dispatch) text: String,

    pub(in crate::dispatch) sequence: u64,

    pub(in crate::dispatch) translation: Option<String>,
}

impl SentenceTranslation {
    pub(crate) fn pending(text: String, sequence: u64) -> Self {
        Self {
            text,
            sequence,
            translation: None,
        }
    }
}
