use std::borrow::Cow;

const MARKER: &str = "[…]";
const MARKER_LEN: usize = 3;

pub fn shorten(s: &str, max_codepoints: usize) -> Cow<str> {
    assert!(max_codepoints >= MARKER_LEN);

    let mut iter = s.char_indices();
    // The code point at `cut` is the first one that doesn't fit the string
    let cut = iter.nth(max_codepoints - MARKER_LEN).map(|(i, _)| i);
    // Try to get the index of the code point that's replaced by the last character of the cut
    // marker. If it's outside the string then the string is short enough already.
    if iter.nth(MARKER_LEN - 1).is_none() {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(String::from(&s[..cut.unwrap()]) + MARKER)
    }
}

#[test]
fn marker_len() {
    assert_eq!(MARKER.chars().count(), MARKER_LEN);
}

#[test]
fn shorten_proptest() {
    let mut input = String::new();

    for _ in 0..16 {
        input.push('💩');
    }

    for input_len in 0..16 {
        let input = &input[..input.char_indices().nth(input_len).map(|(i, _)| i).unwrap()];

        for max_codepoints in MARKER_LEN..24 {
            let s = shorten(&input, max_codepoints);
            assert_eq!(s.chars().count(), std::cmp::min(input_len, max_codepoints));
        }
    }
}
