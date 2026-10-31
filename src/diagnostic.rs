//! Fixed, secret-free messages that may be shown when a command cannot start.
//! Only errors wrapped here (plus the two typed pairing errors) reach the terminal;
//! transport, peer and I/O errors stay silent. `check` has explicit success output.
use crate::pairing::{CodeUnavailable, PairingRejected};
use std::fmt;

#[derive(Debug)]
pub struct Diagnostic(String);

impl Diagnostic {
    /// `message` must never contain keys, prompts, URLs with credentials or file contents.
    pub fn err(message: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self(message.into()))
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Diagnostic {}

/// The line to show for `error`, if its chain holds a message that is safe to print.
pub fn printable(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|cause| {
        if let Some(diagnostic) = cause.downcast_ref::<Diagnostic>() {
            Some(diagnostic.to_string())
        } else if cause.is::<CodeUnavailable>() || cause.is::<PairingRejected>() {
            Some(cause.to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn only_wrapped_and_typed_errors_are_printable() {
        assert_eq!(
            printable(&Diagnostic::err("fixed text")).as_deref(),
            Some("fixed text")
        );
        let wrapped = Err::<(), _>(Diagnostic::err("inner"))
            .context("outer")
            .unwrap_err();
        assert_eq!(printable(&wrapped).as_deref(), Some("inner"));
        assert!(printable(&PairingRejected.into()).is_some());
        assert!(printable(&CodeUnavailable.into()).is_some());
        let io = std::io::Error::other("connection to 192.0.2.1 failed");
        assert!(printable(&io.into()).is_none());
        assert!(printable(&anyhow::anyhow!("anything else")).is_none());
    }
}
