use async_trait::async_trait;
use graphql_parser::query::{Definition, OperationDefinition, parse_query};

use crate::middleware::{Middleware, MiddlewareAction, RequestContext, ResponseContext};
use crate::session::GraphQLInfo;

pub struct GraphQLInspectorMiddleware;

impl GraphQLInspectorMiddleware {
    fn is_graphql(ctx: &RequestContext) -> bool {
        let ct = ctx
            .headers
            .get("content-type")
            .map(|v| v.as_str())
            .unwrap_or("");
        (ct.contains("application/json") || ct.contains("application/graphql"))
            && ctx.body.contains("\"query\"")
    }

    pub fn parse(body: &str) -> Option<GraphQLInfo> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        let query_str = v.get("query")?.as_str()?;
        let variables = v.get("variables").cloned();

        let (operation_type, operation_name) = Self::parse_operation(query_str);

        Some(GraphQLInfo {
            operation_type,
            operation_name,
            variables,
        })
    }

    fn parse_operation(query_str: &str) -> (String, Option<String>) {
        if let Ok(doc) = parse_query::<String>(query_str) {
            for def in &doc.definitions {
                match def {
                    Definition::Operation(op) => {
                        let (op_type, op_name) = match op {
                            OperationDefinition::Query(q) => {
                                ("query".to_string(), q.name.as_ref().map(|n| n.to_string()))
                            }
                            OperationDefinition::Mutation(m) => (
                                "mutation".to_string(),
                                m.name.as_ref().map(|n| n.to_string()),
                            ),
                            OperationDefinition::Subscription(s) => (
                                "subscription".to_string(),
                                s.name.as_ref().map(|n| n.to_string()),
                            ),
                            OperationDefinition::SelectionSet(_) => ("query".to_string(), None),
                        };
                        return (op_type, op_name);
                    }
                    Definition::Fragment(_) => {}
                }
            }
        }
        // Fallback: keyword detection
        let lower = query_str.trim_start().to_lowercase();
        if lower.starts_with("mutation") {
            ("mutation".to_string(), None)
        } else if lower.starts_with("subscription") {
            ("subscription".to_string(), None)
        } else {
            ("query".to_string(), None)
        }
    }
}

#[async_trait]
impl Middleware for GraphQLInspectorMiddleware {
    fn name(&self) -> &str {
        "GraphQLInspectorMiddleware"
    }

    async fn on_request(&self, ctx: &mut RequestContext) -> MiddlewareAction {
        if Self::is_graphql(ctx)
            && let Some(info) = Self::parse(&ctx.body)
            && let Ok(json) = serde_json::to_string(&info)
        {
            ctx.headers.insert("x-oproxy-graphql".to_string(), json);
        }
        MiddlewareAction::Continue
    }

    async fn on_response(&self, _ctx: &mut ResponseContext) -> MiddlewareAction {
        MiddlewareAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_ctx(content_type: &str, body: &str) -> RequestContext {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), content_type.to_string());
        RequestContext {
            method: "POST".to_string(),
            uri: "/graphql".to_string(),
            headers,
            body: body.to_string(),
            host: "api.example.com".to_string(),
            body_bytes: None,
        }
    }

    #[test]
    fn query_detected_and_parsed() {
        let body = r#"{"query":"{ user { id name } }"}"#;
        let info = GraphQLInspectorMiddleware::parse(body).unwrap();
        assert_eq!(info.operation_type, "query");
        assert!(info.operation_name.is_none());
    }

    #[test]
    fn named_query_extracts_name() {
        let body = r#"{"query":"query GetUser { user { id } }"}"#;
        let info = GraphQLInspectorMiddleware::parse(body).unwrap();
        assert_eq!(info.operation_type, "query");
        assert_eq!(info.operation_name.as_deref(), Some("GetUser"));
    }

    #[test]
    fn mutation_detected() {
        let body =
            r#"{"query":"mutation CreateUser($name: String!) { createUser(name: $name) { id } }"}"#;
        let info = GraphQLInspectorMiddleware::parse(body).unwrap();
        assert_eq!(info.operation_type, "mutation");
        assert_eq!(info.operation_name.as_deref(), Some("CreateUser"));
    }

    #[test]
    fn subscription_detected() {
        let body = r#"{"query":"subscription OnMessage { messageAdded { id text } }"}"#;
        let info = GraphQLInspectorMiddleware::parse(body).unwrap();
        assert_eq!(info.operation_type, "subscription");
    }

    #[test]
    fn variables_extracted() {
        let body = r#"{"query":"mutation M($x: Int!) { foo(x: $x) }","variables":{"x":42}}"#;
        let info = GraphQLInspectorMiddleware::parse(body).unwrap();
        let vars = info.variables.unwrap();
        assert_eq!(vars["x"], 42);
    }

    #[test]
    fn non_graphql_post_ignored() {
        let ctx = make_ctx("application/json", r#"{"name":"test"}"#);
        assert!(!GraphQLInspectorMiddleware::is_graphql(&ctx));
    }

    #[test]
    fn non_json_content_type_ignored() {
        let ctx = make_ctx("text/plain", r#"{"query":"{ user { id } }"}"#);
        assert!(!GraphQLInspectorMiddleware::is_graphql(&ctx));
    }

    #[test]
    fn graphql_content_type_with_query_field_detected() {
        let ctx = make_ctx("application/graphql", r#"{"query":"{ user { id } }"}"#);
        assert!(GraphQLInspectorMiddleware::is_graphql(&ctx));
    }

    #[test]
    fn application_json_with_query_field_detected() {
        let ctx = make_ctx("application/json", r#"{"query":"{ user { id } }"}"#);
        assert!(GraphQLInspectorMiddleware::is_graphql(&ctx));
    }
}
