use axum::extract::{DefaultBodyLimit, State, rejection::JsonRejection};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::{ApiError, AppState, Config, ParseRequest, Runner, app_state, process_demo};

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"code": self.code, "message": self.message}})),
        )
            .into_response()
    }
}

pub fn router(config: Config) -> Result<Router, reqwest::Error> {
    let state = app_state(config)?;
    Ok(Router::new()
        .route("/", post(parse))
        .route("/parse", post(parse))
        .route(
            "/health",
            get(|State(state): State<AppState>| async move {
                Json(json!({
                    "status": "ok",
                    "runners": [Runner::ChatMessages, Runner::DemoCheck],
                    "max_concurrent_requests": state.config.max_concurrent_requests,
                }))
            }),
        )
        .fallback(|| async {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "use POST /parse or GET /health",
            )
        })
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state))
}

async fn parse(
    State(state): State<AppState>,
    request: Result<Json<ParseRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            error.body_text(),
        )
    })?;
    if request.run.is_empty()
        || request.run.len() > 2
        || request
            .run
            .iter()
            .enumerate()
            .any(|(i, r)| request.run[..i].contains(r))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_run",
            "run must contain one or two distinct supported runner names",
        ));
    }
    if request
        .match_start_unix
        .is_some_and(|n| !n.is_finite() || n < 0.0)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_timestamp",
            "match_start_unix must be finite, nonnegative Unix seconds",
        ));
    }
    let body = process_demo(&state, request).await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}
