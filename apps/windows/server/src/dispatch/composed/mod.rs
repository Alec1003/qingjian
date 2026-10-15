//! 组句的展示状态：缓冲变化时重查候选并重建 [`Composed`]，云端词异步并入，高亮 / 翻页，按状态生成给 DLL 的帧。

mod sentence_translation;
mod state;

use qingjian_core::{
    Candidate, CandidateKind, CandidateLayout, CandidateList, CloudWord, Language, Query, Sense,
    SurroundingText, Translation,
};
use qingjian_platform::protocol::{Frame, PROTOCOL_VERSION, PreeditKind, PreeditSegment};

use self::sentence_translation::SentenceTranslation;
pub(in crate::dispatch) use self::sentence_translation::StagingGloss;
pub(super) use self::state::{Composed, TypedKeys};
use super::Router;

impl Router {
    /// 缓冲变化后：按 Engine 状态重建 [`Composed`]，发一次云联想请求，归零高亮与整句补全。
    pub(super) fn recompose(&mut self) {
        let previous_sentence_translation =
            self.composed.take().and_then(|composed| match composed {
                Composed::Candidates {
                    sentence_translation,
                    ..
                } => sentence_translation,
                Composed::Raw { .. } => None,
            });
        self.highlight = 0;
        self.navigated = false;
        self.sentence = None;
        if self.engine.composition().is_empty() {
            // 暂存区还在时只撤组句联想，译文继续等；都空了再整段取消。
            if self.engine.has_staging() {
                self.engine.cancel_composition_prediction();
            } else {
                self.engine.cancel_prediction();
            }
            self.composed = None;
            self.stop_rescoring();
            self.sync_staging_translation();
            return;
        }
        self.engine.cancel_composition_prediction();
        self.attach_loaded_model();
        let built = self.engine.query().ok().map(|query| {
            let (preedit, cursor, typed_keys) = marked_parts(&query);
            (query.candidates.items.clone(), preedit, cursor, typed_keys)
        });
        self.composed = Some(match built {
            Some((items, preedit, cursor, typed_keys)) => {
                let layout =
                    CandidateLayout::new(items, self.config.page_size, self.config.cloud_slots);
                if self.config.cloud_enabled && self.engine.prediction_enabled() {
                    let surrounding = self.surrounding_for_prediction();
                    self.engine.request_prediction(surrounding, layout.local());
                }
                let sentence_candidate = (self.config.sentence_translation
                    && !self.staging_translation_wanted()
                    && self.engine.learning_language() != Language::Chinese)
                    .then(|| {
                        layout
                            .local()
                            .iter()
                            .find(|candidate| candidate.kind == CandidateKind::Sentence)
                    })
                    .flatten();
                let sentence_translation = sentence_candidate.and_then(|candidate| {
                    if let Some(annotation) = previous_sentence_translation
                        && annotation.text == candidate.text
                    {
                        return Some(annotation);
                    }
                    self.engine.cancel_sentence_translation();
                    self.engine
                        .request_sentence_translation(&candidate.text)
                        .map(|sequence| {
                            SentenceTranslation::pending(candidate.text.clone(), sequence)
                        })
                });
                if sentence_translation.is_none() && !self.staging_translation_wanted() {
                    self.engine.cancel_sentence_translation();
                }
                Composed::Candidates {
                    preedit,
                    cursor,
                    typed_keys,
                    layout,
                    sentence_translation,
                }
            }
            None => {
                self.cancel_prediction();
                let composition = self.engine.composition();
                let text = composition.text().to_owned();
                let cursor = text[..composition.cursor()].chars().count();
                Composed::Raw { text, cursor }
            }
        });
        self.schedule_rescoring();
        self.sync_staging_translation();
    }

