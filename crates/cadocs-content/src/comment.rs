//! Comment bodies: plain text, never HTML nor Markdown, where `<user:ID>` mentions a user and
//! `<page:ID>` a page. An id is 1 to 64 ASCII letters, digits, `-` or `_`. Anything else, a
//! malformed token included, is text. Clients show the text as text and look up each name or
//! title with the reader's rights, so the body holds ids only. `web/src/lib/mentions.ts` reads
//! the same grammar.

/// A run of a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    User(&'a str),
    Page(&'a str),
}

/// The longest id a token takes.
const MAX_ID: usize = 64;

/// The token at the start of `rest`, with its length, if one is there.
fn token(rest: &str) -> Option<(Piece<'_>, usize)> {
    let (user, after) = if let Some(after) = rest.strip_prefix("<user:") {
        (true, after)
    } else {
        (false, rest.strip_prefix("<page:")?)
    };
    let length = after
        .bytes()
        .take(MAX_ID + 1)
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        .count();
    if length == 0 || length > MAX_ID || after.as_bytes().get(length) != Some(&b'>') {
        return None;
    }
    let id = &after[..length];
    let piece = if user {
        Piece::User(id)
    } else {
        Piece::Page(id)
    };
    Some((piece, rest.len() - after.len() + length + 1))
}

/// The body cut into text and mentions, in order.
pub fn pieces(body: &str) -> Vec<Piece<'_>> {
    let mut found = Vec::new();
    let mut text_start = 0;
    let mut at = 0;
    while let Some(offset) = body[at..].find('<') {
        at += offset;
        match token(&body[at..]) {
            Some((piece, length)) => {
                if text_start < at {
                    found.push(Piece::Text(&body[text_start..at]));
                }
                found.push(piece);
                at += length;
                text_start = at;
            }
            None => at += 1,
        }
    }
    if text_start < body.len() {
        found.push(Piece::Text(&body[text_start..]));
    }
    found
}

/// The users a body mentions, in order, each once.
pub fn mentioned_users(body: &str) -> Vec<String> {
    let mut users: Vec<String> = Vec::new();
    for piece in pieces(body) {
        if let Piece::User(id) = piece
            && !users.iter().any(|user| user == id)
        {
            users.push(id.to_owned());
        }
    }
    users
}
