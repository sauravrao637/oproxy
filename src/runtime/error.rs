#[derive(Debug, thiserror::Error)]
pub(crate) enum StartupError {
    #[error("Invalid bind address '{addr}': {source}")]
    InvalidAddr {
        addr: String,
        source: std::net::AddrParseError,
    },
    #[error("Failed to bind listener on {addr}: {source}")]
    BindFailed {
        addr: String,
        source: std::io::Error,
    },
    #[error("Failed to initialise certificate authority: {0}")]
    CaInit(String),
}
