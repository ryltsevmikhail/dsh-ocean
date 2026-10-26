use dsh_ocean::builtin_locale_planner::LaunchScope;
use std::path::PathBuf;
#[test]
fn packaged_regular_dsh_shim_resolves_only_its_verified_sdk_entry() {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    let root = std::env::temp_dir().join(format!(
        "ocean-packaged-locale-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bin = root.join("node_modules/.bin");
    let sdk = root.join("node_modules/@deepseek-ai/dsh");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(sdk.join("lib")).unwrap();
    fs::write(bin.join("package.json"), "{\"type\":\"commonjs\"}\n").unwrap();
    let shim = "#!/usr/bin/env node\nconst path=require('node:path');const entry=path.resolve(__dirname,\"../@deepseek-ai/dsh/lib/bin.js\");if(typeof process.execve!=='function')throw new Error('pinned Node execve required');process.execve(process.execPath,[process.execPath,entry,...process.argv.slice(2)],process.env);\n";
    fs::write(bin.join("dsh"), shim).unwrap();
    fs::write(
        sdk.join("lib/bin.js"),
        "throw new Error('must not execute');",
    )
    .unwrap();
    fs::write(sdk.join("package.json"), r#"{"name":"@deepseek-ai/dsh","version":"0.2.0-rc.2.ocean.2","bin":{"dsh":"./lib/bin.js"}}"#).unwrap();
    let mut runtime = dsh_ocean::bootstrap::Runtime {
        node: root.join("never-execute"),
        dsh: bin.join("dsh"),
        version: "0.2.0-rc.2.ocean.2".into(),
        install_root: Some(root.clone()),
    };
    assert_eq!(
        dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).unwrap(),
        (sdk.join("package.json"), runtime.version.clone())
    );
    runtime.version = "0.2.1-rc.2".into();
    assert!(dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).is_err());
    runtime.version = "0.2.0-rc.2.ocean.2".into();
    fs::write(bin.join("dsh"), format!("{shim}// foreign extension\n")).unwrap();
    assert!(dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).is_err());
    fs::write(bin.join("dsh"), shim).unwrap();
    fs::remove_file(sdk.join("lib/bin.js")).unwrap();
    assert!(dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).is_err());
    fs::remove_dir_all(&root).unwrap();
}
#[test]
fn selected_sdk_anchor_uses_executable_manifest_not_ambient_package_lookup() {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-anchor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("lib")).unwrap();
    fs::write(root.join("lib/bin.js"), "").unwrap();
    let runtime = dsh_ocean::bootstrap::Runtime {
        node: root.join("node"),
        dsh: root.join("lib/bin.js"),
        version: String::new(),
        install_root: None,
    };
    fs::write(
        root.join("package.json"),
        r#"{"name":"@deepseek-ai/dsh","version":"0.2.0-rc.2"}"#,
    )
    .unwrap();
    let (anchor, version) = dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).unwrap();
    assert_eq!(anchor, root.join("package.json"));
    assert_eq!(version, "0.2.0-rc.2");
    fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
    let mut linked = runtime.clone();
    linked.dsh = root.join("node_modules/.bin/dsh");
    std::os::unix::fs::symlink(&runtime.dsh, &linked.dsh).unwrap();
    assert_eq!(
        dsh_ocean::builtin_locale_planner::selected_sdk(&linked).unwrap(),
        (anchor.clone(), version.clone())
    );
    fs::write(
        root.join("lib/package.json"),
        r#"{"name":"foreign","version":"0.2.0-rc.2"}"#,
    )
    .unwrap();
    assert!(dsh_ocean::builtin_locale_planner::selected_sdk(&runtime).is_err());
    assert!(dsh_ocean::builtin_locale_planner::selected_sdk(&linked).is_err());
    let checked = root.canonicalize().unwrap();
    assert!(
        checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-anchor-")
    );
    fs::remove_dir_all(checked).unwrap();
}

#[test]
fn profile_alias_is_refused_without_creating_any_profile() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-scope-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let scope = LaunchScope::checked(root.clone(), None, root.clone()).unwrap();
    scope.verify().unwrap();
    assert!(!scope.dsh_home.exists());
    fs::create_dir(&scope.dsh_home).unwrap();
    std::os::unix::fs::symlink(&root, scope.dsh_home.join("profiles")).unwrap();
    assert!(scope.verify().is_err());
    let checked = root.canonicalize().unwrap();
    assert!(
        checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-scope-")
    );
    fs::remove_dir_all(checked).unwrap();
}
#[tokio::test]
async fn missing_or_refused_native_approval_and_pending_gate_never_create_locale_cache() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-admission-{}-{}",
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
    let runtime = dsh_ocean::bootstrap::Runtime {
        node: root.join("never-execute"),
        dsh: root.join("sdk/bin.js"),
        version: String::new(),
        install_root: None,
    };
    let scope = LaunchScope::checked(root.clone(), None, root.clone()).unwrap();
    let cache = root.join("builtin-locales");
    let consent = root.join("approval.json");
    let lifecycle = dsh_ocean::runtime_lifecycle::Lifecycle::<
        dsh_ocean::service::Server,
        tokio::process::Child,
    >::new(|_| {});
    let permit = lifecycle
        .try_begin(dsh_ocean::runtime_lifecycle::Operation::Update)
        .unwrap();
    assert!(dsh_ocean::builtin_locale_planner::recorded_plan(
        &runtime, &permit, &scope, &cache, &consent
    )
    .await
    .unwrap()
    .is_none());
    assert!(!cache.exists());
    let approved = dsh_ocean::builtin_locale_consent::Scope::new(
        &scope.dsh_home.join("profiles/web"),
        "0.2.0-rc.2",
    )
    .unwrap();
    dsh_ocean::builtin_locale_consent::record(&consent, &approved, false).unwrap();
    assert!(dsh_ocean::builtin_locale_planner::recorded_plan(
        &runtime, &permit, &scope, &cache, &consent
    )
    .await
    .unwrap()
    .is_none());
    assert!(!cache.exists());
    dsh_ocean::builtin_locale_consent::record(&consent, &approved, true).unwrap();
    fs::write(root.join(".runtime-pending.json"), "pending").unwrap();
    assert!(dsh_ocean::builtin_locale_planner::recorded_plan(
        &runtime, &permit, &scope, &cache, &consent
    )
    .await
    .is_err());
    assert!(!cache.exists());
    assert!(!scope.dsh_home.exists());
    let checked = root.canonicalize().unwrap();
    assert!(
        checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-admission-")
    );
    fs::remove_dir_all(checked).unwrap();
}
#[test]
fn launch_scope_does_not_silently_replace_relative_or_empty_dsh_home() {
    let scope = LaunchScope::checked(
        PathBuf::from("/private/home"),
        None,
        PathBuf::from("/private/cwd"),
    )
    .unwrap();
    assert_eq!(scope.dsh_home, PathBuf::from("/private/home/.dsh"));
    for value in ["", "relative", "/private/../foreign"] {
        assert!(LaunchScope::checked(
            PathBuf::from("/private/home"),
            Some(value.into()),
            PathBuf::from("/private/cwd")
        )
        .is_err());
    }
    assert!(LaunchScope::checked(
        PathBuf::from("relative-home"),
        None,
        PathBuf::from("/private/cwd")
    )
    .is_err());
}
