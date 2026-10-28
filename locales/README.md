# UI translations

One TOML file per language, embedded into the app at build time:

| File | Language | Plural categories |
|---|---|---|
| `en.toml` | English (reference, complete) | one, other |
| `ja.toml` | 日本語 (complete) | other |
| `zh-Hans.toml` | 简体中文 | other |
| `zh-Hant.toml` | 繁體中文 | other |
| `ko.toml` | 한국어 | other |
| `es.toml` | Español | one, many, other |
| `fr.toml` | Français | one (0 and 1), many, other |
| `de.toml` | Deutsch | one, other |
| `pt-BR.toml` | Português (Brasil) | one (0 and 1), many, other |
| `it.toml` | Italiano | one, many, other |
| `ru.toml` | Русский | one, few, many |

`many` in es/fr/pt-BR/it is only for exact millions; you can leave it out (`other` is used).

## Translating

- Work from `en.toml`: copy a section, keep every key (the left of `=`), translate the text.
  A key that is missing or empty (`""`) shows the English text, so a partial file is fine.
- Comments (`# …`) above a key say what the text labels or what a placeholder holds.
- **Placeholders**: `{name}` is filled in by the app (a file name, a number, …). Keep each one
  (you may move it); do not translate the word inside the braces. `{{` and `}}` print `{` `}`.
- **Plurals**: a text with a count is a table of forms by CLDR category, with `{n}` the count:

  ```toml
  atoms = { one = "{n} atom", other = "{n} atoms" }          # en
  atoms = { one = "{n} атом", few = "{n} атома", many = "{n} атомов", other = "{n} атома" }  # ru
  atoms = "{n} 個の原子"                                       # ja: one form is enough
  ```

  Give the categories your language uses (table above); `other` is always required.
- Do not translate chemistry notation (element symbols, formulas, SMILES, units), file format
  names, keyboard shortcuts (`Ctrl+S`), or product names.
- Keep `language.menu` recognisable to people who cannot read your language, e.g.
  `"Sprache (Language)"`.

`cargo test i18n` checks that every file parses, that its keys exist in `en.toml` and that its
placeholders match.

## Adding a language

Add a variant to `Lang` in `src/i18n/mod.rs` (its code, native name, plural rule, catalog in
`Lang::source`, locales in `Lang::from_locale`, and a CJK font locale in
`src/i18n/fonts.rs` if it needs one), then create `locales/<code>.toml`.
