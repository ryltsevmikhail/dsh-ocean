//! Embedded Ocean messages, independent of the host DSH locale registry.
//! Messages and arguments are plain text: consumers must use GTK/textContent,
//! not interpolate this output into HTML, JavaScript or Pango markup.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Ru,
    Es,
    PtBr,
    De,
    Fr,
    Ja,
    Ko,
}

static ACTIVE_LANGUAGE: OnceLock<Language> = OnceLock::new();

/// GUI startup sets a single snapshot before constructing any native controls.
/// Settings changes are persisted for the next start, not partially applied.
pub fn initialize(language: Language) -> Result<(), Language> {
    ACTIVE_LANGUAGE.set(language)
}
pub fn active_language() -> Language {
    ACTIVE_LANGUAGE.get().copied().unwrap_or(Language::En)
}
pub fn text(key: &'static str) -> &'static str {
    message(active_language(), key).unwrap_or(key)
}

pub const LANGUAGES: [(Language, &str); 8] = [
    (Language::En, "English"),
    (Language::Ru, "Русский"),
    (Language::Es, "Español"),
    (Language::PtBr, "Português (Brasil)"),
    (Language::De, "Deutsch"),
    (Language::Fr, "Français"),
    (Language::Ja, "日本語"),
    (Language::Ko, "한국어"),
];

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
            Self::Es => "es",
            Self::PtBr => "pt-BR",
            Self::De => "de",
            Self::Fr => "fr",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }

    /// Normalize an OS locale without changing the process locale/environment.
    pub fn from_locale(value: &str) -> Option<Self> {
        let tag = value.trim().split(['.', '@']).next()?.replace('_', "-");
        if tag.is_empty() || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return None;
        }
        match tag.split('-').next()?.to_ascii_lowercase().as_str() {
            "en" | "c" | "posix" => Some(Self::En),
            "ru" => Some(Self::Ru),
            "es" => Some(Self::Es),
            "pt" => Some(Self::PtBr),
            "de" => Some(Self::De),
            "fr" => Some(Self::Fr),
            "ja" => Some(Self::Ja),
            "ko" => Some(Self::Ko),
            _ => None,
        }
    }
}

/// Explicit preference wins; system mode follows POSIX message-locale priority.
/// An unsupported highest-priority locale falls back to English, not a lower
/// priority variable. Inputs are injected so tests never mutate global env.
pub fn resolve_language(
    preference: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Language {
    let selected = match preference {
        Some(value) if value != "system" => Some(value),
        _ => [lc_all, lc_messages, lang]
            .into_iter()
            .flatten()
            .find(|v| !v.trim().is_empty()),
    };
    selected
        .and_then(Language::from_locale)
        .unwrap_or(Language::En)
}

type Messages = BTreeMap<String, String>;
fn catalogs() -> &'static BTreeMap<&'static str, Messages> {
    static CATALOGS: OnceLock<BTreeMap<&'static str, Messages>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        [
            ("en", include_str!("../locales/ocean/en.json")),
            ("ru", include_str!("../locales/ocean/ru.json")),
            ("es", include_str!("../locales/ocean/es.json")),
            ("pt-BR", include_str!("../locales/ocean/pt-BR.json")),
            ("de", include_str!("../locales/ocean/de.json")),
            ("fr", include_str!("../locales/ocean/fr.json")),
            ("ja", include_str!("../locales/ocean/ja.json")),
            ("ko", include_str!("../locales/ocean/ko.json")),
        ]
        .into_iter()
        .map(|(code, json)| {
            (
                code,
                serde_json::from_str(json).expect("validated embedded Ocean catalog"),
            )
        })
        .collect()
    })
}

fn lookup<'a>(selected: &'a Messages, english: &'a Messages, key: &str) -> Option<&'a str> {
    selected
        .get(key)
        .or_else(|| english.get(key))
        .map(String::as_str)
}

/// Plain IPC data for owned local pages; no code generation or HTML markup.
pub fn messages(language: Language) -> BTreeMap<String, String> {
    let catalogs = catalogs();
    let mut messages = catalogs["en"].clone();
    messages.extend(catalogs[language.code()].clone());
    messages
}

pub fn message(language: Language, key: &str) -> Option<&'static str> {
    let catalogs = catalogs();
    lookup(&catalogs[language.code()], &catalogs["en"], key)
}

/// Named placeholders are inserted exactly once. Argument text is never parsed
/// as a template or markup. Reject missing, duplicate and extra arguments.
pub fn render(
    language: Language,
    key: &str,
    arguments: &[(&str, &str)],
) -> Result<String, &'static str> {
    let template = message(language, key).ok_or("unknown message")?;
    render_template(template, arguments)
}

fn render_template(template: &str, arguments: &[(&str, &str)]) -> Result<String, &'static str> {
    let args: BTreeMap<_, _> = arguments.iter().copied().collect();
    if args.len() != arguments.len() {
        return Err("duplicate message argument");
    }
    let mut used = BTreeSet::new();
    let mut output = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                let mut bytes = name.bytes();
                if !closed
                    || !bytes.next().is_some_and(|b| b.is_ascii_lowercase())
                    || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    return Err("invalid named placeholder");
                }
                output.push_str(args.get(name.as_str()).ok_or("missing message argument")?);
                used.insert(name);
            }
            '}' => return Err("unmatched closing brace"),
            _ => output.push(c),
        }
    }
    if used.len() != args.len() {
        return Err("unexpected message argument");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_selected_key_falls_back_to_english() {
        let selected = Messages::new();
        let english = Messages::from([("key".into(), "English".into())]);
        assert_eq!(lookup(&selected, &english, "key"), Some("English"));
        assert_eq!(lookup(&selected, &english, "unknown"), None);
    }

    #[test]
    fn escaped_braces_and_reordered_arguments_have_no_recursive_expansion() {
        assert_eq!(
            render_template(
                "{{literal}} {to} ← {from}",
                &[("from", "{to}"), ("to", "B")]
            )
            .unwrap(),
            "{literal} B ← {to}"
        );
        for invalid in ["{", "}", "{x.y}", "{x!r}", "{x:>10}", "{x:}"] {
            assert!(render_template(invalid, &[("x", "value")]).is_err());
        }
    }
}
