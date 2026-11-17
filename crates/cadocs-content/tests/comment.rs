use cadocs_content::comment::{Piece, mentioned_users, pieces};

#[test]
fn a_body_reads_as_text_and_mentions() {
    assert_eq!(
        pieces("Ask <user:u-1> about <page:p_2>.\n<b>ok</b>"),
        [
            Piece::Text("Ask "),
            Piece::User("u-1"),
            Piece::Text(" about "),
            Piece::Page("p_2"),
            Piece::Text(".\n<b>ok</b>"),
        ]
    );
}

#[test]
fn malformed_tokens_stay_text() {
    let long = format!("<user:{}>", "a".repeat(65));
    for body in [
        "<user:>",
        "<user:a b>",
        "<user:a",
        "<team:a>",
        "<USER:a>",
        "user:a>",
        long.as_str(),
    ] {
        assert_eq!(pieces(body), [Piece::Text(body)], "{body}");
    }
    assert_eq!(
        pieces("<<user:a>>"),
        [Piece::Text("<"), Piece::User("a"), Piece::Text(">")]
    );
}

#[test]
fn mentioned_users_lists_each_user_once_without_pages() {
    assert_eq!(
        mentioned_users("<user:b> <page:x> <user:a> <user:b>"),
        ["b", "a"]
    );
    assert!(mentioned_users("plain").is_empty());
}
