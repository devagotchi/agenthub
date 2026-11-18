//! Names the board accepts and the errors it answers with.

use devagotchi_contracts::BridgeError;

/// A board, project, seat or agent name: 1 to 63 of `[a-z0-9-]`, starting with a letter or
/// digit.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase() | bytes[0].is_ascii_digit()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn fail(code: &str, message: impl Into<String>) -> BridgeError {
    BridgeError::new(code, message)
}

pub(crate) fn invalid(message: impl Into<String>) -> BridgeError {
    fail("invalid", message)
}
