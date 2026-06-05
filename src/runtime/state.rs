use std::sync::Arc;

use tokio::sync::RwLock;

use crate::api::ApiHandler;
use crate::control_plane;
use crate::core::engine::ProxyEngine;
use crate::middleware::chain::MiddlewareChain;
use crate::middleware::plugins::access_control::{AccessControlMiddleware, SharedAccessRules};
use crate::middleware::plugins::breakpoints::{BreakpointManager, BreakpointMiddleware};
use crate::middleware::plugins::capture_filter::{CaptureFilterConfig, CaptureFilterMiddleware};
use crate::middleware::plugins::dns_override::DnsOverrideMiddleware;
use crate::middleware::plugins::graphql_inspector::GraphQLInspectorMiddleware;
use crate::middleware::plugins::grpc_inspector::GrpcInspectorMiddleware;
use crate::middleware::plugins::inspection::InspectionMiddleware;
use crate::middleware::plugins::jwt_inspector::JwtInspectorMiddleware;
use crate::middleware::plugins::lua_engine::LuaEngineMiddleware;
use crate::middleware::plugins::map_local::{MapLocalMiddleware, SharedMapLocalRules};
use crate::middleware::plugins::map_remote::{MapRemoteMiddleware, SharedMapRemoteRules};
use crate::middleware::plugins::mock::MockMiddleware;
use crate::middleware::plugins::routing::{ThrottlingConfig, ThrottlingMiddleware};
use crate::middleware::plugins::rules::{SharedRuleSets, UnifiedRewriteMiddleware};
use crate::storage;

use super::StartupError;

// Shared state threaded through every axum handler and the proxy engine.
pub(crate) struct AppState {
    pub(crate) proxy_engine: Arc<ProxyEngine>,
    pub(crate) middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub(crate) throttling_config: Arc<RwLock<ThrottlingConfig>>,
    pub(crate) dns_overrides: Arc<RwLock<std::collections::HashMap<String, String>>>,
    pub(crate) capture_filter: Arc<RwLock<CaptureFilterConfig>>,
    pub(crate) session_manager: crate::session::SharedSessionManager,
    pub(crate) api_handler: Arc<ApiHandler>,
    pub(crate) storage_path: std::path::PathBuf,
    pub(crate) started_at: std::time::Instant,
    pub(crate) endpoint_metrics: crate::control_plane::SharedEndpointMetrics,
    pub(crate) assistant: crate::control_plane::SharedAssistantState,
    pub(crate) workspace: crate::control_plane::SharedWorkspaceState,
    pub(crate) config: crate::config::Config,
    pub(crate) webhooks: crate::webhooks::SharedWebhooks,
    pub(crate) mock_rules: crate::middleware::plugins::mock::SharedMockRules,
    pub(crate) lua_scripts: crate::middleware::plugins::lua_engine::SharedLuaScripts,
    /// Live rule-set list — shared between the middleware and the control-plane API.
    pub(crate) rule_sets: SharedRuleSets,
    pub(crate) map_local_rules: SharedMapLocalRules,
    pub(crate) map_remote_rules: SharedMapRemoteRules,
    pub(crate) access_rules: SharedAccessRules,
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

    let throttling_config = Arc::new(RwLock::new(storage::load_throttle(&storage_path)));
    let dns_overrides = Arc::new(RwLock::new(storage::load_dns_overrides(&storage_path)));
    let capture_filter = Arc::new(RwLock::new(storage::load_capture_filter(&storage_path)));
    let access_rules = Arc::new(RwLock::new(storage::load_access_rules(&storage_path)));

    // Unified rule sets (replaces rewrite + header_map + modification middlewares).
    let rule_sets = Arc::new(RwLock::new(storage::load_rule_sets(&storage_path)));
    let rewrite_mw = Arc::new({
        let mut mw = UnifiedRewriteMiddleware::new(vec![]);
        mw.rules = rule_sets.clone();
        mw
    });

    // Map Remote: Location-based upstream routing (replaces crude host→URL routing table).
    let map_remote_rules = Arc::new(RwLock::new(storage::load_map_remote_rules(&storage_path)));
    let map_remote_mw = Arc::new({
        let mut mw = MapRemoteMiddleware::new(vec![]);
        mw.rules = map_remote_rules.clone();
        mw
    });

    // Map Local: Location-based file serving (replaces old host→file HashMap).
    let map_local_rules = Arc::new(RwLock::new(storage::load_map_local_rules(&storage_path)));

