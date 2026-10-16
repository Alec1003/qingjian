//! 以前上过屏的句子，用来做历史补全。
//!
//! 每一次上屏单独记成一句并计一次，不把前后几次拼在一起。组句中的高亮文本是其中一句的前缀时，
//! 补出整句；次数多的优先，次数相同用最近的一句。容量有上限，只留在本机。

use std::collections::VecDeque;
use std::path::Path;

use crate::storage::write_atomic_str;

/// 最多记多少句。再多就丢最旧的。
const MAX_PHRASES: usize = 200;

/// 短于这么多个字的不记、也不当补全前缀。一个字对上的句子太多。
const MIN_CHARS: usize = 2;

/// 已经收束的一句和它出现过的次数。
#[derive(Debug, Clone)]
struct Phrase {
    text: String,

    count: u32,
}

/// 以前上过屏的句子。
#[derive(Debug, Clone, Default)]
pub struct PhraseHistory {
    /// 已记下的句子，最近出现的在末尾。
    phrases: VecDeque<Phrase>,

    /// 有没有还没落盘的改动。
    dirty: bool,
}

impl PhraseHistory {
    /// 把刚上屏的文字记成一句。同一句再出现就加一次，不和上一句拼在一起。
    pub(crate) fn note(&mut self, text: &str) {
        for line in text.split(['\n', '\r']) {
            let line = line.trim();
            if line.chars().count() >= MIN_CHARS {
                self.bump(line, 1);
            }
        }
    }

    /// `prefix` 是某句的开头、而且那句更长时，返回整句。次数多的优先，次数相同用最近的一句。
    pub(crate) fn suggest(&self, prefix: &str) -> Option<&str> {
        self.ranked(prefix).into_iter().next()
    }

    /// 以 `prefix` 开头、并且更长的句子。次数从高到低，次数相同则最近的在前。
    pub(crate) fn ranked(&self, prefix: &str) -> Vec<&str> {
        if prefix.chars().count() < MIN_CHARS {
            return Vec::new();
        }
        let mut matches = self
            .phrases
            .iter()
            .enumerate()
            .filter(|(_, phrase)| longer_prefix(&phrase.text, prefix))
            .map(|(index, phrase)| (phrase.count, index, phrase.text.as_str()))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
        matches.into_iter().map(|(_, _, text)| text).collect()
    }

    /// 从落盘的文本读回。`次数<Tab>句子` 一行一句；没有 Tab 的旧行按一次计。
    /// 以前未收束的 `+<Tab>句子` 按一次计。空行跳过，超出容量留最新的。
    pub(crate) fn load(&mut self, text: &str) {
        self.phrases.clear();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let line = line.strip_prefix("+\t").unwrap_or(line);
            let (count, phrase) = split_count(line);
            if phrase.chars().count() < MIN_CHARS || count == 0 {
                continue;
            }
            self.bump(phrase, count);
        }
        self.dirty = false;
    }

    /// 有改动才写。写失败留着 dirty，下次再试。
    pub(crate) fn save(&mut self, path: Option<&Path>) {
        let Some(path) = path else {
            return;
        };
        if !self.dirty {
            return;
        }
        match write_atomic_str(path, &self.encode()) {
            Ok(()) => self.dirty = false,
            Err(error) => tracing::warn!(path = %path.display(), %error, "历史补全落盘失败"),
        }
    }

    fn bump(&mut self, phrase: &str, by: u32) {
        if let Some(position) = self.phrases.iter().position(|item| item.text == phrase)
            && let Some(mut item) = self.phrases.remove(position)
        {
            item.count = item.count.saturating_add(by);
            self.phrases.push_back(item);
        } else {
            self.phrases.push_back(Phrase {
                text: phrase.to_owned(),
                count: by,
            });
        }
        while self.phrases.len() > MAX_PHRASES {
            self.phrases.pop_front();
        }
        self.dirty = true;
    }

    fn encode(&self) -> String {
        let mut text = String::new();
        for phrase in &self.phrases {
            text.push_str(&phrase.count.to_string());
            text.push('\t');
            text.push_str(&phrase.text);
            text.push('\n');
        }
        text
    }
}

/// 这一行是不是 `次数<Tab>句子`。次数不是正整数时，整行就是句子、计一次。
fn split_count(line: &str) -> (u32, &str) {
    if let Some((count, phrase)) = line.split_once('\t')
        && let Ok(count) = count.parse::<u32>()
        && count > 0
        && !phrase.is_empty()
    {
        return (count, phrase);
    }
    (1, line)
}

fn longer_prefix(phrase: &str, prefix: &str) -> bool {
    phrase.starts_with(prefix) && phrase.chars().count() > prefix.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_commits_stay_separate_and_the_highest_count_wins() {
        let mut history = PhraseHistory::default();
        history.note("我叫做");
        history.note("我叫做王秋冬");
        history.note("我叫做王秋冬");
        assert_eq!(history.suggest("我叫做"), Some("我叫做王秋冬"));
        assert_eq!(history.phrases.len(), 2);
        assert_eq!(history.suggest("我叫"), Some("我叫做王秋冬"));
        assert_eq!(history.suggest("你"), None);
        assert_eq!(history.suggest("我叫做王秋冬"), None);
    }

    #[test]
    fn equal_counts_prefer_the_most_recent_sentence() {
        let mut history = PhraseHistory::default();
        history.note("你好世界");
        history.note("你好吗");
        assert_eq!(history.suggest("你好"), Some("你好吗"));
    }

    #[test]
    fn the_highest_count_wins_and_round_trip_keeps_it() {
        let mut history = PhraseHistory::default();
        history.note("你好世界");
        history.note("你好世界");
        history.note("你好吗");
        assert_eq!(history.suggest("你好"), Some("你好世界"));
        let mut loaded = PhraseHistory::default();
        loaded.load(&history.encode());
        assert_eq!(loaded.suggest("你好"), Some("你好世界"));
        assert_eq!(loaded.phrases.len(), 2);
    }

    #[test]
    fn decimal_point_does_not_close_and_load_keeps_the_newest() {
        let mut history = PhraseHistory::default();
        history.note("3.14");
        assert_eq!(history.suggest("3."), Some("3.14"));
        history.note("版本");
        let mut text = String::new();
        for index in 0..MAX_PHRASES + 5 {
            text.push_str(&format!("句子{index:04}\n"));
        }
        history.load(&text);
        assert_eq!(history.phrases.len(), MAX_PHRASES);
        assert_eq!(
            history.phrases.back().map(|phrase| phrase.text.as_str()),
            Some("句子0204")
        );
        assert!(history.suggest("3.14").is_none());
    }
}
