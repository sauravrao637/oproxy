use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::core::engine::ProxyEngine;
use crate::transport::lifecycle::ConnectionSupervisor;

#[derive(Clone)]
pub struct TransportContext {
    pub session_manager: crate::session::SharedSessionManager,
    pub engine: Arc<ProxyEngine>,
    pub dns_overrides: Arc<RwLock<HashMap<String, String>>>,
    pub connections: ConnectionSupervisor,
    pub inspect_ws_frames: bool,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
}
