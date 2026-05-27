mod api;
mod certs;
mod config;
mod control_plane;
mod core;
mod diff;
mod export;
mod har;
mod middleware;
mod redaction;
mod runtime;
mod security;
mod session;
mod setup;
mod storage;
mod transport;
mod webhooks;

pub(crate) use runtime::AppState;

#[tokio::main]
async fn main() -> Result<(), runtime::StartupError> {
    runtime::run().await
}