    /// 拉一次云联想结果：云端词并进候选布局，整句补全记下；翻译评审时结果是译文。
    pub(super) fn poll_prediction(&mut self) {
        if !self.engine.prediction_enabled() {
            return;
        }
        let Some(prediction) = self.engine.poll_prediction() else {
            return;
        };
        if self.translation.is_some() {
            match prediction.sentence {
                Some(text) => {
                    if let Some(translation) = self.translation.as_mut() {
                        translation.result = Some(text);
                    }
                }
                None => {
                    tracing::info!("翻译选中文字：云端没有给出译文");
                    self.translation = None;
                }
            }
            return;
        }
        if self
            .staging_gloss
            .as_ref()
            .is_some_and(|gloss| gloss.sequence == prediction.sequence)
        {
            if let Some(gloss) = self.staging_gloss.as_mut() {
                gloss.translation = prediction.sentence.filter(|text| !text.is_empty());
            }
            return;
        }
        let is_sentence_translation = matches!(
            &self.composed,
            Some(Composed::Candidates {
                sentence_translation: Some(annotation),
                ..
            }) if annotation.sequence == prediction.sequence
        );
        if is_sentence_translation {
            if let Some(Composed::Candidates {
                sentence_translation: Some(annotation),
                ..
            }) = self.composed.as_mut()
            {
                annotation.translation = prediction.sentence.map(|text| {
                    Translation::new(
                        self.engine.learning_language(),
                        vec![Sense {
                            part_of_speech: None,
                            text,
                            reading: None,
                            fresh: false,
                        }],
                    )
                });
            }
            return;
        }
        if let Some(Composed::Candidates { layout, .. }) = self.composed.as_mut() {
            let words: Vec<Candidate> = prediction
                .words
                .into_iter()
                .map(CloudWord::into_candidate)
                .collect();
            layout.set_cloud(words);
            self.sentence = prediction.sentence;
        }
    }

    pub(super) fn cancel_prediction(&mut self) {
        if self.engine.prediction_enabled() {
            self.engine.cancel_prediction();
        }
    }

    /// DLL 送来新的光标前文后重发一次云联想：作废在飞的空上下文请求，让云端拿得到 before。
    pub(super) fn refresh_prediction(&mut self) {
        if !self.config.cloud_enabled {
            return;
        }
        let Some(Composed::Candidates { layout, .. }) = &self.composed else {
            return;
        };
        let local = layout.local().to_vec();
        let surrounding = self.surrounding_for_prediction();
        self.engine.request_prediction(surrounding, &local);
    }

    /// 给当前的本地整句候选补上已经返回的云端译文。
    ///
    /// 候选布局保留查询原样；画帧、点击和译词快捷键各自拿副本时统一补这一层，
    /// 才不会让同一条译文只有显示、不能上屏。
    pub(super) fn annotate_sentence_translation(&self, candidates: &mut CandidateList) {
        let Some(Composed::Candidates {
            sentence_translation: Some(annotation),
            ..
        }) = &self.composed
        else {
            return;
        };
        let Some(translation) = &annotation.translation else {
            return;
        };
        for candidate in &mut candidates.items {
            if candidate.kind == CandidateKind::Sentence && candidate.text == annotation.text {
                candidate.translation = Some(translation.clone());
            }
        }
    }

    /// DLL 报来的光标前文包成云联想要的形状；没有就 `None`（Engine 只靠拼音猜）。
    pub(super) fn surrounding_for_prediction(&self) -> Option<SurroundingText> {
        (!self.surrounding_before.is_empty()).then(|| SurroundingText {
            before: self.surrounding_before.clone(),
            after: String::new(),
        })
    }

    /// 高亮移动 `delta`，夹在 `[0, 末尾]`，到页边自然换页。
    pub(super) fn move_highlight(&mut self, delta: isize) {
        let count = self.candidate_count();
        if count == 0 {
            self.highlight = 0;
            return;
        }
        let next = (self.highlight as isize + delta).clamp(0, count as isize - 1) as usize;
        self.navigated |= next != self.highlight;
        self.highlight = next;
    }

