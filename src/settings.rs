use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const MIN_LOCK_MINUTES: u32 = 1;
pub const MAX_LOCK_MINUTES: u32 = 30;
pub const MIN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_INTERVAL_MINUTES: u32 = 180;
pub const DEFAULT_LOCK_MINUTES: u32 = 5;
pub const DEFAULT_INTERVAL_MINUTES: u32 = 50;

fn default_work_interval() -> u32 {
    DEFAULT_INTERVAL_MINUTES
}

fn default_lock_duration() -> u32 {
    DEFAULT_LOCK_MINUTES
}

/// Durations Stand keeps between launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_work_interval")]
    pub work_interval_minutes: u32,
    #[serde(default = "default_lock_duration")]
    pub lock_duration_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_interval_minutes: DEFAULT_INTERVAL_MINUTES,
            lock_duration_minutes: DEFAULT_LOCK_MINUTES,
        }
    }
}

impl Settings {
    pub fn clamped(&self) -> Self {
        Self {
            work_interval_minutes: clamp_interval(self.work_interval_minutes),
            lock_duration_minutes: clamp_lock(self.lock_duration_minutes),
        }
    }

    pub fn work_duration(&self) -> Duration {
        Duration::from_secs(u64::from(self.work_interval_minutes) * 60)
    }

    pub fn lock_duration(&self) -> Duration {
        Duration::from_secs(u64::from(self.lock_duration_minutes) * 60)
    }

    pub fn set_work_interval_minutes(&mut self, minutes: u32) {
        self.work_interval_minutes = clamp_interval(minutes);
    }

    pub fn set_lock_duration_minutes(&mut self, minutes: u32) {
        self.lock_duration_minutes = clamp_lock(minutes);
    }

    /// Missing file yields the defaults. A file that does not parse is an error
    /// so a corrupt config is not silently treated as a successful customization.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let settings: Settings = serde_json::from_str(&text)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
                Ok(settings.clamped())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.clamped())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text + "\n")?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn clamp_interval(minutes: u32) -> u32 {
    minutes.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES)
}

pub fn clamp_lock(minutes: u32) -> u32 {
    minutes.clamp(MIN_LOCK_MINUTES, MAX_LOCK_MINUTES)
}

/// `STAND_CONFIG_DIR` overrides the platform directory so tests and a portable
/// checkout can keep settings beside the process.
pub fn settings_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("STAND_CONFIG_DIR") {
        return PathBuf::from(dir).join("settings.json");
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        return home
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Stand/settings.json");
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("stand/settings.json");
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        home.unwrap_or_else(|| PathBuf::from("."))
            .join(".config/stand/settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_product() {
        let settings = Settings::default();
        assert_eq!(settings.work_interval_minutes, 50);
        assert_eq!(settings.lock_duration_minutes, 5);
        assert_eq!(settings.work_duration(), Duration::from_secs(50 * 60));
        assert_eq!(settings.lock_duration(), Duration::from_secs(5 * 60));
    }

    #[test]
    fn setters_clamp_to_the_allowed_ranges() {
        let mut settings = Settings::default();
        settings.set_work_interval_minutes(0);
        settings.set_lock_duration_minutes(0);
        assert_eq!(settings.work_interval_minutes, MIN_INTERVAL_MINUTES);
        assert_eq!(settings.lock_duration_minutes, MIN_LOCK_MINUTES);

        settings.set_work_interval_minutes(10_000);
        settings.set_lock_duration_minutes(10_000);
        assert_eq!(settings.work_interval_minutes, MAX_INTERVAL_MINUTES);
        assert_eq!(settings.lock_duration_minutes, MAX_LOCK_MINUTES);
    }

    #[test]
    fn load_clamps_out_of_range_values_and_fills_missing_fields() {
        let dir = std::env::temp_dir().join(format!("stand-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        fs::write(&path, "{}\n").unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded, Settings::default());

        fs::write(
            &path,
            r#"{"work_interval_minutes": 0, "lock_duration_minutes": 90}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.work_interval_minutes, MIN_INTERVAL_MINUTES);
        assert_eq!(loaded.lock_duration_minutes, MAX_LOCK_MINUTES);

        fs::write(&path, "not json").unwrap();
        assert!(Settings::load(&path).is_err());

        let missing = dir.join("missing.json");
        assert_eq!(Settings::load(&missing).unwrap(), Settings::default());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_round_trips() {
        let dir = std::env::temp_dir().join(format!("stand-settings-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested/settings.json");
        let mut settings = Settings::default();
        settings.set_work_interval_minutes(25);
        settings.set_lock_duration_minutes(12);
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let _ = fs::remove_dir_all(&dir);
    }
}
