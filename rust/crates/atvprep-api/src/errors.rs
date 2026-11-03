//! API error envelopes compatible with the Python manager.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProblemDetail {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub instance: String,
    pub error: ProblemError,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProblemError {
    pub code: String,
    pub message: String,
}

impl ProblemDetail {
    #[must_use]
    pub fn new(status: StatusCode, code: &str, message: &str, path: &str) -> Self {
        let api = path.starts_with("/api/");
        let code = if api {
            match status {
                StatusCode::UNAUTHORIZED => "unauthorized",
                StatusCode::NOT_FOUND => "not_found",
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => "invalid_request",
                status if status.is_client_error() || status == StatusCode::SERVICE_UNAVAILABLE => {
                    "request_error"
                }
                _ => code,
            }
        } else {
            code
        };
        let message =
            if api && status == StatusCode::UNPROCESSABLE_ENTITY && message == "invalid request" {
                "request validation failed"
            } else {
                message
            };
        let title = if api && status == StatusCode::NOT_FOUND {
            "Not Found"
        } else {
            message
        };
        Self {
            kind: "about:blank".to_owned(),
            title: title.to_owned(),
            status: status.as_u16(),
            detail: message.to_owned(),
            instance: path.to_owned(),
            error: ProblemError {
                code: code.to_owned(),
                message: message.to_owned(),
            },
        }
    }
}
impl IntoResponse for ProblemDetail {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(self),
        )
            .into_response()
    }
}

/// Normalize framework rejections and use the actual request path on all API problems.
pub(crate) async fn normalise_response(response: Response, path: &str) -> Response {
    use axum::{
        body::{Body, to_bytes},
        http::header,
    };
    use serde_json::{Value, json};
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, crate::bounds::MAX_RESPONSE_BYTES).await else {
        return ProblemDetail::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal server error",
            path,
        )
        .into_response();
    };
    if let Ok(mut problem) = serde_json::from_slice::<Value>(&bytes)
        && problem.get("error").is_some()
        && problem.get("instance").is_some()
    {
        problem["instance"] = json!(path);
        if let Ok(encoded) = serde_json::to_vec(&problem) {
            parts.headers.remove(header::CONTENT_LENGTH);
            return Response::from_parts(parts, Body::from(encoded));
        }
    }
    if status == StatusCode::METHOD_NOT_ALLOWED {
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.insert(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        return Response::from_parts(parts, Body::from(r#"{"detail":"Method Not Allowed"}"#));
    }
    let rejection = match status {
        StatusCode::BAD_REQUEST
        | StatusCode::UNSUPPORTED_MEDIA_TYPE
        | StatusCode::UNPROCESSABLE_ENTITY => Some((
            StatusCode::UNPROCESSABLE_ENTITY,
            "request validation failed",
        )),

        _ => None,
    };
    if let Some((status, message)) = rejection {
        let mut response =
            ProblemDetail::new(status, "request_error", message, path).into_response();
        for (name, value) in &parts.headers {
            if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
                response.headers_mut().append(name.clone(), value.clone());
            }
        }
        return response;
    }
    Response::from_parts(parts, Body::from(bytes))
}