    /// 整页翻 `step`，高亮落到目标页第一个候选。
    pub(super) fn page(&mut self, step: isize) {
        let count = self.candidate_count();
        if count == 0 {
            self.highlight = 0;
            return;
        }
        let page_size = self.config.page_size;
        let page_count = count.div_ceil(page_size);
        let current = (self.highlight / page_size) as isize;
        let target = (current + step).clamp(0, page_count as isize - 1) as usize;
        if target != current as usize {
            self.navigated = true;
            self.engine.note_page_turn();
        }
        self.highlight = (target * page_size).min(count - 1);
    }

    /// 往 `step` 方向还有没有页可翻。
    pub(super) fn can_page(&self, step: isize) -> bool {
        let page_count = self.candidate_count().div_ceil(self.config.page_size) as isize;
        let target = (self.highlight / self.config.page_size) as isize + step;
        (0..page_count).contains(&target)
    }

    pub(super) fn candidate_count(&self) -> usize {
        match &self.composed {
            Some(Composed::Candidates { layout, .. }) => layout.len(),
            _ => 0,
        }
    }

    /// 候选布局里第 `index` 个（跨页下标）。
    pub(super) fn layout_candidate(&self, index: usize) -> Option<Candidate> {
        match &self.composed {
            Some(Composed::Candidates { layout, .. }) => layout.candidate(index).cloned(),
            _ => None,
        }
    }

    pub(super) fn commit_index(&mut self, index: usize) -> Option<String> {
        let candidate = self.layout_candidate(index)?;
        Some(self.engine.commit(&candidate))
    }

    /// 按当前状态生成一帧：翻译评审优先；没在组句给空帧；否则给高亮所在的那一页。
    /// 焦点会话的 DLL 比 Server 老时按老协议降级（见 [`Self::downgrade_for_old_dll`]）。
    pub(super) fn current_frame(&self) -> Frame {
        let mut frame = self.raw_frame();
        self.show_typed_keys(&mut frame);
        self.hold_staging_composition(&mut frame);
        self.downgrade_for_old_dll(&mut frame);
        frame
    }

    /// 给 DLL 的帧只管应用输入框：双拼「输入框显示原始按键」开着时换成敲的键，拼音行由 Server 自绘照旧全拼。
    fn show_typed_keys(&self, frame: &mut Frame) {
        if self.translation.is_some() {
            return;
        }
        let Some(Composed::Candidates {
            typed_keys: Some(keys),
            ..
        }) = &self.composed
        else {
            return;
        };
        frame.preedit = vec![PreeditSegment {
            text: keys.text.clone(),
            kind: PreeditKind::Typed,
        }];
        frame.cursor = keys.cursor;
    }

    /// 拼音已经空、暂存区还有词时，把这些词放进发给 DLL 的拼音行。
    /// 候选窗仍只按 [`Self::self_drawn_frame`] 画暂存区。输入框里留着这段组句，
    /// 微信、Cursor 才会把回车交给输入法，而不是当成发送；候选窗也能跟着这段文字走，不会停在桌面上。
    fn hold_staging_composition(&self, frame: &mut Frame) {
        if !frame.preedit.iter().all(|segment| segment.text.is_empty()) || frame.staging.is_empty()
        {
            return;
        }
        let cursor = frame.staging_cursor.min(frame.staging.len());
        let mut text = String::new();
        let mut at = 0usize;
        for (index, word) in frame.staging.iter().enumerate() {
            if index == cursor {
                at = text.chars().count();
            }
            text.push_str(word);
        }
        if cursor == frame.staging.len() {
            at = text.chars().count();
        }
        frame.preedit = vec![PreeditSegment {
            text,
            kind: PreeditKind::Typed,
        }];
        frame.cursor = at;
    }

    /// 自绘候选窗用的帧：不做老 DLL 降级，码段照常画。暂存区不写进这里的拼音行，避免和暂存区画重。
    pub(super) fn self_drawn_frame(&self) -> Frame {
        self.raw_frame()
    }

