// General program settings (not audio): language and how the program starts / closes. Stored in %APPDATA%\SonarLite\settings.txt.
use crate::i18n::Lang;
use std::{fs, path::PathBuf};

#[derive(Clone)]
pub struct Settings {
    pub lang: Lang,
    /// closing the window hides the program in the tray instead of quitting
    pub close_to_tray: bool,
    /// the autostart entry starts the program hidden in the tray
    pub start_in_tray: bool,
}

impl Default for Settings { fn default() -> Self { Settings { lang: Lang::Pl, close_to_tray: true, start_in_tray: true } } }

pub fn dir() -> PathBuf { PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("SonarLite") }

fn path() -> PathBuf { dir().join("settings.txt") }

pub fn load() -> Settings { parse(&fs::read_to_string(path()).unwrap_or_default()) }

fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for l in text.lines() {
        match l.split_once('=') {
            Some(("lang", v)) => s.lang = Lang::from_code(v.trim()),
            Some(("close_to_tray", v)) => s.close_to_tray = v.trim() == "1",
            Some(("start_in_tray", v)) => s.start_in_tray = v.trim() == "1",
            _ => {}
        }
    }
    s
}

fn serialize(s: &Settings) -> String {
    format!("lang={}\nclose_to_tray={}\nstart_in_tray={}\n", s.lang.code(), s.close_to_tray as u8, s.start_in_tray as u8)
}

pub fn save(s: &Settings) {
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(path(), serialize(s));
}

#[cfg(test)]
mod t {
    use super::*;
    #[test]
    fn survives_save_and_load_and_has_sane_defaults() {
        let d = parse("");
        assert!(d.lang == Lang::Pl && d.close_to_tray && d.start_in_tray);
        let s = Settings { lang: Lang::En, close_to_tray: false, start_in_tray: false };
        let r = parse(&serialize(&s));
        assert!(r.lang == Lang::En && !r.close_to_tray && !r.start_in_tray);
    }
}
