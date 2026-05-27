use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::api::ApiHandler;
use crate::control_plane;
use crate::core::engine::ProxyEngine;
use crate::middleware::chain::MiddlewareChain;
use crate::middleware::plugins::breakpoints::{BreakpointManager, BreakpointMiddleware};
use crate::middleware::plugins::capture_filter::{CaptureFilterConfig, CaptureFilterMiddleware};
use crate::middleware::plugins::dns_override::DnsOverrideMiddleware;
use crate::middleware::plugins::graphql_inspector::GraphQLInspectorMiddleware;
use crate::middleware::plugins::grpc_inspector::GrpcInspectorMiddleware;
use crate::middleware::plugins::header_map::HeaderMapMiddleware;
use crate::middleware::plugins::inspection::InspectionMiddleware;
use crate::middleware::plugins::jwt_inspector::JwtInspectorMiddleware;
use crate::middleware::plugins::lua_engine::LuaEngineMiddleware;
use crate::middleware::plugins::mock::MockMiddleware;
use crate::middleware::plugins::modification::ModificationMiddleware;
use crate::middleware::plugins::rewrite::RewriteMiddleware;
use crate::middleware::plugins::routing::{
    RoutingMiddleware, ThrottlingConfig, ThrottlingMiddleware,
};
use crate::storage;

use super::StartupError;

// Shared state threaded through every axum handler and the proxy engine.
pub(crate) struct AppState {
    pub(crate) proxy_engine: Arc<ProxyEngine>,
    pub(crate) middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub(crate) routing_table: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) throttling_config: Arc<RwLock<ThrottlingConfig>>,
    pub(crate) dns_overrides: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) map_local: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) capture_filter: Arc<RwLock<CaptureFilterConfig>>,
    pub(crate) session_manager: crate::session::SharedSessionManager,
    pub(crate) api_handler: Arc<ApiHandler>,
    pub(crate) storage_path: std::path::PathBuf,
    pub(crate) started_at: std::time::Instant,
    pub(crate) endpoint_metrics: crate::control_plane::SharedEndpointMetrics,
    pub(crate) config: crate::config::Config,
    pub(crate) webhooks: crate::webhooks::SharedWebhooks,
    pub(crate) mock_rules: crate::middleware::plugins::mock::SharedMockRules,
    pub(crate) lua_scripts: crate::middleware::plugins::lua_engine::SharedLuaScripts,
}

pub(super) struct RuntimeServices {
    pub(super) state: Arc<AppState>,
    pub(super) ca: Arc<crate::certs::CertificateAuthority>,
}