    /// 协议比 Server 老的 DLL 不认识 `AuxCode` 段与颜文字、符号两种候选，收到会整条消息解析失败：
    /// 码段降级成普通拼音段，那两种候选按 emoji 发（DLL 不看候选种类，只要解得出来）。
    fn downgrade_for_old_dll(&self, frame: &mut Frame) {
        if self.focused_dll_protocol() >= PROTOCOL_VERSION {
            return;
        }
        for segment in &mut frame.preedit {
            if segment.kind == PreeditKind::AuxCode {
                segment.kind = PreeditKind::Typed;
            }
        }
        for candidate in &mut frame.candidates.items {
            if matches!(
                candidate.kind,
                CandidateKind::Kaomoji | CandidateKind::Symbol
            ) {
                candidate.kind = CandidateKind::Emoji;
            }
        }
    }

    /// 焦点会话的 DLL 协议版本；没有焦点会话时按最新（不必降级）。
    pub(super) fn focused_dll_protocol(&self) -> u32 {
        self.focused
            .and_then(|session| self.sessions.get(&session))
            .map_or(PROTOCOL_VERSION, |info| info.protocol)
    }

    fn raw_frame(&self) -> Frame {
        if let Some(translation) = &self.translation {
            return self.translation_frame(translation);
        }
        let mut frame = self.composed_frame();
        frame.symbol_panel = self.panel_open();
        frame.staging = self.engine.staging_words();
        frame.staging_cursor = self.engine.staging_cursor();
        frame.staging_translation = self
            .staging_gloss
            .as_ref()
            .and_then(|gloss| gloss.translation.clone());
        frame
    }

    /// 暂存区有词、开关开着、学习语言不是中文时，译文走暂存区这一路。
    fn staging_translation_wanted(&self) -> bool {
        self.config.staging_translation
            && self.engine.has_staging()
            && self.engine.learning_language() != Language::Chinese
    }

    /// 暂存区整句变了就重发；没变就留着已经回来的译文。空了就取消还没发出的请求。
    fn sync_staging_translation(&mut self) {
        if !self.staging_translation_wanted() {
            if self.staging_gloss.is_some() {
                self.engine.cancel_staging_translation();
                self.staging_gloss = None;
            }
            return;
        }
        let text = self.engine.staging_words().join("");
        if self
            .staging_gloss
            .as_ref()
            .is_some_and(|gloss| gloss.text == text)
        {
            return;
        }
        self.engine.cancel_staging_translation();
        self.staging_gloss = self
            .engine
            .request_staging_translation(&text)
            .map(|sequence| StagingGloss {
                text,
                sequence,
                translation: None,
            });
    }

    fn composed_frame(&self) -> Frame {
        match &self.composed {
            None => Frame::default(),
            Some(Composed::Raw { text, cursor }) => Frame {
                preedit: vec![PreeditSegment {
                    text: text.clone(),
                    kind: PreeditKind::Typed,
                }],
                preedit_mode: self.config.preedit,
                cursor: *cursor,
                candidates: CandidateList { items: Vec::new() },
                highlight: usize::MAX,
                page: 0,
                page_count: 1,
                layout: self.config.layout,
                appearance: self.config.appearance,
                aux_code_show: self.config.aux_code_show,
                sentence: None,
                notice: self.notice.clone(),
                symbol_panel: false,
                staging: Vec::new(),
                staging_cursor: 0,
                staging_translation: None,
            },
            Some(Composed::Candidates {
                preedit,
                cursor,
                layout,
                ..
            }) => {
                let page_size = self.config.page_size;
                let highlight = self.highlight.min(layout.len().saturating_sub(1));
                let page = highlight / page_size;
                let items: Vec<Candidate> = layout
                    .page(page)
                    .into_iter()
                    .filter_map(|cell| cell.candidate().cloned())
                    .collect();
                let mut candidates = CandidateList { items };
                self.engine.annotate(&mut candidates);
                self.annotate_sentence_translation(&mut candidates);
                Frame {
                    preedit: preedit.clone(),
                    preedit_mode: self.config.preedit,
                    cursor: *cursor,
                    candidates,
                    highlight: highlight - page * page_size,
                    page,
                    page_count: layout.pages().max(1),
                    layout: self.config.layout,
                    appearance: self.config.appearance,
                    aux_code_show: self.config.aux_code_show,
                    sentence: self.sentence.clone(),
                    notice: self.notice.clone(),
                    symbol_panel: false,
                    staging: Vec::new(),
                    staging_cursor: 0,
                    staging_translation: None,
                }
            }
        }
    }
}

