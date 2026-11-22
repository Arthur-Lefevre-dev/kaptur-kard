//! TOML-backed UI strings (French + English via sys-locale).

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    pub fn from_sys() -> Self {
        let loc = sys_locale::get_locale().unwrap_or_else(|| "en".into());
        let lower = loc.to_ascii_lowercase();
        if lower.starts_with("fr") {
            Lang::Fr
        } else {
            Lang::En
        }
    }
}

#[derive(Debug, Deserialize)]
struct LocaleFile {
    #[serde(flatten)]
    entries: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct I18n {
    lang: Lang,
    map: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl I18n {
    pub fn load() -> Self {
        let lang = Lang::from_sys();
        Self::with_lang(lang)
    }

    pub fn with_lang(lang: Lang) -> Self {
        let fr = parse_locale(include_str!("../../../locales/fr.toml"));
        let en = parse_locale(include_str!("../../../locales/en.toml"));
        let (map, fallback) = match lang {
            Lang::Fr => (fr, en),
            Lang::En => (en, fr),
        };
        Self {
            lang,
            map,
            fallback,
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn t(&self, key: &str) -> String {
        self.map
            .get(key)
            .cloned()
            .or_else(|| self.fallback.get(key).cloned())
            .unwrap_or_else(|| key.to_string())
    }
}

fn parse_locale(raw: &str) -> HashMap<String, String> {
    toml::from_str::<LocaleFile>(raw)
        .map(|f| f.entries)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_has_toolbar_keys() {
        let i18n = I18n::with_lang(Lang::Fr);
        assert_eq!(i18n.t("toolbar_image"), "Image");
        assert!(i18n.t("why_fsr_body").contains("retard"));
        assert!(i18n.t("err_device_busy").contains("autre application"));
        assert_eq!(i18n.t("label_device_busy"), "Carte de capture indisponible");
        assert_eq!(i18n.t("wizard_step1_title"), "Source vidéo entrante");
        assert!(i18n.t("btn_source_auto").contains("Automatique"));
        assert_eq!(i18n.t("label_frame_gen_fps"), "FPS après interpolation");
        assert_eq!(i18n.t("label_display_fps_limit"), "Limite FPS écran");
        assert_eq!(i18n.t("label_vsync"), "VSync");
        assert_eq!(i18n.t("vsync_off"), "Désactivé");
        assert_eq!(i18n.t("btn_pin"), "Épingler");
        assert!(i18n.t("hint_f2_metrics").contains("F2"));
    }

    #[test]
    fn english_fallback_works() {
        let i18n = I18n::with_lang(Lang::En);
        assert_eq!(i18n.t("toolbar_fullscreen"), "Full screen");
    }
}
