use dsh_ocean::builtin_locales::{bundle_id, resources};
#[test]
fn builtin_locale_tree_is_embedded_complete_and_has_stable_content_identity() {
    let files = resources();
    assert_eq!(
        files.iter().map(|x| x.path).collect::<Vec<_>>(),
        vec![
            "LICENSE",
            "builtin-locale-preflight.mjs",
            "builtin-locale-sdk-preflight.mjs",
            "lib/client.js",
            "lib/index.js",
            "overlay.yml",
            "package.json"
        ]
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        files
            .iter()
            .find(|x| x.path == "package.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert_eq!(manifest["name"], "@dsh-ocean/builtin-locales");
    assert_eq!(manifest["dsh"]["client"]["platform"], "web");
    assert_eq!(manifest["exports"]["./client"], "./lib/client.js");
    assert!(manifest.get("dependencies").is_none());
    assert!(manifest.get("scripts").is_none());
    let client = std::str::from_utf8(
        files
            .iter()
            .find(|x| x.path == "lib/client.js")
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert!(client.contains("window.__ModuleLoader__.load"));
    for code in ["ru", "es", "pt-BR", "de", "fr", "ja", "ko"] {
        assert!(client.contains(&format!("\"{code}\":")));
    }
    let license = std::str::from_utf8(files[0].bytes).unwrap();
    assert!(license.contains("DeepSeek"));
    assert!(license.contains("MIT License"));
    let id = bundle_id();
    assert_eq!(id.len(), 64);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(id, bundle_id());
}
#[test]
fn embedded_manifest_covers_the_shipped_and_installed_sdk_contracts() {
    let resource = dsh_ocean::builtin_locales::resources()
        .iter()
        .find(|file| file.path == "package.json")
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(resource.bytes).unwrap();
    let expected = "0.2.0-rc.2 || 0.2.0-rc.2.ocean.2 || 0.2.1-rc.2";
    assert_eq!(manifest["engines"]["dsh"], expected);
    for package in [
        "@deepseek-ai/dsh-client-locale",
        "@deepseek-ai/dsh-client-modules",
    ] {
        assert_eq!(manifest["peerDependencies"][package], expected);
    }
}
