//! Write your own middleware plugin.
//!
//! Every traffic feature in oproxy is a [`Middleware`]: implement the trait,
//! add it to a [`MiddlewareChain`], and the engine runs it on every request.
//! This example defines a plugin that stamps a header onto each request and
//! counts how many it has seen, then runs a request through the chain.
//!
//! Run with:
//!
//! ```text
//! cargo run --example custom_middleware
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use oproxy::middleware::chain::MiddlewareChain;
use oproxy::middleware::{Middleware, MiddlewareAction, RequestContext};

/// A minimal plugin: inject a couple of headers and tally requests.
struct StampHeader {
    seen: AtomicUsize,
}

#[async_trait]
impl Middleware for StampHeader {
    fn name(&self) -> &str {
        "stamp-header"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        let n = self.seen.fetch_add(1, Ordering::Relaxed) + 1;
        ctx.headers.insert("x-stamped-by", "oproxy-example");
        ctx.headers.insert("x-request-number", n.to_string());
        // Returning `Continue` lets the next plugin (and ultimately the upstream
        // forward) run. Return `StopAndReturn` to short-circuit with a response.
        MiddlewareAction::Continue
    }
}

#[tokio::main]
async fn main() {
    let mut chain = MiddlewareChain::new();
    chain.add_middleware(Arc::new(StampHeader {
        seen: AtomicUsize::new(0),
    }));

    let mut req = RequestContext {
        method: "GET".to_string(),
        uri: "/hello".to_string(),
        host: "example.com".to_string(),
        ..Default::default()
    };

    let action = chain.execute_request(&mut req).await;
    assert_eq!(action, MiddlewareAction::Continue);

    println!("chain action: {action:?}");
    println!("x-stamped-by    = {:?}", req.headers.get("x-stamped-by"));
    println!(
        "x-request-number = {:?}",
        req.headers.get("x-request-number")
    );
}
