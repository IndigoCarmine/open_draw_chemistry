//! The UI language: translated strings looked up by key, with named placeholders and plurals.
//!
//! # Catalogs
//!
//! Each language has one TOML file in `locales/` (`en.toml`, `ja.toml`, `zh-Hans.toml`, …),
//! embedded at compile time. Keys are grouped by area in tables (`[menu.file]`, `[status]`,
//! `[three_d.edit]`, …); a key is its dotted path (`menu.file.open`). A value is either a string
//! or, for text with a count, a table of plural forms:
//!
//! ```toml
//! [status]
//! opened = "Opened {name}"
//! atoms = { one = "{n} atom", other = "{n} atoms" }
//! ```
//!
//! `{name}` is a named placeholder (`{{` and `}}` are literal braces). The plural forms are the
//! CLDR categories (`zero`, `one`, `two`, `few`, `many`, `other`) the language uses; `other` is
//! required, and `{n}` is the count.
//!
//! # Lookup
//!
//! [`t!`](crate::t) and [`tn!`](crate::tn) look a key up in the current [`language`], then in
//! English, and finally show the key itself, so a missing translation never hides a control.
//!
//! # Adding a language
//!
//! Add a variant to [`Lang`] (its code, native name and plural rule), a `locales/<code>.toml`
//! and its entry in [`Lang::source`], and map its locales in [`Lang::from_locale`].

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod fonts;

/// Look up a UI string by key: `t!("menu.file.open")` gives a `&'static str`, and
/// `t!("status.opened", name = doc.title())` fills in named placeholders and gives a `String`.
#[macro_export]
macro_rules! t {
    ($key:literal $(,)?) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::tr_args(
            $key,
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

/// Look up a UI string with a count, choosing its plural form: `tn!("status.atoms", n)`, or
/// with more placeholders `tn!("scan.steps", n, name = x)`. The count fills `{n}`.
#[macro_export]
macro_rules! tn {
    ($key:literal, $n:expr $(,)?) => {
        $crate::i18n::tr_plural($key, ($n) as i64, &[])
    };
    ($key:literal, $n:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::tr_plural(
            $key,
            ($n) as i64,
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

/// A UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    Ja,
    #[default]
    En,
    ZhHans,
    ZhHant,
    Es,
    Fr,
    De,
    PtBr,
    Ko,
    Ru,
    It,
}

impl Lang {
    /// Every language, in the order the language menu lists them.
    pub const ALL: [Lang; 11] = [
        Lang::Ja,
        Lang::En,
        Lang::ZhHans,
        Lang::ZhHant,
        Lang::Es,
        Lang::Fr,
        Lang::De,
        Lang::PtBr,
        Lang::Ko,
        Lang::Ru,
        Lang::It,
    ];

    /// The BCP-47 tag, which is also the catalog's file name and the saved setting.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
            Lang::ZhHans => "zh-Hans",
            Lang::ZhHant => "zh-Hant",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::PtBr => "pt-BR",
            Lang::Ko => "ko",
            Lang::Ru => "ru",
            Lang::It => "it",
        }
    }

    /// The language's name in itself, as the language menu shows it (never translated).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ja => "日本語", // i18n-allow: a language's own name
            Lang::En => "English",
            Lang::ZhHans => "简体中文", // i18n-allow: a language's own name
            Lang::ZhHant => "繁體中文", // i18n-allow: a language's own name
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::PtBr => "Português (Brasil)",
            Lang::Ko => "한국어",
            Lang::Ru => "Русский",
            Lang::It => "Italiano",
        }
    }

    /// The language of a saved setting or catalog name (exact tag, case-insensitive).
    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL
            .into_iter()
            .find(|l| l.code().eq_ignore_ascii_case(code))
    }

    /// The UI language for an OS locale such as `ja-JP`, `zh_TW`, `pt-BR` or `en-US`:
    /// Chinese by script or region (Simplified for zh-CN/zh-SG and plain zh, Traditional for
    /// zh-TW/zh-HK/zh-MO), any Portuguese as Brazilian, and English for anything unsupported.
    pub fn from_locale(locale: &str) -> Lang {
        // `ja_JP.UTF-8`, `de_DE@euro` (POSIX) → `ja-JP`, `de-DE`.
        let tag = locale
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .replace('_', "-");
        let mut parts = tag.split('-').filter(|p| !p.is_empty());
        let language = parts.next().unwrap_or_default().to_ascii_lowercase();
        let rest: Vec<String> = parts.map(|p| p.to_ascii_lowercase()).collect();
        let has = |s: &str| rest.iter().any(|p| p == s);
        match language.as_str() {
            "ja" => Lang::Ja,
            "en" => Lang::En,
            // The script, when given, wins over the region (`zh-Hans-HK` is Simplified).
            "zh" if has("hans") => Lang::ZhHans,
            "zh" if has("hant") || has("tw") || has("hk") || has("mo") => Lang::ZhHant,
            "zh" => Lang::ZhHans,
            "es" => Lang::Es,
            "fr" => Lang::Fr,
            "de" => Lang::De,
            "pt" => Lang::PtBr,
            "ko" => Lang::Ko,
            "ru" => Lang::Ru,
            "it" => Lang::It,
            _ => Lang::En,
        }
    }

    /// The UI language for the OS's locale (English when it cannot be read).
    pub fn system() -> Lang {
        sys_locale::get_locale()
            .map(|l| Lang::from_locale(&l))
            .unwrap_or(Lang::En)
    }

    /// The catalog's TOML source.
    fn source(self) -> &'static str {
        match self {
            Lang::Ja => include_str!("../../locales/ja.toml"),
            Lang::En => include_str!("../../locales/en.toml"),
            Lang::ZhHans => include_str!("../../locales/zh-Hans.toml"),
            Lang::ZhHant => include_str!("../../locales/zh-Hant.toml"),
            Lang::Es => include_str!("../../locales/es.toml"),
            Lang::Fr => include_str!("../../locales/fr.toml"),
            Lang::De => include_str!("../../locales/de.toml"),
            Lang::PtBr => include_str!("../../locales/pt-BR.toml"),
            Lang::Ko => include_str!("../../locales/ko.toml"),
            Lang::Ru => include_str!("../../locales/ru.toml"),
            Lang::It => include_str!("../../locales/it.toml"),
        }
    }

    fn index(self) -> usize {
        Lang::ALL.iter().position(|&l| l == self).unwrap_or(1)
    }

    /// The CLDR plural category of `n` (integers only).
    pub fn plural_category(self, n: i64) -> &'static str {
        let n = n.unsigned_abs();
        match self {
            Lang::Ja | Lang::ZhHans | Lang::ZhHant | Lang::Ko => "other",
            Lang::Fr | Lang::PtBr => {
                if n <= 1 {
                    "one"
                } else if n != 0 && n % 1_000_000 == 0 {
                    "many"
                } else {
                    "other"
                }
            }
            Lang::Es | Lang::It => {
                if n == 1 {
                    "one"
                } else if n != 0 && n % 1_000_000 == 0 {
                    "many"
                } else {
                    "other"
                }
            }
            Lang::Ru => {
                let (m10, m100) = (n % 10, n % 100);
                if m10 == 1 && m100 != 11 {
                    "one"
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    "few"
                } else {
                    "many"
                }
            }
            Lang::En | Lang::De => {
                if n == 1 {
                    "one"
                } else {
                    "other"
                }
            }
        }
    }
}

