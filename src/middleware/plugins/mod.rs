// Bandwidth throttling is implemented in routing.rs ThrottlingMiddleware::on_response
// via a proportional sleep based on body size and bandwidth_limit_kbps config field.
pub mod breakpoints;
pub mod capture_filter;
pub mod dns_override;
pub mod graphql_inspector;
pub mod grpc_inspector;
pub mod header_map;
pub mod inspection;
pub mod jwt_inspector;
pub mod lua_engine;
pub mod mock;
pub mod modification;
pub mod rewrite;
pub mod routing;
