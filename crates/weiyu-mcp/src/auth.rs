use crate::error::{QueryError, Result};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::{error::Error as _, path::Path, sync::Arc};
use subtle::ConstantTimeEq;

pub fn validate_token(value: &str, name: &str) -> Result<String> {
    let token = value.trim().to_owned();
    if !(32..=4096).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(QueryError::new(
            "CONFIG_ERROR",
            format!("{name} 需为32至4096个可见ASCII字符"),
        ));
    }
    Ok(token)
}

/// Load a token from an optional file or an environment variable.
/// Returns `None` when neither source is configured.
pub fn load_token_file(path: Option<&Path>, env_name: &str) -> Result<Option<String>> {
    let value = if let Some(path) = path {
        let bytes = std::fs::metadata(path)
            .map_err(|_| QueryError::new("CONFIG_ERROR", format!("无法读取{env_name}令牌文件")))?
            .len();
        if bytes > 4096 {
            return Err(QueryError::new(
                "CONFIG_ERROR",
                format!("{env_name}令牌文件过大"),
            ));
        }
        Some(
            std::fs::read_to_string(path).map_err(|_| {
                QueryError::new("CONFIG_ERROR", format!("无法读取{env_name}令牌文件"))
            })?,
        )
    } else {
        std::env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    value
        .map(|value| validate_token(&value, env_name))
        .transpose()
}

pub fn load_required_token(path: Option<&Path>, env_name: &str) -> Result<String> {
    load_token_file(path, env_name)?.ok_or_else(|| {
        QueryError::new(
            "CONFIG_ERROR",
            format!("HTTP MCP需要配置 {env_name} 或 --token-file"),
        )
    })
}

/// Bearer authentication with constant-time comparison. POST bodies are
/// buffered once so the downstream MCP transport can enforce its own size limit.
pub async fn authorize(
    State(token): State<Arc<String>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, value)| value);
    if provided.is_some_and(|value| bool::from(value.as_bytes().ct_eq(token.as_bytes()))) {
        if request.method() == axum::http::Method::POST {
            let (parts, body) = request.into_parts();
            let body = match axum::body::to_bytes(body, 1024 * 1024).await {
                Ok(bytes) => Body::from(bytes),
                Err(error) => {
                    let status = if error
                        .source()
                        .is_some_and(|source| source.is::<http_body_util::LengthLimitError>())
                    {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    };
                    return (status, Json(json!({"error":"invalid_request_body"}))).into_response();
                }
            };
            return next.run(Request::from_parts(parts, body)).await;
        }
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"weiyu\"")],
        Json(json!({"error":"unauthorized"})),
    )
        .into_response()
}
