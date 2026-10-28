//! Fonts for the UI language.
//!
//! egui's own fonts cover Latin, Greek and Cyrillic; everything else (kana, Han, Hangul) comes
//! from the system fonts, found on demand for each character by
//! [`egui_system_font_provider`]. Han characters are shared by Japanese and both Chinese
//! scripts but drawn differently, so the provider is told the UI language's locale: a Japanese
//! UI gets a Japanese font (Yu Gothic, Hiragino, Noto Sans CJK JP), Simplified Chinese a
//! Simplified one (Microsoft YaHei, PingFang SC, Noto Sans CJK SC), Traditional Chinese a
//! Traditional one (Microsoft JhengHei, PingFang TC, Noto Sans CJK TC) and Korean a Korean one
//! (Malgun Gothic, Apple SD Gothic Neo, Noto Sans CJK KR). Other languages keep the OS locale's
//! choice. A missing font only means the next font that has the character, as before.

use std::sync::Arc;

use super::Lang;

impl Lang {
    /// The locale the system font fallback picks CJK fonts by, or `None` for the OS locale.
    pub fn font_locale(self) -> Option<&'static str> {
        match self {
            Lang::Ja => Some("ja"),
            Lang::ZhHans => Some("zh-Hans"),
            Lang::ZhHant => Some("zh-Hant"),
            Lang::Ko => Some("ko"),
            _ => None,
        }
    }
}

/// Install the font providers for `lang` on `ctx`, replacing the ones there (eframe's system
/// font fallback, which follows the OS locale only). The named-font provider of the structure
/// editor (chembuider-rs) is installed with them, so replacing does not lose it.
///
/// The fonts are rebuilt at the start of the next pass, so this is cheap to call on a switch.
pub fn install(ctx: &egui::Context, lang: Lang) {
    let system = egui_system_font_provider::SystemFontProvider::new();
    let system = match lang.font_locale() {
        Some(locale) => system.with_locale(Some(locale)),
        None => system,
    };
    ctx.set_font_providers(vec![
        Arc::new(system),
        Arc::new(chembuider_rs::widget::fonts::SystemFontProvider),
    ]);
}