impl Serialize for Lang {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let code = String::deserialize(d)?;
        Lang::from_code(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown language {code:?}")))
    }
}

/// For a saved `Option<Lang>`: a language this version does not know reads as unset (the OS
/// language) rather than failing the whole saved state.
pub fn deserialize_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Lang>, D::Error> {
    let code: Option<String> = Option::deserialize(d)?;
    Ok(code.and_then(|c| Lang::from_code(&c)))
}

// ─── The current language ────────────────────────────────────────────────────

static CURRENT: AtomicU8 = AtomicU8::new(1); // Lang::En

thread_local! {
    /// A per-thread language, set by [`ScopedLanguage`] (tests run in parallel threads, so they
    /// must not change each other's language).
    static OVERRIDE: Cell<Option<Lang>> = const { Cell::new(None) };
}

/// The language the UI is shown in.
pub fn language() -> Lang {
    OVERRIDE
        .with(Cell::get)
        .unwrap_or_else(|| Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize % Lang::ALL.len()])
}

/// Show the UI in `lang` from the next lookup on. Called by the app (see
/// `OpenDrawApp::apply_language`), which also switches the library's language and the fonts.
///
/// In this crate's own unit tests, it changes only the calling thread's language.
pub fn set_language(lang: Lang) {
    if cfg!(test) {
        OVERRIDE.with(|o| o.set(Some(lang)));
    } else {
        CURRENT.store(lang.index() as u8, Ordering::Relaxed);
    }
}

/// Sets the calling thread's language until dropped (for tests).
#[doc(hidden)]
pub struct ScopedLanguage(Option<Lang>);

impl ScopedLanguage {
    pub fn new(lang: Lang) -> Self {
        ScopedLanguage(OVERRIDE.with(|o| o.replace(Some(lang))))
    }
}

impl Drop for ScopedLanguage {
    fn drop(&mut self) {
        let previous = self.0;
        OVERRIDE.with(|o| o.set(previous));
    }
}

// ─── Catalogs ────────────────────────────────────────────────────────────────

