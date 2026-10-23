use dsh_ocean::{
    bootstrap::Runtime,
    builtin_locale_consent as consent,
    builtin_locale_planner::{self, LaunchScope},
    runtime_lifecycle::{Lifecycle, Operation},
    service::Server,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[tokio::test]
async fn approval_revoked_during_metadata_inspection_cannot_activate_overlay() {
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-revoke-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("sdk")).unwrap();
    fs::write(root.join("sdk/bin.js"), "").unwrap();
    fs::write(
        root.join("sdk/package.json"),
        r#"{"name":"@deepseek-ai/dsh","version":"0.2.0-rc.2"}"#,
    )
    .unwrap();
    let launch = LaunchScope::checked(root.clone(), None, root.clone()).unwrap();
    let scope = consent::Scope::new(&launch.dsh_home.join("profiles/web"), "0.2.0-rc.2").unwrap();
    let grant = root.join("approval.json");
    consent::record(&root.join("refused.json"), &scope, false).unwrap();
    consent::record(&grant, &scope, true).unwrap();
    let node = root.join("fake-node");
    fs::write(&node,"#!/bin/sh\n/bin/cp \"$HOME/refused.json\" \"$HOME/approval.json\"\nprintf '%s' '{\"state\":\"absent\"}'\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = Runtime {
        node,
        dsh: root.join("sdk/bin.js"),
        version: String::new(),
        install_root: None,
    };
    let lifecycle = Lifecycle::<Server, tokio::process::Child>::new(|_| {});
    let permit = lifecycle.try_begin(Operation::Update).unwrap();
    let cache = root.join("builtin-locales");
    let result =
        builtin_locale_planner::recorded_plan(&runtime, &permit, &launch, &cache, &grant).await;
    assert_eq!(consent::load(&grant, &scope).unwrap(), Some(false));
    assert!(!launch.dsh_home.exists());
    let tree = cache.join(dsh_ocean::builtin_locales::bundle_id());
    fs::set_permissions(tree.join("lib"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700)).unwrap();
    let checked = root.canonicalize().unwrap();
    assert!(
        checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-revoke-")
    );
    fs::remove_dir_all(checked).unwrap();
    assert_eq!(result.unwrap(), None::<PathBuf>);
}
