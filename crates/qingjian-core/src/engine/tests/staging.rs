//! 暂存区：先收词，确认才上屏、才记学习。

use std::sync::{Arc, Mutex};

use super::*;

struct RecordingLearner(Arc<Mutex<Vec<String>>>);

impl Learner for RecordingLearner {
    fn record(&mut self, candidate: &Candidate) {
        self.0.lock().expect("记录").push(candidate.text.clone());
    }

    fn weight(&self, _text: &str) -> u32 {
        0
    }
}

#[test]
fn stage_keeps_the_word_and_confirm_returns_it() {
    let mut engine = engine();
    engine.set_input("kaifa");
    let developed = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开发")
        .expect("开发");
    assert!(engine.stage(&developed));
    assert!(engine.composition().is_empty());
    assert_eq!(engine.staging_words(), vec!["开发".to_owned()]);
    assert_eq!(engine.confirm_staging().as_deref(), Some("开发"));
    assert!(!engine.has_staging());
    assert!(engine.confirm_staging().is_none());
}

#[test]
fn stage_leaves_the_rest_of_the_pinyin() {
    let mut engine = engine();
    engine.set_input("kaifa");
    let kai = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开")
        .expect("开");
    assert!(engine.stage(&kai));
    assert!(!engine.composition().is_empty(), "后面的拼音还在");
    assert_eq!(engine.staging_words(), vec!["开".to_owned()]);
}

#[test]
fn cursor_inserts_and_backspace_removes_one_slot() {
    let mut engine = engine();
    engine.set_input("kai");
    let kai = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开")
        .expect("开");
    assert!(engine.stage(&kai));
    assert!(!engine.move_staging_cursor(1));
    assert!(engine.move_staging_cursor(-1));
    assert_eq!(engine.staging_cursor(), 0);

    engine.set_input("bu");
    let bu = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "不")
        .expect("不");
    assert!(engine.stage(&bu));
    assert_eq!(
        engine.staging_words(),
        vec!["不".to_owned(), "开".to_owned()]
    );
    assert_eq!(engine.staging_cursor(), 1);

    assert!(engine.staging_backspace());
    assert_eq!(engine.staging_words(), vec!["开".to_owned()]);
    assert!(!engine.staging_backspace());
}

#[test]
fn confirm_records_learning_and_stage_does_not() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine().with_learner(Box::new(RecordingLearner(recorded.clone())));
    engine.set_input("kai");
    let kai = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开")
        .expect("开");
    assert!(engine.stage(&kai));
    assert!(recorded.lock().expect("记录").is_empty());
    assert_eq!(engine.confirm_staging().as_deref(), Some("开"));
    assert_eq!(recorded.lock().expect("记录").as_slice(), ["开"]);
}

#[test]
fn clear_drops_staging() {
    let mut engine = engine();
    engine.set_input("kai");
    let kai = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开")
        .expect("开");
    engine.stage(&kai);
    engine.clear();
    assert!(!engine.has_staging());
}

#[test]
fn staging_translation_uses_the_learning_language() {
    let submitted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut engine = engine();
    engine.set_translator(Box::new(FixedTranslator));
    engine.set_predictor(Box::new(EchoPredictor {
        submitted: std::rc::Rc::clone(&submitted),
        replies: Vec::new(),
        sentence: false,
    }));
    let text = "帮我push到github";
    assert!(engine.request_staging_translation(text).is_some());
    assert_eq!(submitted.borrow().last().unwrap().target_language, "en");
    assert!(engine.request_translation(text).is_some());
    assert_eq!(submitted.borrow().last().unwrap().target_language, "zh");
}