/// One catalog entry: a text, or its plural forms by CLDR category.
#[derive(Debug, Clone)]
pub enum Entry {
    Text(&'static str),
    Plural(Vec<(&'static str, &'static str)>),
}

impl Entry {
    /// Every text of the entry (each plural form).
    pub fn texts(&self) -> Vec<&'static str> {
        match self {
            Entry::Text(s) => vec![s],
            Entry::Plural(forms) => forms.iter().map(|(_, s)| *s).collect(),
        }
    }

    fn form(&self, category: &str) -> &'static str {
        match self {
            Entry::Text(s) => s,
            Entry::Plural(forms) => forms
                .iter()
                .find(|(c, _)| *c == category)
                .or_else(|| forms.iter().find(|(c, _)| *c == "other"))
                .map(|(_, s)| *s)
                .unwrap_or(""),
        }
    }
}

pub type Catalog = HashMap<&'static str, Entry>;

const PLURAL_CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// Parse a catalog's TOML into its flat keys. Strings are leaked: catalogs live for the
/// process, and lookups then hand out `&'static str`.
pub fn parse_catalog(source: &str) -> Result<Catalog, String> {
    let table: toml::Table = toml::from_str(source).map_err(|e| e.to_string())?;
    let mut out = Catalog::new();
    flatten(&table, "", &mut out)?;
    Ok(out)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn is_plural_table(table: &toml::Table) -> bool {
    !table.is_empty()
        && table.contains_key("other")
        && table
            .iter()
            .all(|(k, v)| PLURAL_CATEGORIES.contains(&k.as_str()) && v.is_str())
}

fn flatten(table: &toml::Table, prefix: &str, out: &mut Catalog) -> Result<(), String> {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::String(s) => {
                out.insert(leak(&path), Entry::Text(leak(s)));
            }
            toml::Value::Table(t) if is_plural_table(t) => {
                let forms = PLURAL_CATEGORIES
                    .iter()
                    .filter_map(|c| t.get(*c).and_then(|v| v.as_str()).map(|s| (*c, leak(s))))
                    .collect();
                out.insert(leak(&path), Entry::Plural(forms));
            }
            toml::Value::Table(t) => flatten(t, &path, out)?,
            _ => return Err(format!("{path}: expected a string or a table")),
        }
    }
    Ok(())
}

/// The parsed catalog of `lang` (empty if it does not parse; a test makes sure they all do).
pub fn catalog(lang: Lang) -> &'static Catalog {
    static CATALOGS: OnceLock<Vec<Catalog>> = OnceLock::new();
    let all = CATALOGS.get_or_init(|| {
        Lang::ALL
            .iter()
            .map(|l| {
                parse_catalog(l.source()).unwrap_or_else(|e| {
                    eprintln!("locales/{}.toml: {e}", l.code());
                    Catalog::new()
                })
            })
            .collect()
    });
    &all[lang.index()]
}

fn lookup(key: &str) -> Option<&'static Entry> {
    let lang = language();
    catalog(lang)
        .get(key)
        .filter(|e| !e.texts().iter().all(|s| s.is_empty()))
        .or_else(|| catalog(Lang::En).get(key))
}

// ─── Lookups (used through `t!` and `tn!`) ────────────────────────────────────

/// The text of `key` in the current language, else in English, else the key itself.
pub fn tr(key: &'static str) -> &'static str {
    match lookup(key) {
        Some(entry) => entry.form("other"),
        None => key,
    }
}

/// [`tr`] with its `{name}` placeholders filled in.
pub fn tr_args(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    format_text(tr(key), args)
}

/// The plural form of `key` for `n`, with `{n}` and the other placeholders filled in.
pub fn tr_plural(key: &'static str, n: i64, args: &[(&str, &dyn Display)]) -> String {
    let lang = language();
    let text = match catalog(lang).get(key) {
        Some(entry) if !entry.texts().iter().all(|s| s.is_empty()) => {
            entry.form(lang.plural_category(n))
        }
        _ => match catalog(Lang::En).get(key) {
            Some(entry) => entry.form(Lang::En.plural_category(n)),
            None => key,
        },
    };
    let mut all: Vec<(&str, &dyn Display)> = Vec::with_capacity(args.len() + 1);
    all.push(("n", &n));
    all.extend_from_slice(args);
    format_text(text, &all)
}

/// Fill `{name}` placeholders from `args`; `{{` and `}}` are literal braces, and a placeholder
/// without an argument is left as it is.
pub fn format_text(text: &str, args: &[(&str, &dyn Display)]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let name = &tail[1..end];
            if let Some((_, value)) = args.iter().find(|(n, _)| *n == name) {
                let _ = write!(out, "{value}");
            } else {
                out.push_str(&tail[..=end]);
            }
            rest = &tail[end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// The placeholder names in `text` (`{name}`), sorted and without repeats.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            names.push(tail[1..end].to_owned());
            rest = &tail[end + 1..];
            continue;
        }
        rest = &tail[1..];
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests;
