//! User settings persisted as one JSON file. Thresholds live here, never in core/cleaner.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Items above this need an extra confirmation.
    pub large_item_gb: f64,
    /// A cleanup above this total shows the bulk warning summary.
    pub bulk_total_gb: f64,
    /// `node_modules` of projects untouched this long are "stale".
    pub stale_node_modules_days: u32,
    /// "auto", "pl" or "en".
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            large_item_gb: 5.0,
            bulk_total_gb: 25.0,
            stale_node_modules_days: 30,
            language: "auto".into(),
        }
    }
}

pub const GB: f64 = 1_000_000_000.0;

impl Settings {
    pub fn large_item_bytes(&self) -> u64 {
        (self.large_item_gb * GB) as u64
    }
    pub fn bulk_total_bytes(&self) -> u64 {
        (self.bulk_total_gb * GB) as u64
    }

    /// Missing or corrupt file → defaults (logged), never an error.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("settings {path:?} unreadable, using defaults: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Test mode keeps settings inside the fixture root so the real config is never touched.
pub fn settings_path(test_root: Option<&Path>) -> PathBuf {
    match test_root {
        Some(root) => root.join(".fdc-settings.json"),
        None => directories::ProjectDirs::from("com", "codelion", "FreeDiskCleaner")
            .map(|d| d.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            large_item_gb: 1.5,
            ..Default::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        std::fs::write(&p, r#"{"bulk_total_gb": 3}"#).unwrap();
        assert_eq!(Settings::load(&p).bulk_total_gb, 3.0);
        assert_eq!(Settings::load(&p).large_item_gb, 5.0);
    }
}
