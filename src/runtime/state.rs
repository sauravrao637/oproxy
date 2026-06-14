use std::sync::Arc;

use tokio::sync::RwLock;

use crate::api::ApiHandler;
use crate::control_plane;
use crate::core::engine::{ProxyEngine, ProxyEngineConfig};
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
    /// Whether the SOCKS5 listener actually bound at startup (distinct from being configured).
    /// Set to true in spawn_runtime_listeners once we know the bind succeeded.
    pub(crate) socks5_bound: std::sync::atomic::AtomicBool,
    pub(crate) proxy_engine: Arc<ProxyEngine>,
    pub(crate) middleware_chain: Arc<RwLock<MiddlewareChain>>,
    pub(crate) throttling_config: Arc<RwLock<ThrottlingConfig>>,
    pub(crate) dns_overrides: Arc<RwLock<crate::middleware::plugins::dns_override::DnsOverrides>>,
    pub(crate) capture_filter: Arc<RwLock<CaptureFilterConfig>>,
    pub(crate) session_manager: crate::session::SharedSessionManager,
    pub(crate) breakpoint_manager: Arc<BreakpointManager>,
    pub(crate) api_handler: Arc<ApiHandler>,
    pub(crate) storage_path: std::path::PathBuf,
    pub(crate) started_at: std::time::Instant,
    pub(crate) endpoint_metrics: crate::control_plane::SharedEndpointMetrics,
    pub(crate) assistant: crate::control_plane::SharedAssistantState,
    pub(crate) workspace: crate::control_plane::SharedWorkspaceState,
    pub(crate) update_status: crate::control_plane::SharedUpdateStatus,
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

struct RuntimeComponents {
    throttling: Arc<RwLock<ThrottlingConfig>>,
    dns: Arc<RwLock<crate::middleware::plugins::dns_override::DnsOverrides>>,
    capture_filter: Arc<RwLock<CaptureFilterConfig>>,
    access_rules: SharedAccessRules,
    rule_sets: SharedRuleSets,
    map_local_rules: SharedMapLocalRules,
    map_remote_rules: SharedMapRemoteRules,
    breakpoints: Arc<BreakpointManager>,
}

impl RuntimeComponents {
    async fn load(storage_path: &std::path::Path) -> Self {
        let breakpoints = Arc::new(BreakpointManager::new());
        for rule in storage::load_breakpoints(storage_path) {
            breakpoints.add_rule(rule).await;
        }
        Self {
            throttling: Arc::new(RwLock::new(storage::load_throttle(storage_path))),
            dns: Arc::new(RwLock::new(storage::load_dns_overrides(storage_path))),
            capture_filter: Arc::new(RwLock::new(storage::load_capture_filter(storage_path))),
            access_rules: Arc::new(RwLock::new(storage::load_access_rules(storage_path))),
            rule_sets: Arc::new(RwLock::new(storage::load_rule_sets(storage_path))),
            map_local_rules: Arc::new(RwLock::new(storage::load_map_local_rules(storage_path))),
            map_remote_rules: Arc::new(RwLock::new(storage::load_map_remote_rules(storage_path))),
            breakpoints,
        }
    }

    fn build_request_chain(
        &self,
        session_manager: crate::session::SharedSessionManager,
    ) -> MiddlewareChain {
        let mut chain = MiddlewareChain::new();

        let mut access = AccessControlMiddleware::new(vec![]);
        access.rules = self.access_rules.clone();
        chain.add_middleware(Arc::new(access));
        chain.add_middleware(Arc::new(CaptureFilterMiddleware::new(
            self.capture_filter.clone(),
        )));
        chain.add_middleware(Arc::new(DnsOverrideMiddleware {
            overrides: self.dns.clone(),
        }));

        let mut map_remote = MapRemoteMiddleware::new(vec![]);
        map_remote.rules = self.map_remote_rules.clone();
        chain.add_middleware(Arc::new(map_remote));
        chain.add_middleware(Arc::new(ThrottlingMiddleware {
            config: self.throttling.clone(),
        }));

        let mut rewrite = UnifiedRewriteMiddleware::new(vec![]);
        rewrite.rules = self.rule_sets.clone();
        chain.add_middleware(Arc::new(rewrite));
        chain.add_middleware(Arc::new(BreakpointMiddleware::new(
            self.breakpoints.clone(),
            session_manager.clone(),
        )));
        chain.add_middleware(Arc::new(JwtInspectorMiddleware));
        chain.add_middleware(Arc::new(GraphQLInspectorMiddleware));
        chain.add_middleware(Arc::new(GrpcInspectorMiddleware {
            session_manager: session_manager.clone(),
            breakpoint_manager: self.breakpoints.clone(),
        }));
        chain.add_middleware(Arc::new(InspectionMiddleware::new(session_manager)));
        chain
    }
}

fn prepare_storage(storage_path: &std::path::Path) {
    for (path, purpose) in [
        (
            storage_path.to_path_buf(),
            "configuration changes will not persist across restarts",
        ),
        (
            storage_path.join("map-local"),
            "Map Local file uploads will not work",
        ),
    ] {
        if let Err(error) = std::fs::create_dir_all(&path) {
            eprintln!(
                "WARN: could not create directory '{}': {error}; {purpose}.",
                path.display()
            );
        }
    }
}

