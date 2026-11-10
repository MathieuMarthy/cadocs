//! The addresses links and images may point to. Content is written by other users, and public
//! pages render it, so a script address must never reach an `href` (ADR 0008).

/// The schemes a link may use. An address without a scheme is relative, and passes.
const SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

/// `url` without its surrounding spaces, if a link may point to it. Control characters are
/// refused anywhere, since browsers drop tabs and line breaks inside a scheme
/// (`java\tscript:`). The result still needs escaping as an attribute.
pub fn allowed(url: &str) -> Option<&str> {
    let url = url.trim_matches(|c: char| c.is_ascii_whitespace());
    if url.is_empty() || url.chars().any(char::is_control) {
        return None;
    }
    // A scheme is what comes before the first `:`, if no `/`, `?` or `#` comes first.
    let end = url.find([':', '/', '?', '#']);
    match end {
        Some(end) if url.as_bytes()[end] == b':' => {
            let scheme = &url[..end];
            SCHEMES
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
                .then_some(url)
        }
        _ => Some(url),
    }
}
