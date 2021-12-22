use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

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

pub fn split_to_parts(msg: &str, max_codepoints: usize) -> Vec<String> {
    assert!(max_codepoints >= MARKER_LEN);

    let mut ret = vec![];
    let mut next = String::new();
    let mut next_len = 0;
    let mut iter = msg
        .split_word_bounds()
        .flat_map(|mut segment| {
            let mut subsegments = vec![];
            let max_subsegment_length = (max_codepoints - 2 * MARKER_LEN) / 8;
            while let Some((off, c)) = segment.char_indices().take(max_subsegment_length).last() {
                // `off` points to the start of `c`, we want to cut at the end.
                let end = off + c.len_utf8();
                subsegments.push(&segment[..end]);
                segment = &segment[end..];
            }
            subsegments.into_iter()
        })
        .peekable();
    while let Some(segment) = iter.next() {
        let segment_len = segment.chars().count();
        let has_trailing = iter.peek().is_some();
        let max_part_len = max_codepoints - has_trailing.then(|| MARKER_LEN).unwrap_or(0);
        if next_len + segment_len <= max_part_len {
            next.push_str(segment);
            next_len += segment_len;
        } else {
            if has_trailing {
                next.push_str(MARKER);
            }
            ret.push(next);
            next = String::from(MARKER) + segment;
            next_len = MARKER_LEN + segment_len;
        }
    }
    if next_len > 0 {
        ret.push(next);
    }
    ret
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
