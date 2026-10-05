use thiserror::Error;

/// Failure to parse a user- or vendor-supplied value into a domain type.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {what}: {input:?}")]
pub struct ParseError {
    pub what: &'static str,
    pub input: String,
}

impl ParseError {
    #[must_use]
    pub fn new(what: &'static str, input: impl Into<String>) -> Self {
        Self { what, input: input.into() }
    }
}
