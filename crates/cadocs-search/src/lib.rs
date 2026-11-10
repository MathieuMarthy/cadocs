//! Search text, stemmed in Rust (ADR 0007): what FTS5 indexes, the `MATCH` expression of a query,
//! and excerpts made of the real words.
//!
//! A word is a run of letters and digits. It is lowercased, its diacritics are folded, then it is
//! stemmed. Folding comes first so that a query typed without accents finds the accented words:
//! both sides then go through the same stemmer with the same letters.

use std::collections::HashSet;
use std::ops::Range;

use rust_stemmers::{Algorithm, Stemmer};

/// Terms kept from a query, so a pasted paragraph cannot make a costly `MATCH`.
pub const MAX_TERMS: usize = 16;
/// Words of a text searched for the first match of its excerpt.
pub const EXCERPT_SCAN_WORDS: usize = 5000;
/// Words of an excerpt before its first match.
const WORDS_BEFORE: usize = 8;
/// Characters an excerpt shows, about.
const EXCERPT_CHARS: usize = 200;
const ELLIPSIS: &str = "…";

/// A stemming language, set per workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language(Algorithm);

impl Language {
    /// The codes a workspace accepts, one per Snowball stemmer.
    pub const CODES: [&'static str; 18] = [
        "ar", "da", "de", "el", "en", "es", "fi", "fr", "hu", "it", "nl", "no", "pt", "ro", "ru",
        "sv", "ta", "tr",
    ];

    pub fn parse(code: &str) -> Option<Self> {
        let algorithm = match code {
            "ar" => Algorithm::Arabic,
            "da" => Algorithm::Danish,
            "de" => Algorithm::German,
            "el" => Algorithm::Greek,
            "en" => Algorithm::English,
            "es" => Algorithm::Spanish,
            "fi" => Algorithm::Finnish,
            "fr" => Algorithm::French,
            "hu" => Algorithm::Hungarian,
            "it" => Algorithm::Italian,
            "nl" => Algorithm::Dutch,
            "no" => Algorithm::Norwegian,
            "pt" => Algorithm::Portuguese,
            "ro" => Algorithm::Romanian,
            "ru" => Algorithm::Russian,
            "sv" => Algorithm::Swedish,
            "ta" => Algorithm::Tamil,
            "tr" => Algorithm::Turkish,
            _ => return None,
        };
        Some(Self(algorithm))
    }
}

/// A piece of an excerpt: `matched` for a word the query found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub matched: bool,
}

/// A parsed query: the terms to find, the last one maybe as a prefix while it is being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    fts: String,
    terms: HashSet<String>,
    prefix: Option<String>,
}

impl Query {
    /// The FTS5 `MATCH` expression. Every term is quoted and holds only letters and digits, so
    /// nothing a user types is read as FTS5 syntax.
    pub fn fts(&self) -> &str {
        &self.fts
    }

    fn matches(&self, term: &str) -> bool {
        self.terms.contains(term)
            || self
                .prefix
                .as_deref()
                .is_some_and(|prefix| term.starts_with(prefix))
    }
}

pub struct Analyzer {
    stemmer: Stemmer,
}

impl Analyzer {
    pub fn new(language: Language) -> Self {
        Self {
            stemmer: Stemmer::create(language.0),
        }
    }

