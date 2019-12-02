pub fn truncate(s: &str, max_codepoints: usize) -> (&str, bool) {
    if let Some((i, _)) = s.char_indices().nth(max_codepoints) {
        (&s[..i], true)
    } else {
        (s, false)
    }
}

#[test]
fn truncate_short() {
    assert_eq!(truncate("abc", 1024), ("abc", false));
}

#[test]
fn truncate_exact() {
    assert_eq!(truncate("abc", 3), ("abc", false));
}

#[test]
fn truncate_long() {
    assert_eq!(truncate("abc", 2), ("ab", true));
}
