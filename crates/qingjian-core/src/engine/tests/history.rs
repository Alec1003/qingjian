//! 历史补全：以前上过屏的句子。次数多的整句优先；Tab 可以直接上屏，也可以先收进暂存区。

use super::engine;
use crate::Engine;

fn commit_text(engine: &mut Engine, input: &str, text: &str) {
    engine.set_input(input);
    let candidate = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == text)
        .unwrap();
    engine.commit(&candidate);
}

#[test]
fn a_whole_sentence_completes_from_its_prefix_and_tab_accepts_it() {
    let mut engine = engine();
    engine.set_input("kaifa");
    assert_eq!(engine.accept_history("开发开放"), "开发开放");
    assert!(engine.composition().is_empty());
    engine.set_input("kaifa");
    assert_eq!(
        engine.history_completion_for("开发").as_deref(),
        Some("开发开放")
    );
}

#[test]
fn separate_commits_are_not_joined() {
    let mut engine = engine();
    commit_text(&mut engine, "kaifa", "开发");
    commit_text(&mut engine, "kaifang", "开放");
    engine.set_input("kaifa");
    assert_eq!(engine.history_completion_for("开发"), None);
}

#[test]
fn disabled_and_private_input_are_not_offered() {
    let mut engine = engine();
    engine.set_history_completion(false);
    commit_text(&mut engine, "kaifa", "开发");
    commit_text(&mut engine, "kaifang", "开放");
    engine.set_input("kaifa");
    assert_eq!(engine.history_completion_for("开发"), None);

    engine.set_history_completion(true);
    engine.set_input("kaifa");
    engine.accept_history("开发开放");
    engine.set_private(true);
    engine.set_input("xian");
    engine.accept_history("先下");
    engine.set_private(false);
    engine.set_input("kaifa");
    assert_eq!(
        engine.history_completion_for("开发").as_deref(),
        Some("开发开放")
    );
    engine.set_input("xian");
    assert_eq!(engine.history_completion_for("先"), None);
    engine.set_english_mode(true);
    engine.set_input("kaifa");
    assert_eq!(engine.history_completion_for("开发"), None);
}

#[test]
fn the_more_frequent_utterance_is_offered() {
    let mut engine = engine();
    for _ in 0..2 {
        engine.set_input("kaifa");
        engine.accept_history("开发开放");
    }
    engine.set_input("kaifa");
    engine.accept_history("开发先");
    engine.set_input("kaifa");
    assert_eq!(
        engine.history_completion_for("开发").as_deref(),
        Some("开发开放")
    );
}

#[test]
fn a_digit_after_staged_words_stays_in_the_line() {
    let mut engine = engine();
    engine.set_input("kaifa");
    let word = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开发")
        .unwrap();
    assert!(engine.stage(&word));
    assert!(engine.stage_literal("1"));
    assert_eq!(
        engine.staging_words(),
        vec!["开发".to_owned(), "1".to_owned()]
    );
    assert!(engine.composition().is_empty());
    assert!(!engine.stage_literal(""));
}

#[test]
fn confirming_several_staged_words_records_the_whole_line() {
    let mut engine = engine();
    engine.hold_phrases();
    engine.set_input("kaifa");
    let first = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开发")
        .unwrap();
    assert!(engine.stage(&first));
    engine.set_input("kaifang");
    let second = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开放")
        .unwrap();
    assert!(engine.stage(&second));
    let text = engine.confirm_staging().unwrap();
    assert_eq!(text, "开发开放");
    engine.release_phrases(&text);
    engine.set_input("kaifa");
    let again = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开发")
        .unwrap();
    assert!(engine.stage(&again));
    assert_eq!(
        engine.history_matches_for("开发"),
        vec!["开发开放".to_owned()]
    );
}

#[test]
fn staged_prefix_lists_longer_sentences_by_frequency() {
    let mut engine = engine();
    for _ in 0..2 {
        engine.set_input("kaifa");
        engine.accept_history("开发开放");
    }
    engine.set_input("kaifa");
    engine.accept_history("开发先");
    engine.set_input("kaifa");
    let candidate = engine
        .query()
        .unwrap()
        .candidates
        .items
        .into_iter()
        .find(|candidate| candidate.text == "开发")
        .unwrap();
    assert!(engine.stage(&candidate));
    assert_eq!(
        engine.history_matches_for("开发"),
        vec!["开发开放".to_owned(), "开发先".to_owned()]
    );
    assert!(engine.adopt_history_match("开发开放"));
    assert_eq!(engine.staging_words(), vec!["开发开放".to_owned()]);
    assert!(engine.history_matches_for("开发开放").is_empty());
    assert!(!engine.adopt_history_match("开发先"));
}

#[test]
fn tab_can_stage_the_history_sentence_before_it_is_written() {
    let mut engine = engine();
    engine.set_input("kaifa");
    engine.accept_history("开发开放");
    engine.set_input("kaifa");
    let phrase = engine.history_completion_for("开发").unwrap();
    assert!(engine.stage_history(&phrase));
    assert!(engine.composition().is_empty());
    assert_eq!(engine.staging_words(), vec!["开发开放".to_owned()]);
    assert_eq!(engine.confirm_staging().as_deref(), Some("开发开放"));
    assert!(!engine.has_staging());
}
