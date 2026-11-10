use cadocs_search::{Analyzer, Language, Segment};

fn english() -> Analyzer {
    Analyzer::new(Language::parse("en").unwrap())
}

fn french() -> Analyzer {
    Analyzer::new(Language::parse("fr").unwrap())
}

/// The excerpt as text, with each match in brackets.
fn shown(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| {
            if segment.matched {
                format!("[{}]", segment.text)
            } else {
                segment.text.clone()
            }
        })
        .collect()
}

#[test]
fn every_workspace_language_is_known() {
    for code in Language::CODES {
        assert!(Language::parse(code).is_some(), "{code}");
    }
    assert!(Language::parse("klingon").is_none());
    assert!(Language::parse("").is_none());
}

#[test]
fn english_words_share_their_stem() {
    let english = english();
    assert_eq!(
        english.index_text("Deploying deployments"),
        english.index_text("deploy deployment")
    );
    assert_eq!(english.index_text("Running, runs."), "run run");
}

#[test]
fn french_words_share_their_stem() {
    let french = french();
    assert_eq!(
        french.index_text("Les déploiements"),
        french.index_text("le déploiement")
    );
    assert_eq!(french.index_text("chevaux"), french.index_text("cheval"));
    assert_eq!(
        french.index_text("continuellement"),
        french.index_text("continuel")
    );
}

#[test]
fn accents_and_case_do_not_matter() {
    let french = french();
    assert_eq!(
        french.index_text("ÉTÉ Déploiement"),
        french.index_text("ete deploiement")
    );
    assert_eq!(french.index_text("cœur"), french.index_text("coeur"));
}

#[test]
fn apostrophes_and_punctuation_split_words() {
    let french = french();
    assert_eq!(
        french.index_text("l'équipe—rapide"),
        french.index_text("l équipe rapide")
    );
}

#[test]
fn a_query_ands_its_terms_and_prefixes_the_last() {
    let query = english().query("deploying Servers").unwrap();
    assert_eq!(query.fts(), r#""deploy" "server"*"#);
}

#[test]
fn a_query_that_ends_with_a_space_has_no_prefix() {
    let query = english().query("deploying servers ").unwrap();
    assert_eq!(query.fts(), r#""deploy" "server""#);
}

#[test]
fn a_query_cannot_inject_fts_syntax() {
    let query = english().query(r#"a" OR title:* NEAR(b) ^c -d"#).unwrap();
    for part in query.fts().split(' ') {
        let inner = part.trim_end_matches('*');
        assert!(inner.starts_with('"') && inner.ends_with('"'), "{part}");
        assert!(inner[1..inner.len() - 1].chars().all(char::is_alphanumeric));
    }
}

#[test]
fn a_query_with_no_word_is_none() {
    assert!(english().query("  ?!* ").is_none());
    assert!(english().query("").is_none());
}

#[test]
fn a_query_keeps_a_bounded_number_of_terms() {
    let long = "word ".repeat(100);
    let query = english().query(&long).unwrap();
    assert_eq!(query.fts().split(' ').count(), cadocs_search::MAX_TERMS);
}

#[test]
fn an_excerpt_marks_the_words_that_matched() {
    let english = english();
    let query = english.query("deploy").unwrap();
    let segments = english.excerpt(&query, "How we deployed it.\nThe deployment ran.");
    assert_eq!(
        shown(&segments),
        "How we [deployed] it. The [deployment] ran."
    );
}

#[test]
fn an_excerpt_marks_a_prefix_and_ignores_accents() {
    let french = french();
    let query = french.query("ete depl").unwrap();
    let segments = french.excerpt(&query, "Cet été, le déploiement");
    assert_eq!(shown(&segments), "Cet [été], le [déploiement]");
}

#[test]
fn an_excerpt_starts_near_the_first_match() {
    let english = english();
    let mut text = "filler ".repeat(200);
    text.push_str("the needle sits here");
    text.push_str(&" tail".repeat(200));
    let query = english.query("needle ").unwrap();
    let shown = shown(&english.excerpt(&query, &text));
    assert!(shown.starts_with('…'), "{shown}");
    assert!(shown.ends_with('…'), "{shown}");
    assert!(shown.contains("the [needle] sits here"), "{shown}");
    assert!(shown.chars().count() < 300, "{shown}");
}

#[test]
fn an_excerpt_with_no_match_is_the_start_of_the_text() {
    let english = english();
    let query = english.query("title").unwrap();
    let text = "A body that never says it. ".repeat(30);
    let shown = shown(&english.excerpt(&query, &text));
    assert!(shown.starts_with("A body that never says it."), "{shown}");
    assert!(shown.ends_with('…'), "{shown}");
    assert!(!shown.contains('['), "{shown}");
}

#[test]
fn an_empty_text_has_an_empty_excerpt() {
    let english = english();
    let query = english.query("anything").unwrap();
    assert!(english.excerpt(&query, "").is_empty());
}

#[test]
fn a_one_letter_last_word_is_not_a_prefix() {
    // `prefix='2 3'` cannot serve it: it would scan every term.
    assert_eq!(english().query("plan b").unwrap().fts(), r#""plan" "b""#);
}

#[test]
fn an_excerpt_stops_looking_for_a_match_in_a_long_text() {
    let english = english();
    let query = english.query("needle ").unwrap();
    let mut text = "filler ".repeat(cadocs_search::EXCERPT_SCAN_WORDS + 10);
    text.push_str("needle");
    let shown = shown(&english.excerpt(&query, &text));
    assert!(shown.starts_with("filler filler"), "{shown}");
    assert!(!shown.contains('['), "{shown}");
}
