use crate::error::{QueryError, Result};
use crate::settings::write_private_atomic;
use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

const DEFAULT_PASSWORD: &str = "admin";
const MIN_PASSWORD_LEN: usize = 4;
const MAX_PASSWORD_LEN: usize = 128;
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAdmin {
    hash: String,
    #[serde(default)]
    default_password: bool,
}

#[derive(Default)]
struct LoginGuard {
    failures: u32,
    locked_until: Option<Instant>,
}

pub struct AdminAuth {
    path: Option<PathBuf>,
    stored: RwLock<Option<StoredAdmin>>,
    sessions: RwLock<HashMap<String, Instant>>,
    legacy_token: Option<String>,
    login_guard: Mutex<LoginGuard>,
    setup_lock: Mutex<()>,
}

#[derive(Debug, Clone, Copy)]
pub struct AdminAuthState {
    pub needs_setup: bool,
    pub authenticated: bool,
    pub using_default_password: bool,
}

impl AdminAuth {
    pub fn new(data_dir: Option<PathBuf>, legacy_token: Option<String>) -> Arc<Self> {
        let path = data_dir.map(|dir| dir.join("admin.json"));
        let stored = path.as_ref().and_then(|path| load_stored(path));
        Arc::new(Self {
            path,
            stored: RwLock::new(stored),
            sessions: RwLock::new(HashMap::new()),
            legacy_token,
            login_guard: Mutex::new(LoginGuard::default()),
            setup_lock: Mutex::new(()),
        })
    }

    pub fn needs_setup(&self) -> bool {
        self.stored.read().expect("admin lock").is_none()
    }

    pub fn using_default_password(&self) -> bool {
        self.stored
            .read()
            .expect("admin lock")
            .as_ref()
            .is_some_and(|stored| stored.default_password)
    }

    pub fn state(&self, bearer: Option<&str>) -> AdminAuthState {
        AdminAuthState {
            needs_setup: self.needs_setup(),
            authenticated: bearer.is_some_and(|token| self.verify_bearer(token)),
            using_default_password: self.using_default_password(),
        }
    }

    /// First-run setup. Missing or empty passwords use `admin`.
    pub fn setup(&self, password: Option<&str>) -> Result<(String, bool)> {
        let _guard = self.setup_lock.lock().expect("setup lock");
        if self.stored.read().expect("admin lock").is_some() {
            return Err(QueryError::new("CONFLICT", "管理密码已设置"));
        }
        let path = self.path.as_ref().ok_or_else(|| {
            QueryError::new("CONFIG_ERROR", "未配置 WEIYU_DATA_DIR，管理密码无法持久化")
        })?;
        let password = normalize_password(password);
        validate_password(&password)?;
        let stored = StoredAdmin {
            hash: hash_password(&password)?,
            default_password: password == DEFAULT_PASSWORD,
        };
        write_stored(path, &stored)?;
        *self.stored.write().expect("admin lock") = Some(stored.clone());
        let session = self.create_session();
        Ok((session, stored.default_password))
    }

    pub fn login(&self, password: &str) -> Result<String> {
        let stored = self
            .stored
            .read()
            .expect("admin lock")
            .clone()
            .ok_or_else(|| QueryError::new("SETUP_REQUIRED", "尚未设置管理密码"))?;
        {
            let mut guard = self.login_guard.lock().expect("login lock");
            if let Some(until) = guard.locked_until {
                if Instant::now() < until {
                    return Err(QueryError::new(
                        "RATE_LIMITED",
                        "密码错误次数过多，请稍后再试",
                    ));
                }
                guard.locked_until = None;
                guard.failures = 0;
            }
        }
        if verify_password(password, &stored.hash) {
            let mut guard = self.login_guard.lock().expect("login lock");
            guard.failures = 0;
            guard.locked_until = None;
            drop(guard);
            Ok(self.create_session())
        } else {
            let mut guard = self.login_guard.lock().expect("login lock");
            guard.failures += 1;
            if guard.failures >= MAX_FAILURES {
                guard.failures = 0;
                guard.locked_until = Some(Instant::now() + LOCKOUT);
                return Err(QueryError::new(
                    "RATE_LIMITED",
                    "密码错误次数过多，已临时锁定 30 秒",
                ));
            }
            Err(QueryError::new("AUTH_REQUIRED", "密码错误"))
        }
    }

