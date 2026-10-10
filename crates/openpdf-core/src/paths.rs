use std::env;
use std::fs;
use std::path::PathBuf;

use crate::error::Result;

const APP: &str = "openpdf";

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.config/openpdf`, honoring `XDG_CONFIG_HOME`.
pub fn settings_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", |home| home.join(".config")).join(APP)
}

/// `~/.local/state/openpdf`, honoring `XDG_STATE_HOME`.
pub fn recovery_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", |home| home.join(".local").join("state")).join(APP)
}

fn xdg_dir(var: &str, fallback: impl FnOnce(&std::path::Path) -> PathBuf) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback(&home_dir()),
    }
}

/// Create settings and recovery directories if they do not exist.
pub fn ensure_app_dirs() -> Result<(PathBuf, PathBuf)> {
    let settings = settings_dir();
    let recovery = recovery_dir();
    fs::create_dir_all(&settings)?;
    fs::create_dir_all(&recovery)?;
    Ok((settings, recovery))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn xdg_overrides_and_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("XDG_CONFIG_HOME", "/tmp/openpdf-test-config");
        env::set_var("XDG_STATE_HOME", "/tmp/openpdf-test-state");
        assert_eq!(settings_dir(), PathBuf::from("/tmp/openpdf-test-config/openpdf"));
        assert_eq!(recovery_dir(), PathBuf::from("/tmp/openpdf-test-state/openpdf"));
        env::remove_var("XDG_CONFIG_HOME");
        env::remove_var("XDG_STATE_HOME");
        let settings = settings_dir();
        let recovery = recovery_dir();
        assert!(settings.ends_with(".config/openpdf"));
        assert!(recovery.ends_with(".local/state/openpdf"));
    }
}
