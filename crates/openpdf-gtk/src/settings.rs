use std::fs;
use std::path::{Path, PathBuf};

use crate::i18n::Lang;
use crate::tiles::{DEFAULT_CACHE_BYTES, DEFAULT_TILE_SIZE};

const MAX_RECENT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoomMode {
    #[default]
    FitWidth,
    FitPage,
    Percent,
}

impl ZoomMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::FitWidth => "fit_width",
            Self::FitPage => "fit_page",
            Self::Percent => "percent",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "fit_page" => Self::FitPage,
            "percent" => Self::Percent,
            _ => Self::FitWidth,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageLayoutMode {
    #[default]
    Continuous,
    Single,
}

impl PageLayoutMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::Single => "single",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "single" => Self::Single,
            _ => Self::Continuous,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub language: String,
    pub theme: Theme,
    pub restore_last: bool,
    pub show_thumbnails: bool,
    pub zoom_mode: ZoomMode,
    pub zoom_percent: u32,
    pub page_layout: PageLayoutMode,
    pub tile_size: u32,
    pub cache_bytes: usize,
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::System,
            restore_last: false,
            show_thumbnails: true,
            zoom_mode: ZoomMode::FitWidth,
            zoom_percent: 100,
            page_layout: PageLayoutMode::Continuous,
            tile_size: DEFAULT_TILE_SIZE,
            cache_bytes: DEFAULT_CACHE_BYTES,
            recent: Vec::new(),
        }
    }
}

impl Settings {
    pub fn lang(&self) -> Lang {
        if self.language.is_empty() || self.language == "auto" {
            Lang::parse(&openpdf_core::detect_locale())
        } else {
            Lang::parse(&self.language)
        }
    }

    /// Apply this settings language to the shared gettext catalog.
    pub fn apply_locale(&self) {
        crate::i18n::apply_locale(&self.language);
    }

    pub fn zoom_factor(&self) -> f64 {
        match self.zoom_mode {
            ZoomMode::Percent => f64::from(self.zoom_percent.clamp(25, 400)) / 100.0,
            ZoomMode::FitWidth | ZoomMode::FitPage => 1.333,
        }
    }

    pub fn push_recent(&mut self, path: &Path) {
        let s = path.to_string_lossy().into_owned();
        self.recent.retain(|p| p != &s);
        self.recent.insert(0, s);
        self.recent.truncate(MAX_RECENT);
    }

    fn to_toml(&self) -> String {
        let mut out = String::from("# openPDF settings\n");
        out.push_str(&format!("language = {:?}\n", self.language));
        out.push_str(&format!("theme = {:?}\n", self.theme.as_str()));
        out.push_str(&format!("restore_last = {}\n", self.restore_last));
        out.push_str(&format!("show_thumbnails = {}\n", self.show_thumbnails));
        out.push_str(&format!("zoom_mode = {:?}\n", self.zoom_mode.as_str()));
        out.push_str(&format!("zoom_percent = {}\n", self.zoom_percent));
        out.push_str(&format!("page_layout = {:?}\n", self.page_layout.as_str()));
        out.push_str(&format!("tile_size = {}\n", self.tile_size));
        out.push_str(&format!("cache_bytes = {}\n", self.cache_bytes));
        if self.recent.is_empty() {
            out.push_str("recent = []\n");
        } else {
            out.push_str("recent = [\n");
            for p in &self.recent {
                out.push_str(&format!("  {:?},\n", p));
            }
            out.push_str("]\n");
        }
        out
    }

    fn from_toml(raw: &str) -> Self {
        let mut s = Settings::default();
        for raw_line in raw.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = unquote(v.trim().trim_end_matches(','));
                match key {
                    "language" => s.language = val,
                    "theme" => s.theme = Theme::parse(&val),
                    "restore_last" => s.restore_last = val == "true",
                    "show_thumbnails" => s.show_thumbnails = val == "true",
                    "zoom_mode" => s.zoom_mode = ZoomMode::parse(&val),
                    "zoom_percent" => s.zoom_percent = val.parse().unwrap_or(s.zoom_percent),
                    "page_layout" => s.page_layout = PageLayoutMode::parse(&val),
                    "tile_size" => s.tile_size = val.parse().unwrap_or(s.tile_size),
                    "cache_bytes" => s.cache_bytes = val.parse().unwrap_or(s.cache_bytes),
                    _ => {}
                }
            }
        }
        for line in raw.lines() {
            let t = line.trim().trim_end_matches(',');
            if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
                s.recent.push(unquote(t));
            }
        }
        s
    }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

/// Directory for settings: `$OPENPDF_CONFIG_DIR`, else `$XDG_CONFIG_HOME/openpdf`,
/// else `~/.config/openpdf`.
pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("OPENPDF_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    openpdf_core::settings_dir()
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

pub fn load() -> Settings {
    match fs::read_to_string(settings_path()) {
        Ok(raw) => Settings::from_toml(&raw),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(config_dir())?;
    fs::write(settings_path(), settings.to_toml())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn roundtrip_under_xdg() {
        let _g = ENV.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("openpdf-gtk-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("OPENPDF_CONFIG_DIR", &tmp);
        let mut s = Settings::default();
        s.language = "es".into();
        s.theme = Theme::Dark;
        s.show_thumbnails = false;
        s.push_recent(Path::new("/tmp/a.pdf"));
        s.push_recent(Path::new("/tmp/b.pdf"));
        s.push_recent(Path::new("/tmp/a.pdf"));
        save(&s).unwrap();
        let loaded = load();
        assert_eq!(loaded.language, "es");
        assert_eq!(loaded.theme, Theme::Dark);
        assert!(!loaded.show_thumbnails);
        assert_eq!(
            loaded.recent,
            vec!["/tmp/a.pdf".to_string(), "/tmp/b.pdf".to_string()]
        );
        assert_eq!(loaded.lang(), Lang::Es);
        let _ = fs::remove_dir_all(&tmp);
        std::env::remove_var("OPENPDF_CONFIG_DIR");
    }

    #[test]
    fn missing_file_is_default() {
        let _g = ENV.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("openpdf-gtk-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        std::env::set_var("OPENPDF_CONFIG_DIR", &tmp);
        let s = load();
        assert_eq!(s.language, "auto");
        assert_eq!(s.tile_size, DEFAULT_TILE_SIZE);
        std::env::remove_var("OPENPDF_CONFIG_DIR");
    }
}
