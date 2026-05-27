use axum::{
    http::header,
    response::{Html, IntoResponse},
};

mod design_assets {
    include!(concat!(env!("OUT_DIR"), "/design_assets.rs"));
}

pub(super) async fn robots_txt() -> impl IntoResponse {
    (
        [("content-type", "text/plain")],
        "User-agent: *\nDisallow: /\n",
    )
}

pub(super) async fn serve_index() -> impl IntoResponse {
    Html(design_assets::INDEX_HTML)
}

pub(super) async fn serve_manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../manifest.json"),
    )
}

pub(super) async fn serve_sw() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("../sw.js"),
    )
}

pub(super) async fn serve_icon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../icon.svg"),
    )
}

pub(super) async fn serve_design_app_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        design_assets::APP_CSS,
    )
}

pub(super) async fn serve_design_app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        design_assets::APP_JS,
    )
}

pub(super) async fn serve_setup_wizard() -> impl IntoResponse {
    Html(include_str!("../setup_wizard.html"))
}

pub(super) async fn not_found() -> impl IntoResponse {
    axum::http::StatusCode::NOT_FOUND
}
