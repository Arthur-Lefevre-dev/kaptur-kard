//! TOML-backed UI strings (English default; French optional via Infos prefs).

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Prefs / combo value. Unknown or empty → English (app default).
    pub fn from_pref(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "fr" | "french" | "francais" | "français" => Lang::Fr,
            _ => Lang::En,
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
    /// Load from prefs string (`"en"` / `"fr"`). Empty / unknown → English.
    pub fn load_pref(pref: &str) -> Self {
        Self::with_lang(Lang::from_pref(pref))
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
        assert_eq!(i18n.t("label_hdr"), "Sortie HDR");
        assert_eq!(i18n.t("hdr_off"), "Désactivé");
        assert!(i18n.t("vrr_status_capable").contains("VRR"));
        assert_eq!(i18n.t("btn_pin"), "Épingler");
        assert!(i18n.t("hint_f2_metrics").contains("F2"));
        assert!(i18n.t("tip_dlss_greyed").contains("Grisé"));
        assert!(i18n.t("effect_reason_dlss_gpu").contains("BYO"));
        assert_eq!(i18n.t("launch_tips_title"), "Raccourcis");
        assert_eq!(i18n.t("launch_tips_key_f1"), "F1");
        assert!(i18n.t("launch_tips_desc_f1").contains("Réglages"));
        assert_eq!(i18n.t("label_language"), "Langue");
    }

    #[test]
    fn english_is_default_and_complete() {
        let i18n = I18n::load_pref("");
        assert_eq!(i18n.lang(), Lang::En);
        assert_eq!(i18n.t("toolbar_fullscreen"), "Full screen");
        assert_eq!(i18n.t("launch_tips_title"), "Shortcuts");
        assert_eq!(i18n.t("launch_tips_key_f2"), "F2");
        assert!(i18n.t("launch_tips_desc_f2").contains("Metrics"));
        assert_eq!(i18n.t("label_language"), "Language");
        assert_eq!(Lang::from_pref(""), Lang::En);
        assert_eq!(Lang::from_pref("fr"), Lang::Fr);
    }

    #[test]
    fn fr_en_locale_keys_match() {
        let fr = parse_locale(include_str!("../../../locales/fr.toml"));
        let en = parse_locale(include_str!("../../../locales/en.toml"));
        let mut only_fr: Vec<_> = fr.keys().filter(|k| !en.contains_key(*k)).cloned().collect();
        let mut only_en: Vec<_> = en.keys().filter(|k| !fr.contains_key(*k)).cloned().collect();
        only_fr.sort();
        only_en.sort();
        assert!(only_fr.is_empty(), "keys only in fr.toml: {only_fr:?}");
        assert!(only_en.is_empty(), "keys only in en.toml: {only_en:?}");
        assert!(!fr.is_empty());
    }
}