async fn build_proxy_engine(
    config: &crate::config::Config,
    storage_path: &std::path::Path,
    middleware_chain: Arc<RwLock<MiddlewareChain>>,
    ca: Arc<crate::certs::CertificateAuthority>,
    session_manager: crate::session::SharedSessionManager,
) -> Arc<ProxyEngine> {
    let hot_config = storage::load_hot_config(storage_path);
    let max_body_bytes = hot_config.max_body_bytes.unwrap_or(config.max_body_bytes);
    let stored_proxy = storage::load_upstream_proxy(storage_path);
    let engine = Arc::new(ProxyEngine::new(ProxyEngineConfig {
        middleware_chain,
        ca: Some(ca),
        mitm_enabled: config.mitm.enabled,
        bind_port: config.port,
        bind_host: config.bind_host.clone(),
        timeout_secs: config.timeout_secs,
        max_body_bytes,
        pool_max_idle_per_host: config.pool_max_idle_per_host,
        pool_idle_timeout_secs: config.pool_idle_timeout_secs,
        upstream_proxy: stored_proxy.or_else(|| config.upstream_proxy.clone()),
    }));
    engine
        .set_short_circuit_session_manager(session_manager)
        .await;
    if config.http3_enabled
        && let Some(port) = config.http3_port
    {
        engine.set_alt_svc_header(format!("h3=\":{port}\"; ma=86400"));
    }
    engine
}

async fn add_short_circuit_middlewares(
    chain: &Arc<RwLock<MiddlewareChain>>,
    config: &crate::config::Config,
    storage_path: &std::path::Path,
    map_local_rules: SharedMapLocalRules,
    mock_rules: crate::middleware::plugins::mock::SharedMockRules,
    lua_scripts: crate::middleware::plugins::lua_engine::SharedLuaScripts,
    session_manager: crate::session::SharedSessionManager,
) {
    let mut chain = chain.write().await;
    let mut map_local = MapLocalMiddleware::with_dirs(
        vec![],
        config.map_local_base_path.clone(),
        Some(storage_path.join("map-local")),
    );
    map_local.rules = map_local_rules;
    chain.add_middleware(Arc::new(map_local));
    chain.add_middleware(Arc::new(MockMiddleware::new(mock_rules)));
    chain.add_middleware(Arc::new(LuaEngineMiddleware::new(lua_scripts)));
    chain.add_middleware(Arc::new(InspectionMiddleware::new_response_pass(
        session_manager,
    )));
}

pub(super) async fn build_runtime_services(
    config: &crate::config::Config,
) -> Result<RuntimeServices, StartupError> {
    let session_manager = Arc::new(crate::session::SessionManager::with_body_budget(
        config.max_sessions,
        config.max_retained_body_bytes,
    ));

    let storage_path = config.storage_path.clone();

    prepare_storage(&storage_path);

    crate::examples::seed_first_run_examples(&storage_path).await;

    let components = RuntimeComponents::load(&storage_path).await;
    let middleware_chain = Arc::new(RwLock::new(
        components.build_request_chain(session_manager.clone()),
    ));

    let ca = Arc::new(
        crate::certs::CertificateAuthority::new(&config.mitm.root_ca_path)
            .await
            .map_err(|e| StartupError::CaInit(e.to_string()))?,
    );

    let proxy_engine = build_proxy_engine(
        config,
        &storage_path,
        middleware_chain.clone(),
        ca.clone(),
        session_manager.clone(),
    )
    .await;

    let api_handler = Arc::new(ApiHandler::new(
        session_manager.clone(),
        components.breakpoints.clone(),
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

    add_short_circuit_middlewares(
        &middleware_chain,
        config,
        &storage_path,
        components.map_local_rules.clone(),
        mock_rules_shared.clone(),
        lua_scripts_shared.clone(),
        session_manager.clone(),
    )
    .await;

    let state = Arc::new(AppState {
        socks5_bound: std::sync::atomic::AtomicBool::new(false),
        proxy_engine,
        middleware_chain,
        throttling_config: components.throttling,
        dns_overrides: components.dns,
        capture_filter: components.capture_filter,
        session_manager,
        breakpoint_manager: components.breakpoints,
        api_handler,
        storage_path,
        started_at: std::time::Instant::now(),
        endpoint_metrics: control_plane::new_endpoint_metrics(),
        assistant: control_plane::new_assistant_state(),
        workspace: control_plane::new_workspace_state(),
        update_status: control_plane::new_update_status(),
        config: config.clone(),
        webhooks: webhooks_shared,
        mock_rules: mock_rules_shared,
        lua_scripts: lua_scripts_shared,
        rule_sets: components.rule_sets,
        map_local_rules: components.map_local_rules,
        map_remote_rules: components.map_remote_rules,
        access_rules: components.access_rules,
    });

    Ok(RuntimeServices { state, ca })
}
