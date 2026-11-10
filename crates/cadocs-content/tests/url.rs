use cadocs_content::url::allowed;

#[test]
fn web_and_mail_addresses_pass() {
    for url in [
        "https://example.com/a?b#c",
        "http://example.com",
        "HTTPS://EXAMPLE.COM",
        "mailto:someone@example.com",
        "tel:+33123456789",
        "/?page=0190",
        "#section",
        "relative/path",
        "?q=1",
        "  https://example.com  ",
    ] {
        assert!(allowed(url).is_some(), "{url} should pass");
    }
    assert_eq!(
        allowed("  https://example.com  "),
        Some("https://example.com")
    );
}

#[test]
fn script_and_data_schemes_are_refused() {
    for url in [
        "javascript:alert(1)",
        "JavaScript:alert(1)",
        " javascript:alert(1)",
        "java\tscript:alert(1)",
        "java\nscript:alert(1)",
        "vbscript:msgbox(1)",
        "data:text/html,<script>alert(1)</script>",
        "file:///etc/passwd",
        "\u{0}javascript:alert(1)",
        "",
        "   ",
    ] {
        assert!(allowed(url).is_none(), "{url:?} should be refused");
    }
}

#[test]
fn a_colon_after_a_path_is_not_a_scheme() {
    assert!(allowed("/a:b").is_some());
    assert!(allowed("./javascript:alert(1)").is_some());
    assert!(allowed("a/b:c").is_some());
    assert!(allowed("?x=javascript:1").is_some());
}
