use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgmError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Manifest not found: agm.json not found in current directory")]
    ManifestNotFound,

    #[error("Lock file not found: run `agm install` first")]
    LockNotFound,

    #[error("Package not found in manifest: {0}")]
    PackageNotInManifest(String),

    #[error("Package not found in store: {0}")]
    PackageNotInStore(String),

    #[error("Version conflict: {details}")]
    VersionConflict { details: String },

    #[error("Semver parse error: {0}")]
    Semver(#[from] semver::Error),

    #[error("Invalid commit hash: {0}")]
    InvalidCommitHash(String),

    #[error("Registry error: {0}")]
    Registry(String),

    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlobPattern { pattern: String, reason: String },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AgmError>;