    pub fn change_password(&self, current: &str, new: &str) -> Result<String> {
        let stored = self
            .stored
            .read()
            .expect("admin lock")
            .clone()
            .ok_or_else(|| QueryError::new("SETUP_REQUIRED", "尚未设置管理密码"))?;
        if !verify_password(current, &stored.hash) {
            return Err(QueryError::new("INVALID_PASSWORD", "当前密码错误"));
        }
        let new = new.trim();
        validate_password(new)?;
        let path = self.path.as_ref().ok_or_else(|| {
            QueryError::new("CONFIG_ERROR", "未配置 WEIYU_DATA_DIR，管理密码无法持久化")
        })?;
        let new_stored = StoredAdmin {
            hash: hash_password(new)?,
            default_password: new == DEFAULT_PASSWORD,
        };
        write_stored(path, &new_stored)?;
        *self.stored.write().expect("admin lock") = Some(new_stored);
        self.sessions.write().expect("session lock").clear();
        Ok(self.create_session())
    }

    pub fn logout(&self, token: &str) {
        self.sessions.write().expect("session lock").remove(token);
    }

    pub fn verify_bearer(&self, token: &str) -> bool {
        if let Some(legacy) = &self.legacy_token {
            if bool::from(token.as_bytes().ct_eq(legacy.as_bytes())) {
                return true;
            }
        }
        let now = Instant::now();
        let mut sessions = self.sessions.write().expect("session lock");
        sessions.retain(|_, expiry| *expiry > now);
        if let Some(expiry) = sessions.get_mut(token) {
            *expiry = now + SESSION_TTL;
            true
        } else {
            false
        }
    }

    fn create_session(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        self.sessions
            .write()
            .expect("session lock")
            .insert(token.clone(), Instant::now() + SESSION_TTL);
        token
    }
}

fn normalize_password(password: Option<&str>) -> String {
    let password = password.unwrap_or("").trim();
    if password.is_empty() {
        DEFAULT_PASSWORD.to_owned()
    } else {
        password.to_owned()
    }
}

fn validate_password(password: &str) -> Result<()> {
    let length = password.chars().count();
    if !(MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&length) {
        return Err(QueryError::invalid(format!(
            "密码长度需在 {MIN_PASSWORD_LEN} 到 {MAX_PASSWORD_LEN} 个字符之间"
        )));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| QueryError::new("INTERNAL_ERROR", format!("密码哈希失败: {error}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

fn load_stored(path: &Path) -> Option<StoredAdmin> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("[admin] 无法读取 {}: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(stored) => Some(stored),
        Err(error) => {
            log::error!(
                "[admin] 管理密码文件 {} 格式错误: {error}；将按未设置处理",
                path.display()
            );
            None
        }
    }
}

fn write_stored(path: &Path, stored: &StoredAdmin) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(stored)?;
    write_private_atomic(path, &bytes)
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, value)| value)
}

pub async fn authorize(
    State(auth): State<Arc<AdminAuth>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let allowed = bearer_token(request.headers()).is_some_and(|token| auth.verify_bearer(token));
    if allowed {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"weiyu-admin\"")],
            Json(json!({"error":"unauthorized"})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_login_and_change_password() {
        let dir = std::env::temp_dir().join(format!("weiyu-admin-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let auth = AdminAuth::new(Some(dir.clone()), None);
        assert!(auth.needs_setup());
        let (session, using_default) = auth.setup(None).unwrap();
        assert!(using_default);
        assert!(auth.verify_bearer(&session));
        assert!(auth.login("admin").is_ok());
        assert!(auth.login("wrong").is_err());
        let new_session = auth.change_password("admin", "secret-1234").unwrap();
        assert!(auth.verify_bearer(&new_session));
        assert!(!auth.using_default_password());
        assert!(auth.login("admin").is_err());
        assert!(auth.login("secret-1234").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_token_is_still_accepted() {
        let dir = std::env::temp_dir().join(format!("weiyu-admin-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let auth = AdminAuth::new(
            Some(dir.clone()),
            Some("legacy-token-with-32-characters!!".into()),
        );
        assert!(auth.verify_bearer("legacy-token-with-32-characters!!"));
        assert!(!auth.verify_bearer("wrong"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
