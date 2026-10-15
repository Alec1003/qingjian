//! 暂存区：数字键选中的词先留在这里，确认后才写入应用并记学习。

use super::commit::LastCommit;
use super::input_log::InputSource;
use super::learning::Learner;
use super::{EXPLICIT_TRANSITION_WEIGHT, Engine};
use crate::candidate::{Candidate, CandidateKind};

/// 已经选中、还没写入应用的一段。
struct Staged {
    /// 确认时写入应用的文本。
    text: String,

    /// 记学习用的候选；繁体输出时文本已换回简体。
    candidate: Candidate,

    /// 这段候选吃掉的输入，按输入串记选择。
    input: String,

    /// 接受的敲错音节对，确认时才写入个人敲错表。
    typos: Vec<(String, String)>,

    /// 整句路径上的词。收进暂存时就算好，确认时拼音已经不在了。
    sentence_words: Option<Vec<crate::sentence::SentenceWord>>,

    /// 退格改过字。确认时仍写入剩下的文本，不再按原来的候选记学习。
    edited: bool,
}

/// 暂存的词和插入位置。光标在第 `cursor` 格之前，下一词插在这里。
#[derive(Default)]
pub(super) struct Staging {
    items: Vec<Staged>,

    /// 0 到 `items.len()`：下一词插入的位置，退格删的是它前面那一格的末字。
    cursor: usize,
}

impl Staging {
    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.cursor = 0;
    }
}

impl Engine {
    /// 把候选收进暂存区，并吃掉它对应的拼音。不写应用、不记学习。
    /// 吃不到拼音时返回 `false`，调用方仍按上屏处理。
    pub fn stage(&mut self, candidate: &Candidate) -> bool {
        if candidate.text.is_empty() {
            return false;
        }
        let sentence_words = (candidate.kind == CandidateKind::Sentence)
            .then(|| self.sentence_words(candidate))
            .flatten();
        let typos = match candidate.kind {
            CandidateKind::Chinese | CandidateKind::Sentence => self.accepted_typos(candidate),
            _ => Vec::new(),
        };
        let (consumed, input) = self.staging_span(candidate);
        if consumed == 0 {
            return false;
        }
        let staged = Staged {
            text: self.staged_text(candidate),
            candidate: self.staged_candidate(candidate),
            input,
            typos,
            sentence_words,
            edited: false,
        };
        self.forget_span_cache();
        self.composition.drain_prefix(consumed);
        self.aux_code = None;
        let at = self.staging.cursor.min(self.staging.items.len());
        self.staging.items.insert(at, staged);
        self.staging.cursor = at + 1;
        true
    }

    /// 把暂存区整段写入应用该用的文本，并按选词顺序记学习。空则 `None`。
    pub fn confirm_staging(&mut self) -> Option<String> {
        if self.staging.items.is_empty() {
            return None;
        }
        let items = std::mem::take(&mut self.staging.items);
        self.staging.cursor = 0;
        let composition_left = !self.composition.is_empty();
        let last = items.len() - 1;
        let mut text = String::new();
        for (index, staged) in items.iter().enumerate() {
            self.learn_staged(staged, composition_left || index < last);
            text.push_str(&staged.text);
        }
        Some(text)
    }

    /// 暂存区里还有词。
    pub fn has_staging(&self) -> bool {
        !self.staging.items.is_empty()
    }

    /// 暂存的各格文本，按顺序。
    pub fn staging_words(&self) -> Vec<String> {
        self.staging
            .items
            .iter()
            .map(|item| item.text.clone())
            .collect()
    }

    /// 下一词要插入的位置。
    pub fn staging_cursor(&self) -> usize {
        self.staging.cursor
    }

    /// 左右移动暂存光标。已经在两端时返回 `false`。
    pub fn move_staging_cursor(&mut self, step: isize) -> bool {
        let next = self.staging.cursor as isize + step;
        if next < 0 || next > self.staging.items.len() as isize {
            return false;
        }
        self.staging.cursor = next as usize;
        true
    }

    /// 把暂存光标放到 `cursor`，超出范围就收到最后。
    pub fn set_staging_cursor(&mut self, cursor: usize) {
        self.staging.cursor = cursor.min(self.staging.items.len());
    }

    /// 丢掉暂存区，不记学习。`committed` 是壳写入应用的文本，之后的退格按它计数。
    pub fn abandon_staging(&mut self, committed: &str) {
        self.staging.clear();
        if committed.is_empty() {
            return;
        }
        self.chain.reset();
        self.punctuation.note_committed(committed);
        self.history.record(committed);
        let mut plain = LastCommit::plain(committed);
        plain.chars = committed.chars().count();
        plain.tail = committed.chars().last();
        self.remember_commit(plain);
    }

    /// 删掉光标前一个字。这一格删空后才去掉，后面的词留着。光标已经在开头时返回 `false`。
    pub fn staging_backspace(&mut self) -> bool {
        if self.staging.cursor == 0 || self.staging.items.is_empty() {
            return false;
        }
        let index = self.staging.cursor - 1;
        let Some((split, _)) = self.staging.items[index].text.char_indices().next_back() else {
            self.staging.cursor -= 1;
            self.staging.items.remove(index);
            return true;
        };
        if split == 0 {
            self.staging.cursor -= 1;
            self.staging.items.remove(index);
            return true;
        }
        self.staging.items[index].text.truncate(split);
        self.staging.items[index].edited = true;
        true
    }

