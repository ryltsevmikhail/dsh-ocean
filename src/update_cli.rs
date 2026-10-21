//! Headless native-updater configuration/check commands. No WebView invoke API.
use crate::release_feed::{Channel, Kind};
use crate::update_settings::{self, Confirmation, Product};
use anyhow::{Context, Result};
use serde_json::Value;

pub fn run(args: &[String]) -> Result<Value> {
    match args.first().map(String::as_str) {
        Some("--update-settings") => {
            anyhow::ensure!(args.len() == 1, "usage: --update-settings");
            update_settings::load()?.summary()
        }
        Some("--set-update-channel") => {
            anyhow::ensure!(
                args.len() == 3,
                "usage: --set-update-channel shell|runtime stable|next"
            );
            let product = Product::parse(&args[1])?;
            let channel = match args[2].as_str() {
                "stable" => Channel::Stable,
                "next" => Channel::Next,
                _ => anyhow::bail!("signed update channel must be stable or next"),
            };
            update_settings::set_channel(product, channel)?.summary()
        }
        Some("--configure-updates") => {
            anyhow::ensure!(args.len()>=2,"usage: --configure-updates CONFIG --confirm-shell-key SHA256 [--confirm-runtime-key SHA256] [--confirm-replace-trust yes]");
            let mut flags = std::collections::HashMap::new();
            let (pairs, rest) = args[2..].as_chunks::<2>();
            anyhow::ensure!(
                rest.is_empty(),
                "update configuration option requires a value"
            );
            for pair in pairs {
                anyhow::ensure!(
                    matches!(
                        pair[0].as_str(),
                        "--confirm-shell-key" | "--confirm-runtime-key" | "--confirm-replace-trust"
                    ),
                    "unknown update configuration option"
                );
                anyhow::ensure!(
                    flags.insert(pair[0].as_str(), pair[1].as_str()).is_none(),
                    "duplicate update configuration option"
                );
            }
            let replace = match flags.get("--confirm-replace-trust").copied() {
                None => false,
                Some("yes") => true,
                _ => anyhow::bail!("trust replacement requires --confirm-replace-trust yes"),
            };
            let confirmation = Confirmation {
                shell: flags.get("--confirm-shell-key").map(|s| (*s).to_owned()),
                runtime: flags.get("--confirm-runtime-key").map(|s| (*s).to_owned()),
                replace,
            };
            let candidate = update_settings::read_candidate(std::path::Path::new(&args[1]))?;
            update_settings::configure(candidate, &confirmation)?.summary()
        }
        Some("--check-shell-update") => {
            let kind = if args.len() == 1 {
                Kind::AppImage
            } else {
                anyhow::ensure!(
                    args.len() == 3 && args[1] == "--kind",
                    "usage: --check-shell-update [--kind appimage|tar.gz]"
                );
                match args[2].as_str() {
                    "appimage" => Kind::AppImage,
                    "tar.gz" => Kind::TarGz,
                    _ => anyhow::bail!("unsupported shell artifact kind"),
                }
            };
            let settings = update_settings::load().context("load updater settings")?;
            crate::update_feed::check_shell(&settings, kind)?.summary()
        }
        _ => anyhow::bail!("unknown native updater command"),
    }
}