/// 一次查询的拼音行分段与光标，以及输入框另显示原始按键时的那一串。
/// 光标用 Core 的映射：自动补的 `'` 会让显示串比敲的长。
pub(super) fn marked_parts(query: &Query) -> (Vec<PreeditSegment>, usize, Option<TypedKeys>) {
    let preedit = query.marked_segments().iter().map(Into::into).collect();
    let typed_keys = query.shuangpin_raw_preedit.then(|| TypedKeys {
        text: query.marked_text(),
        cursor: query.marked_cursor(),
    });
    (preedit, query.segments_cursor(), typed_keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{Router, RouterConfig};
    use qingjian_core::Engine;
    use qingjian_dictionary::Dictionary;
    use qingjian_platform::protocol::{ClientMessage, KeyEvent, KeyModifiers, SessionId};

    fn router() -> Router {
        let mut router = Router::new(
            Engine::new(Dictionary::parse("你\tni\t100\n").unwrap()),
            RouterConfig::default(),
        );
        router.handle(ClientMessage::OpenSession {
            session: SessionId(1),
            app: None,
            protocol: PROTOCOL_VERSION,
        });
        router
    }

    fn compose(router: &mut Router, text: &str) {
        for c in text.chars() {
            router
                .handle(ClientMessage::Key {
                    session: SessionId(1),
                    event: KeyEvent::new(c as u32, Some(c), KeyModifiers::default()),
                })
                .unwrap();
        }
    }

    /// 回归 #283：DLL 报来的光标前文要成为云联想的 `before` 上下文；
    /// 没前文时不带上下文，空文本不算上下文，组句结束即作废。
    #[test]
    fn surrounding_context_flows_to_prediction_and_clears_on_reset() {
        let mut router = router();
        compose(&mut router, "ni");
        assert!(router.surrounding_for_prediction().is_none());

        router.handle(ClientMessage::Surrounding {
            session: SessionId(1),
            text: "这个文件权限有问题，".to_owned(),
        });
        let surrounding = router.surrounding_for_prediction().expect("应带上前文");
        assert_eq!(surrounding.before, "这个文件权限有问题，");
        assert_eq!(surrounding.after, "");

        // 空文本不算上下文（Engine 只靠拼音猜，不猜错的）
        router.handle(ClientMessage::Surrounding {
            session: SessionId(1),
            text: String::new(),
        });
        assert!(router.surrounding_for_prediction().is_none());

        // 组句结束：前文作废，等下一段组句 DLL 再送
        router.handle(ClientMessage::Surrounding {
            session: SessionId(1),
            text: "别的内容".to_owned(),
        });
        router.stop_rescoring();
        assert!(router.surrounding_for_prediction().is_none());
    }

    /// 不是聚焦会话的前文不收，防止换应用后把 A 应用的文字带进 B 的联想。
    #[test]
    fn surrounding_from_unfocused_session_is_dropped() {
        let mut router = router();
        compose(&mut router, "ni");
        router.handle(ClientMessage::Surrounding {
            session: SessionId(99),
            text: "别的应用的前文".to_owned(),
        });
        assert!(router.surrounding_for_prediction().is_none());
    }
}
