use std::sync::Arc;
use tracing::{debug, instrument};

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};

#[derive(Clone)]
pub struct MiddlewareChain {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    pub fn add_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    /// Returns the names of all registered middlewares in execution order.
    pub fn list_plugins(&self) -> Vec<String> {
        self.middlewares
            .iter()
            .map(|m| m.name().to_string())
            .collect()
    }

    #[instrument(skip(self, ctx))]
    pub async fn execute_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        for middleware in &self.middlewares {
            debug!("Executing middleware request step");
            let action = middleware.on_request(ctx).await;
            if action != MiddlewareAction::Continue {
                debug!("Middleware request step stopped chain");
                return action;
            }
        }
        MiddlewareAction::Continue
    }

    #[instrument(skip(self, ctx))]
    pub async fn execute_response(&self, ctx: &mut ResponseContext) -> MiddlewareAction {
        // Response middleware are typically executed in reverse order
        for middleware in self.middlewares.iter().rev() {
            debug!("Executing middleware response step");
            let action = middleware.on_response(ctx).await;
            if action != MiddlewareAction::Continue {
                debug!("Middleware response step stopped chain");
                return action;
            }
        }
        MiddlewareAction::Continue
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Records which order middlewares fired by pushing a label into a shared vec.
    struct OrderMiddleware {
        label: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
        req_action: MiddlewareAction,
        res_action: MiddlewareAction,
    }

    #[async_trait]
    impl Middleware for OrderMiddleware {
        fn name(&self) -> &str {
            self.label
        }
        async fn on_request(&self, _ctx: &mut RequestContext) -> MiddlewareAction {
            self.order.lock().unwrap().push(self.label);
            self.req_action
        }
        async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
            self.order.lock().unwrap().push(self.label);
            self.res_action
        }
    }

    fn req() -> RequestContext {
        RequestContext {
            method: "GET".to_string(),
            uri: "/".to_string(),
            headers: HashMap::new(),
            body: "".to_string(),
            host: "localhost".to_string(),
            body_bytes: None,
        }
    }

    fn res() -> ResponseContext {
        ResponseContext {
            status: 200,
            headers: HashMap::new(),
            body: "".to_string(),
            request_uri: "/".to_string(),
            session_id: None,
            ttfb_ms: 0,
            body_ms: 0,
            body_bytes: None,
        }
    }

    #[tokio::test]
    async fn empty_chain_returns_continue_for_request() {
        let chain = MiddlewareChain::new();
        assert_eq!(
            chain.execute_request(&mut req()).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn empty_chain_returns_continue_for_response() {
        let chain = MiddlewareChain::new();
        assert_eq!(
            chain.execute_response(&mut res()).await,
            MiddlewareAction::Continue
        );
    }

    #[tokio::test]
    async fn request_middlewares_run_in_insertion_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "A",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "B",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        chain.execute_request(&mut req()).await;
        assert_eq!(*order.lock().unwrap(), vec!["A", "B"]);
    }

    #[tokio::test]
    async fn response_middlewares_run_in_reverse_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "A",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "B",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        chain.execute_response(&mut res()).await;
        assert_eq!(*order.lock().unwrap(), vec!["B", "A"]);
    }

    #[tokio::test]
    async fn stop_and_return_short_circuits_request_chain() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "A",
            order: order.clone(),
            req_action: MiddlewareAction::StopAndReturn,
            res_action: MiddlewareAction::Continue,
        }));
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "B",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        let action = chain.execute_request(&mut req()).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        assert_eq!(
            *order.lock().unwrap(),
            vec!["A"],
            "B must not run after StopAndReturn"
        );
    }

    #[tokio::test]
    async fn stop_and_return_short_circuits_response_chain() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut chain = MiddlewareChain::new();
        // B is added second → runs FIRST on response (reverse order)
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "A",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "B",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::StopAndReturn,
        }));
        let action = chain.execute_response(&mut res()).await;
        assert_eq!(action, MiddlewareAction::StopAndReturn);
        assert_eq!(
            *order.lock().unwrap(),
            vec!["B"],
            "A must not run after B returns StopAndReturn"
        );
    }

    #[tokio::test]
    async fn pause_action_short_circuits_and_is_propagated() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut chain = MiddlewareChain::new();
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "A",
            order: order.clone(),
            req_action: MiddlewareAction::Pause,
            res_action: MiddlewareAction::Continue,
        }));
        chain.add_middleware(Arc::new(OrderMiddleware {
            label: "B",
            order: order.clone(),
            req_action: MiddlewareAction::Continue,
            res_action: MiddlewareAction::Continue,
        }));
        let action = chain.execute_request(&mut req()).await;
        assert_eq!(action, MiddlewareAction::Pause);
        assert_eq!(*order.lock().unwrap(), vec!["A"]);
    }
}
