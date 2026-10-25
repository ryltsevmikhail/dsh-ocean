//! Explicit native permission for the pinned SDK-generated web root only.
//! A stored choice is NOT ownership of the profile or an attached runtime.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
const SDKS: &[&str] = &["0.2.0-rc.2", "0.2.0-rc.2.ocean.2", "0.2.1-rc.2"];
const CONTRACT: &str = "generated-cordis-root-v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    profile: PathBuf,
    sdk: String,
    contract: String,
}
impl Scope {
    pub fn new(profile: &Path, sdk: &str) -> Result<Self> {
        let scope = Self {
            profile: profile.to_path_buf(),
            sdk: sdk.into(),
            contract: CONTRACT.into(),
        };
        scope.validate()?;
        Ok(Self {
            profile: scope.profile.components().collect(),
            ..scope
        })
    }
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            SDKS.contains(&self.sdk.as_str()) && self.contract == CONTRACT,
            "unsupported generated-profile locale contract"
        );
        anyhow::ensure!(
            self.profile.is_absolute()
                && !self
                    .profile
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
                && self.profile.ends_with("profiles/web"),
            "absolute web-profile scope required"
        );
        anyhow::ensure!(
            self.profile.as_os_str().len() <= 4096,
            "profile scope exceeds size limit"
        );
        Ok(())
    }
    pub fn profile(&self) -> &Path {
        &self.profile
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    scope: Scope,
    allowed: bool,
}
fn validate(document: &Document) -> Result<()> {
    anyhow::ensure!(
        document.version == 1,
        "unsupported native locale consent version"
    );
    document.scope.validate()
}
/// None (including a different profile) never authorizes activation; false is a
/// remembered refusal. Missing reads are side-effect free.
pub fn load(path: &Path, scope: &Scope) -> Result<Option<bool>> {
    scope.validate()?;
    Ok(crate::settings_file::load(path, validate)?
        .filter(|d| d.scope == *scope)
        .map(|d| d.allowed))
}
/// Call ONLY after a real native user choice. This writes Ocean's preference,
/// not the scoped profile; it cannot grant process/unit ownership.
pub fn record(path: &Path, scope: &Scope, allowed: bool) -> Result<()> {
    crate::settings_file::save(
        path,
        &Document {
            version: 1,
            scope: scope.clone(),
            allowed,
        },
        validate,
    )
}
