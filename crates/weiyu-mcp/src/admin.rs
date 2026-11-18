use crate::{
    auth,
    cookie_store::CookieStore,
    error::QueryError,
    query::QueryService,
    settings::{RuntimeSettings, SettingsStore},
};
use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    middleware,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};
use weiyu_core::bilibili as bili;

const INDEX_HTML: &str = include_str!("../static/admin.html");

#[derive(Clone)]
pub struct AdminState {
    service: Arc<QueryService>,
    settings_store: SettingsStore,
    cookie_store: CookieStore,
    admin_token: String,
    started_at: Instant,
    read_tool_count: usize,
    write_tool_count: usize,
}

impl AdminState {
    pub fn new(
        service: Arc<QueryService>,
        settings_store: SettingsStore,
        cookie_store: CookieStore,
        admin_token: String,
    ) -> Self {
        Self {
            service,
            settings_store,
            cookie_store,
            admin_token,
            started_at: Instant::now(),
            read_tool_count: crate::query::tools(false).len(),
            write_tool_count: crate::query::tools(true).len(),
        }
    }
}

pub fn router(state: AdminState) -> Router {
    let token = Arc::new(state.admin_token.clone());
    let protected = Router::new()
        .route("/admin/api/status", get(status))
        .route("/admin/api/settings", get(get_settings).put(put_settings))
        .route("/admin/api/login/qrcode", post(login_qrcode))
        .route("/admin/api/login/poll", get(login_poll))
        .route("/admin/api/login/verify", post(login_verify))
        .route("/admin/api/logout", post(logout))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(token, auth::authorize))
        .with_state(state);
    Router::new()
        .route("/admin", get(index))
        .route("/admin/", get(index))
        .merge(protected)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

type ApiError = (StatusCode, Json<Value>);

fn api_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": {"code": code, "message": message.into()}})),
    )
}

fn map_query_error(error: QueryError) -> ApiError {
    let status = match error.code {
        "INVALID_PARAMS" | "CONFIG_ERROR" => StatusCode::BAD_REQUEST,
        "AUTH_REQUIRED" => StatusCode::UNAUTHORIZED,
        "UPSTREAM_ERROR" | "BILIBILI_VERIFICATION_REQUIRED" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_error(status, error.code, error.message)
}

async fn status(State(state): State<AdminState>) -> Json<Value> {
    let credential = state.service.credential_snapshot();
    let settings = state.service.settings();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "read_tools": state.read_tool_count,
        "write_tools": state.write_tool_count,
        "credential_configured": credential.is_some(),
        "credential_source": state.cookie_store.source().as_str(),
        "write_ready": credential.as_ref().is_some_and(|c| !c.bili_jct.trim().is_empty()),
        "write_enabled": settings.allow_write,
        "settings": settings,
        "restart_required": state.service.restart_required(),
        "data_dir": state
            .cookie_store
            .data_path()
            .and_then(|path| path.parent())
            .map(|path| path.display().to_string()),
        "persistence": state.cookie_store.can_write(),
    }))
}

async fn get_settings(State(state): State<AdminState>) -> Json<Value> {
    Json(json!({
        "settings": state.service.settings(),
        "restart_required": state.service.restart_required(),
        "persistence": state.cookie_store.can_write(),
        "data_dir": state
            .cookie_store
            .data_path()
            .and_then(|path| path.parent())
            .map(|path| path.display().to_string()),
    }))
}

async fn put_settings(
    State(state): State<AdminState>,
    Json(settings): Json<RuntimeSettings>,
) -> Result<Json<Value>, ApiError> {
    settings.validate().map_err(map_query_error)?;
    state
        .settings_store
        .save(&settings)
        .map_err(map_query_error)?;
    state
        .service
        .update_settings(settings.clone())
        .map_err(map_query_error)?;
    Ok(Json(json!({
        "ok": true,
        "settings": settings,
        "restart_required": state.service.restart_required(),
    })))
}

async fn login_qrcode(State(_state): State<AdminState>) -> Result<Json<Value>, ApiError> {
    let data = bili::passport::generate_qrcode()
        .await
        .map_err(|error| map_query_error(error.into()))?;
    let code = qrcode::QrCode::new(data.url.as_bytes()).map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "QR_ERROR",
            format!("二维码生成失败: {error}"),
        )
    })?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(280, 280)
        .quiet_zone(true)
        .build();
    Ok(Json(json!({
        "qrcode_key": data.qrcode_key,
        "svg": svg,
        "expires_in_secs": 180,
    })))
}

#[derive(Deserialize)]
struct PollQuery {
    key: String,
}

async fn login_poll(
    State(state): State<AdminState>,
    Query(params): Query<PollQuery>,
) -> Result<Json<Value>, ApiError> {
    let key = params.key.trim();
    if key.is_empty() || key.len() > 256 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PARAMS",
            "qrcode key 无效",
        ));
    }
    let (code, credential) = bili::passport::poll_qrcode(key)
        .await
        .map_err(|error| map_query_error(error.into()))?;
    match code {
        0 => {
            let credential = credential.ok_or_else(|| {
                api_error(
                    StatusCode::BAD_GATEWAY,
                    "UPSTREAM_ERROR",
                    "登录成功但未返回 Cookie",
                )
            })?;
            let mut persisted = true;
            let mut warning = None;
            if let Err(error) = state.cookie_store.save(&credential) {
                persisted = false;
                warning = Some(error.message);
            }
            let write_ready = !credential.bili_jct.trim().is_empty();
            state.service.update_credential(Some(credential));
            Ok(Json(json!({
                "status": "success",
                "message": "登录成功",
                "persisted": persisted,
                "write_ready": write_ready,
                "warning": warning,
            })))
        }
        86101 => Ok(Json(json!({"status":"pending","message":"等待扫码"}))),
        86090 => Ok(Json(
            json!({"status":"scanned","message":"已扫码，请在手机上确认"}),
        )),
        86038 => Ok(Json(
            json!({"status":"expired","message":"二维码已过期，请重新生成"}),
        )),
        other => Ok(Json(json!({
            "status": "error",
            "code": other,
            "message": "登录状态异常，请重新生成二维码",
        }))),
    }
}

async fn login_verify(State(state): State<AdminState>) -> Result<Json<Value>, ApiError> {
    let credential = state.service.credential_snapshot().ok_or_else(|| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "尚未配置 B站 Cookie",
        )
    })?;
    let user = bili::passport::validate_credentials(&credential)
        .await
        .map_err(|error| map_query_error(error.into()))?;
    Ok(Json(json!({"ok": true, "user": user})))
}

async fn logout(State(state): State<AdminState>) -> Result<Json<Value>, ApiError> {
    state.service.update_credential(None);
    let warning = match state.cookie_store.clear() {
        Ok(warning) => warning,
        Err(error) => Some(error.message),
    };
    Ok(Json(json!({"ok": true, "warning": warning})))
}
