use std::borrow::Cow;

use regex::{Captures, Regex};

pub fn escape(text: &str) -> Cow<str> {
    lazy_static::lazy_static! {
        static ref RE_META: Regex = Regex::new(r"(https?://\S+)|([_`*~|])").unwrap();
    }

    RE_META.replace_all(text, |caps: &Captures| {
        if let Some(m) = caps.get(1) {
            format!("<{}>", m.as_str())
        } else if let Some(m) = caps.get(2) {
            format!("\\{}", m.as_str())
        } else {
            unreachable!()
        }
    })
}

pub fn escape_code_block(text: &str) -> String {
    text.replace("```", "`\\``")
}

pub fn suppress_embeds(text: &str) -> Cow<str> {
    lazy_static::lazy_static! {
        static ref RE_URL: Regex = Regex::new(r"(https?://\S+)").unwrap();
    }

    RE_URL.replace_all(text, "<$1>")
}
