use crate::error::{QueryError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Runtime settings persisted in `<data-dir>/settings.json`.
///
/// CLI/env values only seed the file on first start; once the file exists it is
/// the source of truth, so changes made in the admin UI survive restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    pub rate_limit: u32,
    pub max_concurrency: u32,
    pub timeout_secs: u64,
    pub allow_write: bool,
    /// 评论发表单独开关；默认关闭，不受 allow_write 影响
    #[serde(default)]
    pub allow_comment: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            rate_limit: 2,
            max_concurrency: 4,
            timeout_secs: 45,
            allow_write: false,
            allow_comment: false,
        }
    }
}

impl RuntimeSettings {
    pub fn validate(&self) -> Result<()> {
        if self.rate_limit > 100 {
            return Err(QueryError::invalid("rate_limit 必须在 0 到 100 之间"));
        }
        if !(1..=64).contains(&self.max_concurrency) {
            return Err(QueryError::invalid("max_concurrency 必须在 1 到 64 之间"));
        }
        if !(1..=600).contains(&self.timeout_secs) {
            return Err(QueryError::invalid("timeout_secs 必须在 1 到 600 之间"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SettingsStore {
    path: Option<PathBuf>,
}

impl SettingsStore {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join("settings.json")),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn load(&self, defaults: RuntimeSettings) -> Result<RuntimeSettings> {
        let Some(path) = &self.path else {
            return Ok(defaults);
        };
        if !path.exists() {
            return Ok(defaults);
        }
        let bytes = std::fs::read(path).map_err(|error| {
            QueryError::new(
                "CONFIG_ERROR",
                format!("无法读取设置文件 {}: {error}", path.display()),
            )
        })?;
        let settings: RuntimeSettings = serde_json::from_slice(&bytes).map_err(|error| {
            QueryError::new(
                "CONFIG_ERROR",
                format!("设置文件 {} 格式错误: {error}", path.display()),
            )
        })?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, settings: &RuntimeSettings) -> Result<()> {
        settings.validate()?;
        let path = self.path.as_ref().ok_or_else(|| {
            QueryError::new("CONFIG_ERROR", "未配置 WEIYU_DATA_DIR，设置无法持久化")
        })?;
        let bytes = serde_json::to_vec_pretty(settings)?;
        write_private_atomic(path, &bytes)
    }

    /// Write the seed settings on first start. A read-only data dir must not
    /// prevent the server from starting.
    pub fn save_if_missing(&self, settings: &RuntimeSettings) {
        let Some(path) = &self.path else {
            return;
        };
        if path.exists() {
            return;
        }
        if let Err(error) = self.save(settings) {
            log::warn!("[settings] 无法创建 {}: {error}", path.display());
        }
    }
}

pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            QueryError::new(
                "CONFIG_ERROR",
                format!("无法创建目录 {}: {error}", parent.display()),
            )
        })?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|error| {
        QueryError::new(
            "CONFIG_ERROR",
            format!("无法写入 {}: {error}", tmp.display()),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|error| {
        QueryError::new(
            "CONFIG_ERROR",
            format!("无法保存 {}: {error}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ranges() {
        let mut settings = RuntimeSettings::default();
        assert!(settings.validate().is_ok());
        settings.rate_limit = 101;
        assert!(settings.validate().is_err());
        settings.rate_limit = 2;
        settings.max_concurrency = 0;
        assert!(settings.validate().is_err());
        settings.max_concurrency = 4;
        settings.timeout_secs = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn loads_legacy_settings_without_allow_comment() {
        let raw = br#"{"rate_limit":2,"max_concurrency":4,"timeout_secs":45,"allow_write":true}"#;
        let settings: RuntimeSettings = serde_json::from_slice(raw).unwrap();
        assert!(settings.allow_write);
        assert!(!settings.allow_comment);
    }

    #[test]
    fn saves_and_loads_from_data_dir() {
        let dir = std::env::temp_dir().join(format!("weiyu-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SettingsStore::new(Some(dir.clone()));
        let defaults = RuntimeSettings::default();
        assert_eq!(store.load(defaults.clone()).unwrap(), defaults);
        store.save_if_missing(&defaults);
        assert!(dir.join("settings.json").exists());
        let mut changed = defaults.clone();
        changed.rate_limit = 7;
        store.save(&changed).unwrap();
        assert_eq!(store.load(defaults).unwrap().rate_limit, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
