//! Local-only update status for the Nexum runtime.

pub const UNAVAILABLE_EXIT_CODE: i32 = 69;

/// Explains that this runtime has no integrated updater and never executes
/// downloaded installation scripts.
pub fn unavailable_message() -> &'static str {
    "La actualizacion integrada todavia no esta disponible. Usa solamente el installer local canonico de Nexum."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_is_explicitly_unavailable() {
        assert_eq!(UNAVAILABLE_EXIT_CODE, 69);
        assert!(unavailable_message().contains("no esta disponible"));
    }
}