    let mut chain = MiddlewareChain::new();
    // AccessControl runs first: blocks/allows before any recording or processing.
    chain.add_middleware(Arc::new({
        let mut mw = AccessControlMiddleware::new(vec![]);
        mw.rules = access_rules.clone();
        mw
    }));
    // CaptureFilter: injects skip-recording flag for filtered hosts.
    chain.add_middleware(Arc::new(CaptureFilterMiddleware::new(
        capture_filter.clone(),
    )));
    // DNS override must run before MapRemote so the host rewrite is visible to it.
    chain.add_middleware(Arc::new(DnsOverrideMiddleware {
        overrides: dns_overrides.clone(),
    }));
    // MapRemote: sets ctx.destination so the engine forwards to the right upstream.
    chain.add_middleware(map_remote_mw);
    chain.add_middleware(Arc::new(ThrottlingMiddleware {
        config: throttling_config.clone(),
    }));

    // Unified rewrite engine: runs early (before Breakpoint) so rewritten requests
    // can still hit breakpoints.
    chain.add_middleware(rewrite_mw);

    let breakpoint_manager = Arc::new(BreakpointManager::new());
    for rule in storage::load_breakpoints(&storage_path) {
        breakpoint_manager.add_rule(rule).await;
    }
    chain.add_middleware(Arc::new(BreakpointMiddleware::new(
        breakpoint_manager.clone(),
    )));
    // Inspector plugins run BEFORE InspectionMiddleware so they can set inspector data
    // that InspectionMiddleware reads on the same on_request pass.
    chain.add_middleware(Arc::new(JwtInspectorMiddleware));
    chain.add_middleware(Arc::new(GraphQLInspectorMiddleware));
    chain.add_middleware(Arc::new(GrpcInspectorMiddleware));
    chain.add_middleware(Arc::new(InspectionMiddleware::new(session_manager.clone())));
    // MapLocal, Mock and Lua come after InspectionMiddleware so the request is
    // recorded before they short-circuit it (StopAndReturn). The session captures
    // the original request.

    let middleware_chain = Arc::new(RwLock::new(chain));

    // CA is always initialised so the cert is downloadable regardless of mitm_enabled.
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
        config.port,
        config.bind_host.clone(),
        config.timeout_secs,
        effective_max_body,
        config.pool_max_idle_per_host,
        config.pool_idle_timeout_secs,
        upstream_proxy
            .as_deref()
            .or(config.upstream_proxy.as_deref()),
    ));
    proxy_engine
        .set_short_circuit_session_manager(session_manager.clone())
        .await;

    let api_handler = Arc::new(ApiHandler::new(
        session_manager.clone(),
        breakpoint_manager.clone(),
        crate::security::AdminEgressPolicy::from_config(config),
    ));

    let webhooks_shared = {
        let hooks = storage::load_webhooks(&storage_path);
        let shared = Arc::new(tokio::sync::RwLock::new(hooks));
        let dispatcher = crate::webhooks::WebhookDispatcher::new(
            shared.clone(),
            crate::security::AdminEgressPolicy::from_config(config),
        )
        .map_err(|e| StartupError::ServiceInit(format!("webhook dispatcher: {e}")))?;
        dispatcher.spawn(session_manager.subscribe(), session_manager.clone());
        shared
    };
    let mock_rules_shared = Arc::new(tokio::sync::RwLock::new(storage::load_mock_rules(
        &storage_path,
    )));
    let lua_scripts_shared = Arc::new(tokio::sync::RwLock::new(storage::load_lua_scripts(
        &storage_path,
    )));

    // Wire MapLocal, Mock and Lua into the middleware chain now that their shared state is ready.
    {
        let mut chain = middleware_chain.write().await;
        chain.add_middleware(Arc::new({
            let mw = MapLocalMiddleware::with_base_path(
                vec![],
                config.map_local_base_path.clone(),
            );
            let mut mw = mw;
            mw.rules = map_local_rules.clone();
            mw
        }));
        chain.add_middleware(Arc::new(MockMiddleware::new(mock_rules_shared.clone())));
        chain.add_middleware(Arc::new(LuaEngineMiddleware::new(
            lua_scripts_shared.clone(),
        )));
        // A second idempotent inspection pass records responses from short-circuit
        // middlewares above (Map Local, Mock, Lua) that stop request execution after
        // the primary inspection pass has recorded the request.
        chain.add_middleware(Arc::new(InspectionMiddleware::new_response_pass(
            session_manager.clone(),
        )));
    }

    let state = Arc::new(AppState {
        proxy_engine,
        middleware_chain,
        throttling_config,
        dns_overrides,
        capture_filter,
        session_manager,
        api_handler,
        storage_path,
        started_at: std::time::Instant::now(),
        endpoint_metrics: control_plane::new_endpoint_metrics(),
        assistant: control_plane::new_assistant_state(),
        workspace: control_plane::new_workspace_state(),
        config: config.clone(),
        webhooks: webhooks_shared,
        mock_rules: mock_rules_shared,
        lua_scripts: lua_scripts_shared,
        rule_sets,
        map_local_rules,
        map_remote_rules,
        access_rules,
    });

    Ok(RuntimeServices { state, ca })
}
