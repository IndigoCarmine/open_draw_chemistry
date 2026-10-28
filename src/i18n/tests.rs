use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;

/// Every `.rs` file under `src/`.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out.sort();
    out
}

/// The keys passed to `t!` and `tn!` in the sources, with whether each was a `tn!`.
fn used_keys() -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    // The i18n module's own docs and tests show made-up keys.
    let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("i18n");
    for path in sources().into_iter().filter(|p| !p.starts_with(&own)) {
        let text: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .flat_map(|l| [l, "\n"])
            .collect();
        for (macro_name, plural) in [("t!(", false), ("tn!(", true)] {
            let mut rest = text.as_str();
            while let Some(i) = rest.find(macro_name) {
                let before = rest[..i].chars().next_back();
                rest = &rest[i + macro_name.len()..];
                // `tn!(` also contains `t!(` preceded by `n`; skip identifiers ending in t.
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let args = rest.trim_start();
                if let Some(args) = args.strip_prefix('"')
                    && let Some(end) = args.find('"')
                {
                    out.push((args[..end].to_owned(), plural, path.display().to_string()));
                }
            }
        }
    }
    out
}

#[test]
fn every_catalog_parses() {
    for lang in Lang::ALL {
        parse_catalog(lang.source()).unwrap_or_else(|e| panic!("{}: {e}", lang.code()));
    }
}

#[test]
fn every_key_used_in_code_is_in_english() {
    let en = catalog(Lang::En);
    let mut missing = BTreeSet::new();
    for (key, plural, file) in used_keys() {
        match en.get(key.as_str()) {
            None => {
                missing.insert(format!("{key} ({file})"));
            }
            Some(Entry::Text(_)) if plural => {
                missing.insert(format!("{key} is used with tn! but has no plural forms ({file})"));
            }
            Some(_) => {}
        }
    }
    assert!(missing.is_empty(), "keys missing from en.toml:\n{}", Vec::from_iter(missing).join("\n"));
}

#[test]
fn every_english_key_is_used() {
    let used: BTreeSet<String> = used_keys().into_iter().map(|(k, _, _)| k).collect();
    // Keys looked up through a variable rather than a literal are listed here.
    let unused: Vec<&str> = catalog(Lang::En)
        .keys()
        .copied()
        .filter(|k| !used.contains(*k))
        .collect();
    assert!(unused.is_empty(), "keys in en.toml that no code uses: {unused:?}");
}

#[test]
fn japanese_is_complete() {
    let en = catalog(Lang::En);
    let ja = catalog(Lang::Ja);
    let mut missing: Vec<&str> = en.keys().copied().filter(|k| !ja.contains_key(k)).collect();
    missing.sort();
    assert!(missing.is_empty(), "keys missing from ja.toml: {missing:?}");
}

#[test]
fn every_catalog_matches_english_keys_and_placeholders() {
    let en = catalog(Lang::En);
    for lang in Lang::ALL {
        for (key, entry) in catalog(lang) {
            let Some(reference) = en.get(key) else {
                panic!("{}: {key} is not in en.toml", lang.code());
            };
            let expected: BTreeSet<String> =
                reference.texts().iter().flat_map(|t| placeholders(t)).collect();
            for text in entry.texts() {
                if text.is_empty() {
                    continue; // Untranslated: falls back to English.
                }
                let found: BTreeSet<String> = placeholders(text).into_iter().collect();
                // A plural form may leave out `{n}` ("one file"), never add an unknown name.
                let unknown: Vec<_> = found.difference(&expected).collect();
                assert!(unknown.is_empty(), "{}: {key}: unknown placeholders {unknown:?} in {text:?}", lang.code());
                let lost: Vec<_> = expected
                    .difference(&found)
                    .filter(|n| !(n.as_str() == "n" && matches!(reference, Entry::Plural(_))))
                    .collect();
                assert!(lost.is_empty(), "{}: {key}: placeholders {lost:?} missing from {text:?}", lang.code());
            }
            if let Entry::Plural(forms) = entry {
                assert!(forms.iter().any(|(c, _)| *c == "other"), "{}: {key} has no `other` form", lang.code());
            }
        }
    }
}

/// Japanese (kana, CJK ideographs and punctuation, full-width forms) in a string literal.
fn has_japanese(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{3000}'..='\u{30FF}' // CJK punctuation, hiragana, katakana
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{FF00}'..='\u{FFEF}') // full-width forms
    })
}

/// The string literals of a line of code (outside a trailing `//` comment), and whether the
/// last one goes on to the next line.
fn string_literals(line: &str) -> (Vec<&str>, bool) {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut start = None;
    let mut escaped = false;
    while let Some((i, c)) = chars.next() {
        match start {
            None => {
                if c == '/' && chars.peek().is_some_and(|&(_, n)| n == '/') {
                    break;
                }
                if c == '"' {
                    start = Some(i + 1);
                }
                // A char literal such as '"' would otherwise open a string.
                if c == '\'' {
                    let rest = &line[i + 1..];
                    if let Some(end) = rest.find('\'')
                        && end <= 4
                    {
                        for _ in 0..rest[..=end].chars().count() {
                            chars.next();
                        }
                    }
                }
            }
            Some(s) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    out.push(&line[s..i]);
                    start = None;
                }
            }
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]); // continues on the next line
    }
    (out, start.is_some())
}

