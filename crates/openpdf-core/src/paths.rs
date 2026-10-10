use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::Result;

const APP: &str = "openpdf";

fn project() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", APP)
}

/// Settings directory: XDG config on Linux, `%APPDATA%\openpdf` on Windows,
/// `~/Library/Application Support/openpdf` on macOS.
pub fn settings_dir() -> PathBuf {
    project()
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(APP))
}

/// Recovery directory: XDG state on Linux, `%LOCALAPPDATA%\openpdf` on Windows,
/// `~/Library/Caches/openpdf` on macOS (`state_dir` is Linux-only).
pub fn recovery_dir() -> PathBuf {
    project()
        .map(|p| {
            p.state_dir().map(PathBuf::from).unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    p.cache_dir().to_path_buf()
                } else {
                    p.data_local_dir().to_path_buf()
                }
            })
        })
        .unwrap_or_else(|| PathBuf::from(APP))
}

/// Create settings and recovery directories if they do not exist.
pub fn ensure_app_dirs() -> Result<(PathBuf, PathBuf)> {
    let settings = settings_dir();
    let recovery = recovery_dir();
    std::fs::create_dir_all(&settings)?;
    std::fs::create_dir_all(&recovery)?;
    Ok((settings, recovery))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    #[test]
    fn xdg_overrides_and_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("XDG_CONFIG_HOME", "/tmp/openpdf-test-config");
        env::set_var("XDG_STATE_HOME", "/tmp/openpdf-test-state");
        assert_eq!(
            settings_dir(),
            PathBuf::from("/tmp/openpdf-test-config/openpdf")
        );
        assert_eq!(
            recovery_dir(),
            PathBuf::from("/tmp/openpdf-test-state/openpdf")
        );
        env::remove_var("XDG_CONFIG_HOME");
        env::remove_var("XDG_STATE_HOME");
        let settings = settings_dir();
        let recovery = recovery_dir();
        assert!(settings.ends_with("openpdf"), "settings={settings:?}");
        assert!(recovery.ends_with("openpdf"), "recovery={recovery:?}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_appdata() {
        let _g = ENV_LOCK.lock().unwrap();
        let settings = settings_dir();
        let recovery = recovery_dir();
        let settings_s = settings.to_string_lossy().to_ascii_lowercase();
        let recovery_s = recovery.to_string_lossy().to_ascii_lowercase();
        assert!(
            settings_s.contains("appdata") && settings.ends_with("openpdf"),
            "settings={settings:?}"
        );
        assert!(recovery.ends_with("openpdf"), "recovery={recovery:?}");
        assert_ne!(settings_s, recovery_s);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_library() {
        let _g = ENV_LOCK.lock().unwrap();
        let settings = settings_dir();
        let recovery = recovery_dir();
        assert!(
            settings.to_string_lossy().contains("Library"),
            "settings={settings:?}"
        );
        assert!(settings.ends_with("openpdf"), "settings={settings:?}");
        assert!(recovery.ends_with("openpdf"), "recovery={recovery:?}");
    }
}
