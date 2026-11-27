//! The catalogue of mail (step 62): every text in English and French, and the browser's choice.

use cadocs_mail::{Lang, Text};

/// The names in braces of a template, in order.
fn placeholders(template: &str) -> Vec<&str> {
    template
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
        .collect()
}

#[test]
fn every_text_is_translated_with_the_same_placeholders() {
    for &text in Text::ALL {
        let (en, fr) = (text.get(Lang::En), text.get(Lang::Fr));
        assert!(!fr.trim().is_empty(), "{text:?} has no French");
        assert_ne!(en, fr, "{text:?} is left in English");
        let (mut want, mut got) = (placeholders(en), placeholders(fr));
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(want, got, "{text:?} has other placeholders in French");
    }
}

#[test]
fn a_filled_value_is_not_read_as_a_placeholder() {
    let filled = Text::MentionLine.fill(Lang::En, &[("who", "{title}"), ("title", "Plans")]);
    assert_eq!(filled, "{title} mentioned you in Plans\n");
    // A name with no value stays as it is.
    assert_eq!(Text::Hello.fill(Lang::Fr, &[]), "Bonjour {name},\n\n");
}

#[test]
fn a_language_code_falls_back_to_english() {
    assert_eq!(Lang::of(Some("fr")), Lang::Fr);
    assert_eq!(Lang::of(Some("de")), Lang::En);
    assert_eq!(Lang::of(None), Lang::En);
    assert_eq!(Lang::Fr.code(), "fr");
}

#[test]
fn the_browser_language_is_the_first_mail_has_by_weight() {
    let cases = [
        ("fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7", Some(Lang::Fr)),
        ("de-DE,de;q=0.9,en;q=0.5,FR;q=0.7", Some(Lang::Fr)),
        ("en-GB, fr;q=0.5", Some(Lang::En)),
        ("fr;q=0, en;q=0.1", Some(Lang::En)),
        ("fr;q=nonsense", None),
        ("de, *;q=0.5", None),
        ("", None),
    ];
    for (header, want) in cases {
        assert_eq!(Lang::from_accept_language(header), want, "{header}");
    }
}
