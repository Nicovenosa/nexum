use anyhow::{bail, Result};

/// Remote sync is opt-in: a server and an explicit human confirmation are
/// both required before the caller can initiate any connection.
pub fn require_explicit_server<'a>(server: Option<&'a str>, confirmed: bool) -> Result<&'a str> {
    let Some(server) = server.filter(|server| !server.trim().is_empty()) else {
        bail!("Remote sync is disabled by default. Provide --server and --confirm-remote to continue.");
    };

    if !confirmed {
        bail!("Remote sync requires explicit human consent via --confirm-remote.");
    }

    Ok(server)
}
