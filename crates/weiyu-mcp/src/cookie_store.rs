use crate::{
    error::{QueryError, Result},
    input,
    settings::write_private_atomic,
};
use std::path::{Path, PathBuf};
use weiyu_core::bilibili::credential::Credential;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieSource {
    /// Writable cookie managed by the admin UI / QR login.
    Data,
    /// Read-only cookie provided by the operator.
    Env,
    None,
}

impl CookieSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Env => "env",
            Self::None => "none",
        }
    }
}

/// Resolves the active Bilibili credential.
///
/// A cookie in the writable data directory wins. An empty data-dir cookie file
/// is a logout marker: it intentionally suppresses the read-only fallback so a
/// logout survives a restart. The fallback is only used when no data-dir file
/// exists at all.
#[derive(Clone)]
pub struct CookieStore {
    data_path: Option<PathBuf>,
    fallback_path: Option<PathBuf>,
}

impl CookieStore {
    pub fn new(data_dir: Option<PathBuf>, fallback: Option<PathBuf>) -> Self {
        Self {
            data_path: data_dir.map(|dir| dir.join("bilibili.cookie")),
            fallback_path: fallback,
        }
    }

    pub fn data_path(&self) -> Option<&Path> {
        self.data_path.as_deref()
    }

    pub fn can_write(&self) -> bool {
        self.data_path.is_some()
    }

    pub fn source(&self) -> CookieSource {
        match &self.data_path {
            Some(path) if path.exists() => {
                if std::fs::metadata(path)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false)
                {
                    CookieSource::Data
                } else {
                    CookieSource::None
                }
            }
            _ => match &self.fallback_path {
                Some(path) if path.exists() => {
                    if std::fs::metadata(path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false)
                    {
                        CookieSource::Env
                    } else {
                        CookieSource::None
                    }
                }
                _ => CookieSource::None,
            },
        }
    }

    pub fn load(&self) -> Result<(Option<Credential>, CookieSource)> {
        if let Some(path) = &self.data_path {
            if path.exists() {
                let credential = input::load_cookie(Some(path))?;
                let source = if credential.is_some() {
                    CookieSource::Data
                } else {
                    CookieSource::None
                };
                return Ok((credential, source));
            }
        }
        if let Some(path) = &self.fallback_path {
            if path.exists() {
                let credential = input::load_cookie(Some(path))?;
                let source = if credential.is_some() {
                    CookieSource::Env
                } else {
                    CookieSource::None
                };
                return Ok((credential, source));
            }
        }
        Ok((None, CookieSource::None))
    }

    pub fn save(&self, credential: &Credential) -> Result<()> {
        let path = self.data_path.as_ref().ok_or_else(|| {
            QueryError::new(
                "CONFIG_ERROR",
                "未配置 WEIYU_DATA_DIR，扫码登录结果无法持久化",
            )
        })?;
        let mut text = credential.cookie_header();
        text.push('\n');
        write_private_atomic(path, text.as_bytes())
    }

    /// Clear the active credential. Writes an empty data-dir cookie as a logout
    /// marker when possible. Returns a warning when a read-only fallback remains.
    pub fn clear(&self) -> Result<Option<String>> {
        let mut warning = None;
        if let Some(path) = &self.data_path {
            write_private_atomic(path, b"")?;
        }
        if let Some(path) = &self.fallback_path {
            if path.exists() {
                warning = Some(format!(
                    "已清除数据目录登录态，但只读 Cookie 文件 {} 仍存在；重启后会作为回退加载。",
                    path.display()
                ));
            }
        }
        Ok(warning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_cookie_wins_over_fallback_and_empty_marks_logout() {
        let dir = std::env::temp_dir().join(format!("weiyu-cookie-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("bilibili.cookie");
        let fallback = dir.join("fallback.cookie");
        std::fs::write(&fallback, "SESSDATA=env; bili_jct=csrf; DedeUserID=1\n").unwrap();
        let store = CookieStore::new(Some(dir.clone()), Some(fallback.clone()));
        let (credential, source) = store.load().unwrap();
        assert!(credential.is_some());
        assert_eq!(source, CookieSource::Env);

        let credential = credential.unwrap();
        store.save(&credential).unwrap();
        let (credential, source) = store.load().unwrap();
        assert!(credential.is_some());
        assert_eq!(source, CookieSource::Data);

        store.clear().unwrap();
        let (credential, source) = store.load().unwrap();
        assert!(credential.is_none());
        assert_eq!(source, CookieSource::None);
        assert!(data.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
