use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};

const OPERATOR_INDEX: &str = include_str!("../../web/index.html");
const OPERATOR_SCRIPT: &str = include_str!("../../web/app.js");
const OPERATOR_STYLES: &str = include_str!("../../web/styles.css");

pub(super) async fn index() -> Html<&'static str> {
    Html(OPERATOR_INDEX)
}

pub(super) async fn script() -> Response {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        OPERATOR_SCRIPT,
    )
        .into_response()
}

pub(super) async fn styles() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        OPERATOR_STYLES,
    )
        .into_response()
}