pub(super) async fn build_runtime_services(
    config: &crate::config::Config,
) -> Result<RuntimeServices, StartupError> {
    let session_manager = Arc::new(crate::session::SessionManager::with_body_budget(
        config.max_sessions,
        config.max_retained_body_bytes,
    ));

    let storage_path = config.storage_path.clone();

    let _ = std::fs::create_dir_all(&storage_path);

    let routing_table = Arc::new(RwLock::new(storage::load_routes(&storage_path)));
    let throttling_config = Arc::new(RwLock::new(storage::load_throttle(&storage_path)));
    let dns_overrides = Arc::new(RwLock::new(storage::load_dns_overrides(&storage_path)));
    let initial_rewrites = storage::load_rewrites(&storage_path);

    let capture_filter = Arc::new(RwLock::new(storage::load_capture_filter(&storage_path)));

    let mut chain = MiddlewareChain::new();
    // CaptureFilter runs first: injects skip-recording header for filtered hosts.
    chain.add_middleware(Arc::new(CaptureFilterMiddleware::new(
        capture_filter.clone(),
    )));
    // DNS override must run before RoutingMiddleware so the host rewrite is visible to it.
    chain.add_middleware(Arc::new(DnsOverrideMiddleware {
        overrides: dns_overrides.clone(),
    }));
    let map_local = Arc::new(tokio::sync::RwLock::new(storage::load_map_local(
        &storage_path,
    )));
    let routing_mw = Arc::new({
        let mut mw: RoutingMiddleware = RoutingMiddleware::new(routing_table.clone());
        mw.map_local = map_local.clone();
        mw
    });
    chain.add_middleware(routing_mw);
    chain.add_middleware(Arc::new(ThrottlingMiddleware {
        config: throttling_config.clone(),
    }));

    let rewrite_middleware = Arc::new(RewriteMiddleware::new(initial_rewrites));
    chain.add_middleware(rewrite_middleware.clone());

    let header_map_middleware = Arc::new(HeaderMapMiddleware::new(storage::load_header_maps(
        &storage_path,
    )));
    chain.add_middleware(header_map_middleware.clone());

    let breakpoint_manager = Arc::new(BreakpointManager::new());
    for rule in storage::load_breakpoints(&storage_path) {
        breakpoint_manager.add_rule(rule).await;
    }
    chain.add_middleware(Arc::new(BreakpointMiddleware::new(
        breakpoint_manager.clone(),
    )));
    // Inspector plugins run BEFORE InspectionMiddleware so they can set x-oproxy-jwt /
    // x-oproxy-graphql / x-oproxy-grpc headers that InspectionMiddleware reads on the
    // same on_request pass and stores into the session's inspector_data.
    chain.add_middleware(Arc::new(JwtInspectorMiddleware));
    chain.add_middleware(Arc::new(GraphQLInspectorMiddleware));
    chain.add_middleware(Arc::new(GrpcInspectorMiddleware));
    chain.add_middleware(Arc::new(InspectionMiddleware::new(session_manager.clone())));
    let modification_middleware = Arc::new(ModificationMiddleware::new(
        storage::load_modifications(&storage_path),
    ));
    chain.add_middleware(modification_middleware.clone());
    // Mock and Lua come after InspectionMiddleware so the request is recorded before
    // they short-circuit it (StopAndReturn). The session captures the original request.

    let middleware_chain = Arc::new(RwLock::new(chain));

    // CA is always initialised so the cert is downloadable regardless of mitm_enabled.
    // mitm_enabled only controls CONNECT interception.
    let ca = Arc::new(
        crate::certs::CertificateAuthority::new(&config.mitm.root_ca_path)
            .await
            .map_err(|e| StartupError::CaInit(e.to_string()))?,
    );

    let hot_cfg = storage::load_hot_config(&storage_path);
    let effective_max_body = hot_cfg.max_body_bytes.unwrap_or(config.max_body_bytes);
    let upstream_proxy = storage::load_upstream_proxy(&storage_path);
    let proxy_engine = Arc::new(ProxyEngine::new(
        middleware_chain.clone(),
        Some(ca.clone()),
        config.mitm.enabled,
        config.timeout_secs,
        effective_max_body,
        config.pool_max_idle_per_host,
        config.pool_idle_timeout_secs,
        upstream_proxy
            .as_deref()
            .or(config.upstream_proxy.as_deref()),
    ));

    let api_handler = Arc::new(ApiHandler::new(
        session_manager.clone(),
        rewrite_middleware.clone(),
        breakpoint_manager.clone(),
        header_map_middleware.clone(),
        modification_middleware.clone(),
        crate::security::AdminEgressPolicy::from_config(config),
    ));

    let webhooks_shared = {
        let hooks = storage::load_webhooks(&storage_path);
        let shared = Arc::new(tokio::sync::RwLock::new(hooks));
        let dispatcher = crate::webhooks::WebhookDispatcher::new(
            shared.clone(),
            crate::security::AdminEgressPolicy::from_config(config),
        );
        dispatcher.spawn(session_manager.subscribe(), session_manager.clone());
        shared
    };
    let mock_rules_shared = Arc::new(tokio::sync::RwLock::new(storage::load_mock_rules(
        &storage_path,
    )));
    let lua_scripts_shared = Arc::new(tokio::sync::RwLock::new(storage::load_lua_scripts(
        &storage_path,
    )));

    // Wire Mock and Lua into the middleware chain now that their shared state is ready.
    {
        let mut chain = middleware_chain.write().await;
        chain.add_middleware(Arc::new(MockMiddleware::new(mock_rules_shared.clone())));
        chain.add_middleware(Arc::new(LuaEngineMiddleware::new(
            lua_scripts_shared.clone(),
        )));
    }

    let state = Arc::new(AppState {
        proxy_engine,
        middleware_chain,
        routing_table,
        throttling_config,
        dns_overrides,
        map_local,
        capture_filter,
        session_manager,
        api_handler,
        storage_path,
        started_at: std::time::Instant::now(),
        endpoint_metrics: control_plane::new_endpoint_metrics(),
        config: config.clone(),
        webhooks: webhooks_shared,
        mock_rules: mock_rules_shared,
        lua_scripts: lua_scripts_shared,
    });

    Ok(RuntimeServices { state, ca })
}
