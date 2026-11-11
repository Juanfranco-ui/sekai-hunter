use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// XDG config directory for the application.
pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(".config")
    });
    base.join("sekai-hunter")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// User-selected MHW game directory.
    pub game_dir: Option<String>,
    /// Window width.
    pub window_width: f32,
    /// Window height.
    pub window_height: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game_dir: None,
            window_width: 920.0,
            window_height: 680.0,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(cfg) => return cfg,
                    Err(e) => log::warn!("Failed to parse config: {e}"),
                },
                Err(e) => log::warn!("Failed to read config: {e}"),
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                log::error!(
                    "Failed to create config directory {}: {error}",
                    parent.display()
                );
                return;
            }
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = fs::write(&path, s) {
                    log::error!("Failed to write config: {e}");
                }
            }
            Err(e) => log::error!("Failed to serialize config: {e}"),
        }
    }

    pub fn game_path(&self) -> Option<PathBuf> {
        self.game_dir.as_ref().map(PathBuf::from)
    }
}

/// Common Steam library paths where MHW might be installed.
pub fn common_steam_paths() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home"));
    let mhw_suffix = Path::new("steamapps/common/Monster Hunter World");

    let steam_roots = [
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut candidates = Vec::new();

    for steam_root in &steam_roots {
        candidates.push(steam_root.join(mhw_suffix));
        let vdf_path = steam_root.join("steamapps/libraryfolders.vdf");
        if let Ok(content) = fs::read_to_string(vdf_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("\"path\"") {
                    if let Some(val) = trimmed.split('"').nth(3) {
                        candidates.push(PathBuf::from(val).join(mhw_suffix));
                    }
                }
            }
        }
    }

    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    unique
}
