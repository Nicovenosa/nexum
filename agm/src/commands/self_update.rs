use crate::error::{AgmError, Result};

pub const DISABLED_EXIT_CODE: i32 = 69;

/// Explains that AGM has no integrated updater and never executes downloaded
/// installation scripts.
pub fn disabled_message() -> &'static str {
    "La actualizacion integrada todavia no esta disponible. Usa solamente el installer local canonico de Nexum."
}

pub fn execute(_force: bool) -> Result<()> {
    Err(AgmError::Other(disabled_message().into()))
}