    /// The text FTS5 indexes: every term, separated by a space.
    pub fn index_text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (_, word) in words(text) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&self.term(word));
        }
        out
    }

    /// `None` for a query with no word. The last word is a prefix unless the query ends after it,
    /// with a space or a mark, so results follow what is typed.
    pub fn query(&self, text: &str) -> Option<Query> {
        let found: Vec<(Range<usize>, &str)> = words(text).take(MAX_TERMS).collect();
        let (last_range, _) = found.last()?;
        let typing = last_range.end == text.len();
        let mut terms: Vec<String> = found.iter().map(|(_, word)| self.term(word)).collect();
        // A one-letter prefix would scan every term, as `prefix='2 3'` starts at two.
        let prefix = if typing && found.last()?.1.chars().count() > 1 {
            terms.pop()
        } else {
            None
        };
        let mut fts: Vec<String> = terms.iter().map(|term| format!("\"{term}\"")).collect();
        if let Some(prefix) = &prefix {
            fts.push(format!("\"{prefix}\"*"));
        }
        Some(Query {
            fts: fts.join(" "),
            terms: terms.into_iter().collect(),
            prefix,
        })
    }

    /// About `EXCERPT_CHARS` of `text` around its first match, or from its start when only the
    /// title matched. Line breaks and runs of spaces become one space, and an ellipsis marks a cut.
    pub fn excerpt(&self, query: &Query, text: &str) -> Vec<Segment> {
        // Two lazy passes and no list of every word, so a long page costs about its window. A
        // match past the first `EXCERPT_SCAN_WORDS` words is not looked for.
        let matches = |word: &str| query.matches(&self.term(word));
        let first = words(text)
            .take(EXCERPT_SCAN_WORDS)
            .position(|(_, word)| matches(word))
            .unwrap_or(0);
        let start = first.saturating_sub(WORDS_BEFORE);
        let mut rest = words(text).skip(start);
        let Some(head) = rest.next() else {
            return Vec::new();
        };
        let from = head.0.start;
        let mut window = vec![head];
        let mut after = None;
        for word in rest {
            // A single word longer than the limit is still shown, whole.
            if word.0.end - from > EXCERPT_CHARS {
                after = Some(word.0.start);
                break;
            }
            window.push(word);
        }
        let mut out = Excerpt::default();
        if start > 0 {
            out.push(ELLIPSIS, false);
        }
        let mut cursor = from;
        for (range, word) in window {
            out.push(&text[cursor..range.start], false);
            out.push(word, matches(word));
            cursor = range.end;
        }
        match after {
            Some(next) => {
                out.push(&text[cursor..next], false);
                out.push(ELLIPSIS, false);
            }
            None => out.push(&text[cursor..], false),
        }
        out.segments
    }

    fn term(&self, word: &str) -> String {
        let mut folded = String::with_capacity(word.len());
        for c in word.chars().flat_map(char::to_lowercase) {
            fold(c, &mut folded);
        }
        self.stemmer.stem(&folded).into_owned()
    }
}

#[derive(Default)]
struct Excerpt {
    segments: Vec<Segment>,
}

impl Excerpt {
    /// Spaces collapse to one, and a piece joins the last one when both match or both do not.
    fn push(&mut self, text: &str, matched: bool) {
        let mut clean = String::with_capacity(text.len());
        let mut space = false;
        for c in text.chars() {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if space && !(clean.is_empty() && self.ends_with_space()) {
                clean.push(' ');
            }
            space = false;
            clean.push(c);
        }
        if space && !self.ends_with_space() {
            clean.push(' ');
        }
        if clean.is_empty() {
            return;
        }
        match self.segments.last_mut() {
            Some(last) if last.matched == matched => last.text.push_str(&clean),
            _ => self.segments.push(Segment {
                text: clean,
                matched,
            }),
        }
    }

    fn ends_with_space(&self) -> bool {
        self.segments
            .last()
            .is_some_and(|last| last.text.ends_with(' '))
    }
}

/// Runs of letters and digits, with their byte range. A combining mark belongs to its word.
fn words(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        let start = loop {
            let (index, c) = chars.next()?;
            if c.is_alphanumeric() {
                break index;
            }
        };
        let mut end = text.len();
        while let Some(&(index, c)) = chars.peek() {
            if !(c.is_alphanumeric() || combining(c)) {
                end = index;
                break;
            }
            chars.next();
        }
        Some((start..end, &text[start..end]))
    })
}

fn combining(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}')
}

/// Latin letters lose their diacritics, as FTS5's `remove_diacritics` would, and ligatures are
/// spelt out. Combining marks go. `c` is already lowercase.
fn fold(c: char, out: &mut String) {
    let base = match c {
        _ if combining(c) => return,
        'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĳ' => "ij",
        'ĵ' => "j",
        'ķ' | 'ĸ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => {
            out.push(c);
            return;
        }
    };
    out.push_str(base);
}
