use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use weiyu_mcp::{
    admin::AdminState,
    cookie_store::CookieStore,
    http,
    query::QueryService,
    settings::{RuntimeSettings, SettingsStore},
};

const TOKEN: &str = "test-token-with-at-least-32-characters";
const ADMIN_TOKEN: &str = "test-admin-token-with-at-least-32-chars";
fn app() -> Router {
    app_with_write(false)
}
fn app_with_write(allow_write: bool) -> Router {
    http::router(
        QueryService::new(
            None,
            RuntimeSettings {
                rate_limit: 0,
                max_concurrency: 4,
                timeout_secs: 3,
                allow_write,
            },
        ),
        TOKEN.into(),
        None,
        vec![],
        vec![],
        CancellationToken::new(),
    )
}
fn admin_app(dir: &std::path::Path) -> Router {
    let service = QueryService::new(None, RuntimeSettings::default());
    let settings_store = SettingsStore::new(Some(dir.to_path_buf()));
    let cookie_store = CookieStore::new(Some(dir.to_path_buf()), None);
    let admin = AdminState::new(
        service.clone(),
        settings_store,
        cookie_store,
        ADMIN_TOKEN.into(),
    );
    http::router(
        service,
        TOKEN.into(),
        Some(admin),
        vec![],
        vec![],
        CancellationToken::new(),
    )
}
fn admin_request(method: &str, path: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("authorization", format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap()
}
fn request(body: Value) -> Request<Body> {
    Request::post("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-03-26")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}
async fn rpc(app: Router, body: Value) -> Value {
    let response = app.oneshot(request(body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn initialize_list_and_query_over_real_mcp_handler() {
    let server = app();
    let init=rpc(server.clone(),json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}})).await;
    assert_eq!(init["result"]["serverInfo"]["name"], "weiyu-mcp");
    let list = rpc(
        server.clone(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 35);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert!(![
            "download_video",
            "coin_video",
            "like_video",
            "delete_download",
            "save_settings"
        ]
        .contains(&tool["name"].as_str().unwrap()));
    }
    let result=rpc(server.clone(),json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"parse_url","arguments":{"url":"BV17x411w7KC"}}})).await;
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(result["result"]["structuredContent"]["aid"], 170001);
    let status=rpc(server,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_login_status","arguments":{}}})).await;
    assert_eq!(status["result"]["structuredContent"]["configured"], false);
    assert_eq!(
        status["result"]["structuredContent"]["write_enabled"],
        false
    );
}

#[tokio::test]
async fn write_tools_appear_only_after_opt_in() {
    let server = app_with_write(true);
    let list = rpc(
        server.clone(),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 40);
    let like = tools
        .iter()
        .find(|tool| tool["name"] == "like_video")
        .expect("like_video should be listed");
    assert_eq!(like["annotations"]["readOnlyHint"], false);
    let result=rpc(server,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"like_video","arguments":{"bvid":"BV17x411w7KC"}}})).await;
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["error"]["code"],
        "AUTH_REQUIRED"
    );
}

#[tokio::test]
async fn auth_is_required_but_health_check_is_public() {
    let mut req = request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    req.headers_mut().remove("authorization");
    let response = app().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));
    let mut req = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    req.headers_mut()
        .insert("authorization", "Bearer wrong-token".parse().unwrap());
    assert_eq!(
        app().oneshot(req).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut req = request(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
    req.headers_mut()
        .insert("authorization", format!("bearer {TOKEN}").parse().unwrap());
    assert_eq!(app().oneshot(req).await.unwrap().status(), StatusCode::OK);
    let response = app()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains(TOKEN));
}

#[tokio::test]
async fn host_and_origin_rules_apply_to_authenticated_requests() {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let mut req = request(body.clone());
    req.headers_mut()
        .insert("host", "unexpected.example".parse().unwrap());
    assert_eq!(
        app().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut req = request(body.clone());
    req.headers_mut()
        .insert("origin", "https://unexpected.example".parse().unwrap());
    assert_eq!(
        app().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let configured = http::router(
        QueryService::new(
            None,
            RuntimeSettings {
                rate_limit: 0,
                max_concurrency: 4,
                timeout_secs: 3,
                allow_write: false,
            },
        ),
        TOKEN.into(),
        None,
        vec!["mcp.example.com".into()],
        vec!["https://client.example.com:443".into()],
        CancellationToken::new(),
    );
    let mut req = request(body);
    req.headers_mut()
        .insert("host", "mcp.example.com".parse().unwrap());
    req.headers_mut()
        .insert("origin", "https://client.example.com".parse().unwrap());
    assert_eq!(
        configured.oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn tool_failures_are_actionable_and_do_not_become_protocol_failures() {
    let result=rpc(app(),json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_videos","arguments":{"keyword":"旅行"}}})).await;
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["error"]["code"],
        "AUTH_REQUIRED"
    );
    for url in [
        "http://127.0.0.1/",
        "https://www.bilibili.com/video/BV11111111111111111111111111111",
    ] {
        let result=rpc(app(),json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"parse_url","arguments":{"url":url}}})).await;
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(
            result["result"]["structuredContent"]["error"]["code"],
            "INVALID_PARAMS"
        );
    }
    let unknown=rpc(app(),json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"download_video","arguments":{}}})).await;
    assert!(unknown["error"].is_object());
}

#[tokio::test]
async fn oversized_requests_are_rejected() {
    let req = request(
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"parse_url","arguments":{"url":"x".repeat(1024*1024)}}}),
    );
    assert_eq!(
        app().oneshot(req).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn admin_requires_separate_token_and_serves_ui() {
    let dir = tempfile::tempdir().unwrap();
    let app = admin_app(dir.path());

    let response = app
        .clone()
        .oneshot(admin_request("GET", "/admin/api/status", TOKEN, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(admin_request("GET", "/admin/api/status", ADMIN_TOKEN, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["read_tools"], 35);
    assert_eq!(body["write_tools"], 40);
    assert_eq!(body["credential_configured"], false);
    assert_eq!(body["persistence"], true);

    let response = app
        .oneshot(admin_request("GET", "/admin", ADMIN_TOKEN, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("未雨"));
    assert!(html.contains("扫码"));
}

#[tokio::test]
async fn admin_settings_apply_to_live_service() {
    let dir = tempfile::tempdir().unwrap();
    let app = admin_app(dir.path());
    let payload = json!({
        "rate_limit": 5,
        "max_concurrency": 8,
        "timeout_secs": 30,
        "allow_write": true
    });
    let response = app
        .clone()
        .oneshot(admin_request(
            "PUT",
            "/admin/api/settings",
            ADMIN_TOKEN,
            Some(payload),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["restart_required"], true);
    assert!(dir.path().join("settings.json").exists());

    let list = rpc(
        app.clone(),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 40);

    let invalid = json!({
        "rate_limit": 101,
        "max_concurrency": 4,
        "timeout_secs": 45,
        "allow_write": false
    });
    let response = app
        .oneshot(admin_request(
            "PUT",
            "/admin/api/settings",
            ADMIN_TOKEN,
            Some(invalid),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_qr_poll_rejects_invalid_key_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let response = admin_app(dir.path())
        .oneshot(admin_request(
            "GET",
            "/admin/api/login/poll?key=bad%20key",
            ADMIN_TOKEN,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
