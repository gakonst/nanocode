use std::borrow::Cow;

use crate::regex;

pub(crate) struct Regex {
    source: String,
    compiled: regex::Regex,
}

impl Regex {
    pub(crate) fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            source: pattern.to_owned(),
            compiled: regex::Regex::new(pattern)?,
        })
    }

    pub(crate) fn is_match(&self, text: &str) -> bool {
        self.compiled.is_match(text)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.source
    }
}

pub(crate) fn convert(pattern: &str) -> Result<Cow<'_, str>, Box<dyn std::error::Error>> {
    Regex::new(pattern)?;
    Ok(Cow::Borrowed(pattern))
}