/// The guard against UI text that skipped translation: no Japanese in string literals of the
/// app's code. Allowed are comments, test modules (from `#[cfg(test)]` on, which this crate
/// keeps at the end of each file) and lines marked `i18n-allow` with a reason (a language's own
/// name, CLI output, a sample of text in the font menu, …).
#[test]
fn no_japanese_literals_outside_catalogs() {
    let mut found = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).unwrap();
        let mut in_string = false;
        // Inside a `#[cfg(test)]` item: its brace depth, once it has opened.
        let mut test_item: Option<Option<i32>> = None;
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(test)]") {
                test_item = Some(None);
                continue;
            }
            if let Some(depth) = test_item {
                let delta = line.matches('{').count() as i32 - line.matches('}').count() as i32;
                let depth = match depth {
                    None if line.contains('{') => delta,
                    None if trimmed.ends_with(';') => 0,
                    None => {
                        continue; // attributes, the item's signature
                    }
                    Some(d) => d + delta,
                };
                test_item = (depth > 0).then_some(Some(depth));
                continue;
            }
            if !in_string && trimmed.starts_with("//") {
                continue;
            }
            let scan = if in_string { format!("\"{line}") } else { line.to_owned() };
            let (literals, open) = string_literals(&scan);
            in_string = open;
            if line.contains("i18n-allow") {
                continue;
            }
            if literals.iter().any(|l| has_japanese(l)) {
                found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(found.is_empty(), "Japanese text outside the catalogs (use t!):\n{}", found.join("\n"));
}

#[test]
fn locales_map_to_ui_languages() {
    let cases = [
        ("ja-JP", Lang::Ja),
        ("ja", Lang::Ja),
        ("en-US", Lang::En),
        ("en_GB.UTF-8", Lang::En),
        ("zh-CN", Lang::ZhHans),
        ("zh-SG", Lang::ZhHans),
        ("zh", Lang::ZhHans),
        ("zh-Hans-HK", Lang::ZhHans),
        ("zh-TW", Lang::ZhHant),
        ("zh_HK", Lang::ZhHant),
        ("zh-MO", Lang::ZhHant),
        ("zh-Hant", Lang::ZhHant),
        ("es-MX", Lang::Es),
        ("fr-CA", Lang::Fr),
        ("de-AT", Lang::De),
        ("de_DE@euro", Lang::De),
        ("pt-BR", Lang::PtBr),
        ("pt-PT", Lang::PtBr),
        ("ko-KR", Lang::Ko),
        ("ru-RU", Lang::Ru),
        ("it-IT", Lang::It),
        ("nl-NL", Lang::En),
        ("C", Lang::En),
        ("", Lang::En),
    ];
    for (locale, lang) in cases {
        assert_eq!(Lang::from_locale(locale), lang, "{locale}");
    }
}

#[test]
fn codes_round_trip() {
    for lang in Lang::ALL {
        assert_eq!(Lang::from_code(lang.code()), Some(lang));
        assert_eq!(Lang::from_locale(lang.code()), lang);
    }
    assert_eq!(Lang::from_code("xx"), None);
}

#[test]
fn placeholders_are_filled_and_braces_escaped() {
    let name = "benzene";
    assert_eq!(
        format_text("Opened {name} ({count})", &[("name", &name), ("count", &3)]),
        "Opened benzene (3)"
    );
    assert_eq!(format_text("{{x}} {missing}", &[]), "{x} {missing}");
    assert_eq!(placeholders("{a} {{b}} {c} {a}"), ["a", "c"]);
}

#[test]
fn plural_rules() {
    assert_eq!(Lang::En.plural_category(1), "one");
    assert_eq!(Lang::En.plural_category(0), "other");
    assert_eq!(Lang::Fr.plural_category(0), "one");
    assert_eq!(Lang::Ru.plural_category(21), "one");
    assert_eq!(Lang::Ru.plural_category(3), "few");
    assert_eq!(Lang::Ru.plural_category(12), "many");
    assert_eq!(Lang::Ja.plural_category(1), "other");
}

#[test]
fn lookups_fall_back_to_english_then_the_key() {
    let _ja = ScopedLanguage::new(Lang::Ja);
    assert_eq!(tr("common.cancel"), "キャンセル"); // i18n-allow: test
    {
        let _de = ScopedLanguage::new(Lang::De);
        assert_eq!(tr("common.cancel"), "Abbrechen");
    }
    assert_eq!(language(), Lang::Ja);
    assert_eq!(tr("no.such.key"), "no.such.key");
}

#[test]
fn plural_lookup_chooses_the_form() {
    let catalog = parse_catalog(r#"x = { one = "{n} atom", other = "{n} atoms" }"#).unwrap();
    let Some(entry @ Entry::Plural(_)) = catalog.get("x") else {
        panic!("not plural");
    };
    assert_eq!(entry.form(Lang::En.plural_category(1)), "{n} atom");
    assert_eq!(entry.form(Lang::En.plural_category(2)), "{n} atoms");
    // A form the language lacks falls back to `other`.
    assert_eq!(entry.form("few"), "{n} atoms");
}
