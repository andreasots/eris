use std::borrow::Cow;

use regex::{Captures, Regex};

pub fn escape(text: &str) -> Cow<str> {
    lazy_static::lazy_static! {
        static ref RE_META: Regex = Regex::new(r"(https?://\S+)|([_`*~|])").unwrap();
    }

    RE_META.replace_all(text, |caps: &Captures| {
        if let Some(m) = caps.get(1) {
            return m.as_str().to_string();
        } else if let Some(m) = caps.get(2) {
            return format!("\\{}", m.as_str());
        } else {
            unreachable!()
        }
    })
}

pub fn escape_code_block(text: &str) -> String {
    text.replace("```", "`\\``")
}
