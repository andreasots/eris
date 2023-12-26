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
    assert!(max_codepoints > 2 * MARKER_LEN);

    let mut ret = vec![];
    let mut next = String::new();
    let mut next_len = 0;

    let mut iter = msg.split_sentence_bounds().peekable();

    while let Some(mut sentence) = iter.next() {
        let mut sentence_len = sentence.chars().count();
        let has_trailing = iter.peek().is_some();
        let trailing_marker_len = if has_trailing { MARKER_LEN } else { 0 };

        if next_len + sentence_len + trailing_marker_len <= max_codepoints {
            // Sentence fits in this part.
            next.push_str(sentence);
            next_len += sentence_len;
        } else if MARKER_LEN + sentence_len + trailing_marker_len < max_codepoints {
            // Sentence fits in its own part.
            next.push_str(MARKER);
            ret.push(next);

            next = String::from(MARKER) + sentence;
            next_len = MARKER_LEN + sentence_len;
        } else {
            // Sentence needs to be split to fit in a part.
            loop {
                let remaining_len = max_codepoints - next_len - MARKER_LEN;

                if sentence_len <= remaining_len {
                    next.push_str(sentence);
                    next_len += sentence_len;
                    break;
                }

                let mut split_point = 0;
                let mut first_split_len = 0;
                for segment in sentence.split_word_bounds() {
                    let segment_len = segment.chars().count();
                    if first_split_len + segment_len < remaining_len {
                        first_split_len += segment_len;
                        split_point += segment.len();
                    } else {
                        break;
                    }
                }

                if split_point == 0 && (next.is_empty() || next == MARKER) {
                    // Empty part but the first word is too long to fit.
                    split_point =
                        sentence.char_indices().nth(remaining_len).map(|(i, _)| i).unwrap();
                    first_split_len = remaining_len;
                }

                next.push_str(&sentence[..split_point]);
                next.push_str(MARKER);
                ret.push(next);

                next = String::from(MARKER);
                next_len = MARKER_LEN;
                sentence = &sentence[split_point..];
                sentence_len -= first_split_len;
            }
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

#[cfg(test)]
mod split_to_parts {
    use super::split_to_parts;

    #[test]
    fn single() {
        const MSG: &str = "According to all known laws of aviation, there is no way that a bee should be able to fly.";
        assert_eq!(split_to_parts(MSG, 128), vec![String::from(MSG)]);
    }

    #[test]
    fn multi() {
        assert_eq!(
            split_to_parts(
                concat!(
                    "According to all known laws of aviation, there is no way that a bee should be able to fly. ",
                    "Its wings are too small to get its fat little body off the ground. ",
                    "The bee, of course, flies anyway because bees don't care what humans think is impossible.",
                ),
                128,
            ), vec![
                "According to all known laws of aviation, there is no way that a bee should be able to fly. […]".to_string(),
                "[…]Its wings are too small to get its fat little body off the ground. […]".to_string(),
                "[…]The bee, of course, flies anyway because bees don't care what humans think is impossible.".to_string(),
            ],
        );
    }

    #[test]
    fn single_megasentence() {
        assert_eq!(
            split_to_parts(
                concat!(
                    "according to all known laws of aviation there is no way that a bee should be able to fly ",
                    "its wings are too small to get its fat little body off the ground ",
                    "the bee of course flies anyway because bees don't care what humans think is impossible",
                ),
                64,
            ), vec![
                "according to all known laws of aviation there is no way that[…]".to_string(),
                "[…] a bee should be able to fly its wings are too small to […]".to_string(),
                "[…]get its fat little body off the ground the bee of course […]".to_string(),
                "[…]flies anyway because bees don't care what humans think is[…]".to_string(),
                "[…] impossible".to_string()
            ],
        );
    }

    #[test]
    fn mixed_megasentence() {
        assert_eq!(
            split_to_parts(
                concat!(
                    "According to all known laws of aviation, there is no way that a bee should be able to fly: ",
                    "its wings are too small to get its fat little body off the ground. ",
                    "The bee, of course, flies anyway because bees don't care what humans think is impossible.",
                ),
                95,
            ), vec![
                "According to all known laws of aviation, there is no way that a bee should be able to fly: […]".to_string(),
                "[…]its wings are too small to get its fat little body off the ground. […]".to_string(),
                "[…]The bee, of course, flies anyway because bees don't care what humans think is impossible.".to_string(),
            ],
        );
    }

    #[test]
    fn starts_with_a_too_long_word() {
        assert_eq!(
            split_to_parts(
                "Accordingtoallknownlawsofaviationthereisnowaythatabeeshould be able to fly.",
                32,
            ),
            vec![
                "Accordingtoallknownlawsofavia[…]".to_string(),
                "[…]tionthereisnowaythatabeesh[…]".to_string(),
                "[…]ould be able to fly.".to_string(),
            ],
        );
    }

    #[test]
    fn too_long_word() {
        assert_eq!(
            split_to_parts(
                "According to all known lawsofaviationthereisnowaythat a bee should be able to fly.",
                32,
            ), vec![
                "According to all known […]".to_string(),
                "[…]lawsofaviationthereisnoway[…]".to_string(),
                "[…]that a bee should be able[…]".to_string(),
                "[…] to fly.".to_string(),
            ],
        );
    }
}
