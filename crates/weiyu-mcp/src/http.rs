use crate::{admin, auth, mcp::QueryServer, query::QueryService};
use axum::{http::StatusCode, middleware, response::IntoResponse, routing::get, Json, Router};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub fn router(
    service: Arc<QueryService>,
    token: String,
    admin: Option<admin::AdminState>,
    hosts: Vec<String>,
    origins: Vec<String>,
    cancel: CancellationToken,
) -> Router {
    let mut config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(1024 * 1024)
        .with_allowed_origins(origins)
        .enforce_origin_validation()
        .with_cancellation_token(cancel);
    if !hosts.is_empty() {
        config = config.with_allowed_hosts(hosts);
    }
    let mcp = StreamableHttpService::new(
        move || Ok(QueryServer(service.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let protected = Router::new()
        .nest_service("/mcp", mcp)
        .layer(middleware::from_fn_with_state(
            Arc::new(token),
            auth::authorize,
        ));
    let mut app = Router::new()
        .route(
            "/healthz",
            get(|| async {
                Json(json!({"status":"ok","service":"weiyu-mcp","version":env!("CARGO_PKG_VERSION")}))
            }),
        )
        .merge(protected);
    match admin {
        Some(admin) => app = app.merge(admin::router(admin)),
        None => {
            app = app
                .route("/admin", get(admin_disabled))
                .route("/admin/", get(admin_disabled));
        }
    }
    app
}

async fn admin_disabled() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "管理台未启用：请配置 WEIYU_ADMIN_TOKEN 或 --admin-token-file 后重启服务。",
    )
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {_=ctrl_c=>{},_=terminate=>{}}
}