    /// 候选吃掉多少拼音。整段作用域的候选（英文、云端、快捷）吃光；词和整句按音节对齐。
    fn staging_span(&self, candidate: &Candidate) -> (usize, String) {
        match candidate.kind {
            CandidateKind::Chinese
            | CandidateKind::Sentence
            | CandidateKind::Emoji
            | CandidateKind::Kaomoji
            | CandidateKind::Symbol => self.consumed_by(candidate),
            CandidateKind::Code
            | CandidateKind::Cloud
            | CandidateKind::Generated
            | CandidateKind::English
            | CandidateKind::Shortcut
            | CandidateKind::Custom(_) => self.whole_scope(),
        }
    }

    /// 写入应用的文本：繁体候选保持候选上的字，自动空格只补词内部。
    fn staged_text(&self, candidate: &Candidate) -> String {
        let verbatim = matches!(
            candidate.kind,
            CandidateKind::Custom(_) | CandidateKind::Shortcut
        );
        if self.auto_space && !verbatim {
            crate::spacing::space_inner(&candidate.text)
        } else {
            candidate.text.clone()
        }
    }

    /// 记学习用的候选。繁体输出时词频仍按简体记。
    fn staged_candidate(&self, candidate: &Candidate) -> Candidate {
        let mut learned = candidate.clone();
        if let Some(simplified) = self.traditional.restore(&learned.text) {
            learned.text = simplified;
        }
        learned
    }

    /// 确认一格：补上收进时故意没记的词频、转移和输入日志。
    fn learn_staged(&mut self, staged: &Staged, buffer_left: bool) {
        if staged.edited {
            self.chain.reset();
            self.punctuation.note_committed(&staged.text);
            self.history.record(&staged.text);
            let mut plain = LastCommit::plain(&staged.text);
            plain.chars = staged.text.chars().count();
            plain.tail = staged.text.chars().last();
            self.remember_commit(plain);
            return;
        }
        let candidate = &staged.candidate;
        self.recording.clear();
        match candidate.kind {
            CandidateKind::Chinese | CandidateKind::Cloud | CandidateKind::Code => {
                self.learner.record(candidate);
                self.learner.record_choice(&staged.input, &candidate.text);
                for (typed, intended) in &staged.typos {
                    self.learner.record_typo(typed, intended);
                }
                self.record_word(
                    &candidate.text,
                    &candidate.syllables,
                    EXPLICIT_TRANSITION_WEIGHT,
                    true,
                    buffer_left,
                );
            }
            CandidateKind::Sentence => {
                for (typed, intended) in &staged.typos {
                    self.learner.record_typo(typed, intended);
                }
                if let Some(words) = &staged.sentence_words {
                    if let Some(word) = words.last().filter(|word| {
                        !word.placeholder
                            && !word.text.is_empty()
                            && word.text.bytes().all(|byte| byte.is_ascii_alphabetic())
                    }) {
                        self.learner.learn_english(&word.text);
                    }
                    let last = words.len().saturating_sub(1);
                    for (index, word) in words.iter().enumerate() {
                        self.record_word(
                            &word.text,
                            &word.syllables,
                            1,
                            false,
                            buffer_left || index < last,
                        );
                    }
                } else {
                    self.chain.reset();
                }
            }
            CandidateKind::English => {
                self.learner.record(candidate);
                self.learner.learn_english(&candidate.text);
                self.chain.reset();
            }
            CandidateKind::Shortcut
            | CandidateKind::Custom(_)
            | CandidateKind::Emoji
            | CandidateKind::Kaomoji
            | CandidateKind::Symbol
            | CandidateKind::Generated => self.chain.reset(),
        }
        let source = InputSource::from(candidate.kind);
        let log_id = self.log_commit(&staged.input, &staged.text, source);
        self.meter_commit(
            &staged.text,
            source,
            candidate.kind == CandidateKind::English,
        );
        self.punctuation.note_committed(&staged.text);
        self.history.record(&staged.text);
        let learned = matches!(
            candidate.kind,
            CandidateKind::Chinese
                | CandidateKind::Code
                | CandidateKind::Cloud
                | CandidateKind::Sentence
        );
        let commit = if learned {
            LastCommit {
                text: candidate.text.clone(),
                chars: staged.text.chars().count(),
                tail: staged.text.chars().last(),
                input: staged.input.clone(),
                chosen: matches!(
                    candidate.kind,
                    CandidateKind::Chinese | CandidateKind::Code | CandidateKind::Cloud
                )
                .then(|| candidate.text.clone()),
                transitions: std::mem::take(&mut self.recording),
                typos: staged.typos.clone(),
                erased: 0,
                log_id,
                phrase: None,
            }
        } else {
            let mut plain = LastCommit::plain(&candidate.text);
            plain.chars = staged.text.chars().count();
            plain.tail = staged.text.chars().last();
            plain
        };
        self.remember_commit(commit);
    }
}
