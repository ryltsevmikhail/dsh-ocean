//! Ocean-only language preference. No DSH profile or runtime is touched here.
use crate::i18n::Language;
use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::Path;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    System,
    Language(Language),
}
impl Preference {
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "system" => Some(Self::System),
            "en" => Some(Self::Language(Language::En)),
            "ru" => Some(Self::Language(Language::Ru)),
            "es" => Some(Self::Language(Language::Es)),
            "pt-BR" => Some(Self::Language(Language::PtBr)),
            "de" => Some(Self::Language(Language::De)),
            "fr" => Some(Self::Language(Language::Fr)),
            "ja" => Some(Self::Language(Language::Ja)),
            "ko" => Some(Self::Language(Language::Ko)),
            _ => None,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Language(language) => language.code(),
        }
    }
}
impl Serialize for Preference {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}
impl<'de> Deserialize<'de> for Preference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::parse(&code).ok_or_else(|| serde::de::Error::custom("unsupported Ocean language"))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    language: Preference,
}
fn validate(document: &Document) -> Result<()> {
    anyhow::ensure!(
        document.version == 1,
        "unsupported language preference version"
    );
    Ok(())
}
/// Missing preference means system; malformed/unsafe/future documents are errors.
pub fn load(path: &Path) -> Result<Preference> {
    Ok(crate::settings_file::load(path, validate)?
        .map(|d| d.language)
        .unwrap_or_default())
}
/// Atomic replacement in a pinned, owner-controlled directory. Not a security
/// boundary against concurrent external editors running as the same UID.
pub fn save(path: &Path, preference: Preference) -> Result<()> {
    crate::settings_file::save(
        path,
        &Document {
            version: 1,
            language: preference,
        },
        validate,
    )
}
