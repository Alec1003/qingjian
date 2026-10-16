//! 历史补全：高亮候选是以前上过屏的句子的前缀时，给出整句。

use std::path::Path;

use super::Engine;

impl Engine {
    /// 历史补全的句子文件。读得出来就装上；没有这个文件从空开始。
    pub fn set_phrase_path(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        if let Ok(text) = std::fs::read_to_string(path) {
            self.phrases.load(&text);
        }
        self.phrase_path = Some(path.to_owned());
    }

    /// 开或关历史补全。关掉之后不再记、也不再提示。
    pub fn set_history_completion(&mut self, enabled: bool) {
        self.history_completion = enabled;
    }

    /// 高亮候选（或任何当前组句文本）对上以前的句子时，返回出现次数最多的那一整句。
    /// 已经是繁体的前缀会先换回简体再比，返回值按当前输出字形换好。
    /// 暂存区还有词时不给：整句里含着还没写入应用的字，接受会叠上去。
    pub fn history_completion_for(&self, displayed: &str) -> Option<String> {
        if !self.history_completion
            || self.private
            || self.english_mode
            || self.has_staging()
            || self.composition.is_empty()
        {
            return None;
        }
        let restored = self.traditional.restore(displayed);
        let prefix = restored.as_deref().unwrap_or(displayed);
        let phrase = self.phrases.suggest(prefix)?;
        Some(self.traditional.convert_remember(phrase))
    }

    /// 暂存区里的文字是以前某几句的开头时，按出现次数从高到低返回那些更长的句子。
    /// 次数相同的，最近的在前。繁体前缀先换回简体再比，返回值按当前输出字形换好。
    pub fn history_matches_for(&self, displayed: &str) -> Vec<String> {
        if !self.history_completion || self.private || self.english_mode {
            return Vec::new();
        }
        let restored = self.traditional.restore(displayed);
        let prefix = restored.as_deref().unwrap_or(displayed);
        self.phrases
            .ranked(prefix)
            .into_iter()
            .map(|phrase| self.traditional.convert_remember(phrase))
            .collect()
    }

    /// 同一次按键里会连着上屏好几段（暂存区里的词，再跟上当前候选）。先拦住按词记的句子。
    pub fn hold_phrases(&mut self) {
        self.phrase_hold = true;
    }

    /// 这次按键实际写进应用的是 `text` 这一整段。按词拦住的不记，改记这一整段。
    pub fn release_phrases(&mut self, text: &str) {
        self.phrase_hold = false;
        self.note_phrase(text);
    }

    /// 记一段上屏文本。私密输入、开关关着、空串都不记。繁体先换回简体。
    /// [`Self::hold_phrases`] 开着时先不记，等 [`Self::release_phrases`] 记写进应用的整段。
    pub(super) fn note_phrase(&mut self, text: &str) {
        if self.phrase_hold || !self.history_completion || self.private || text.is_empty() {
            return;
        }
        let restored = self.traditional.restore(text);
        let text = restored.as_deref().unwrap_or(text);
        self.phrases.note(text);
    }

    pub(super) fn save_phrases(&mut self) {
        self.phrases.save(self.phrase_path.as_deref());
    }
}
