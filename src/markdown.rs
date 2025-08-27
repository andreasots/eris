use std::borrow::Cow;
use std::sync::OnceLock;

use regex::{Captures, Regex};

pub fn escape(text: &str) -> Cow<'_, str> {
    static RE_META: OnceLock<Regex> = OnceLock::new();
    let re_meta = RE_META.get_or_init(|| Regex::new(r"(https?://\S+)|([_`*~|])").unwrap());

    re_meta.replace_all(text, |caps: &Captures| {
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

pub fn suppress_embeds(text: &str) -> Cow<'_, str> {
    static RE_URL: OnceLock<Regex> = OnceLock::new();
    let re_url = RE_URL.get_or_init(|| Regex::new(r"(https?://\S+)").unwrap());

    re_url.replace_all(text, "<$1>")
}
