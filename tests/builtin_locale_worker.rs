use dsh_ocean::builtin_locale_worker::{inspect, Presence, Request};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[tokio::test]
async fn metadata_worker_is_explicit_bounded_and_credential_free() {
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-worker-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let node = root.join("node");
    fs::write(&node,"#!/bin/sh\n[ \"$1\" = --experimental-import-meta-resolve ] || exit 7\n[ -z \"${NODE_OPTIONS+x}\" ] || exit 8\n[ -z \"${OPENAI_API_KEY+x}\" ] || exit 9\nprintf '{\"state\":\"absent\"}\\n'\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    let request = Request {
        install_anchor: root.join("sdk/package.json"),
        profile_dir: root.join("dsh/profiles/web"),
        home: root.join("dsh"),
        cwd: root.clone(),
        telemetry_disabled_env: Some("1".into()),
    };
    assert_eq!(
        inspect(&node, &root.join("metadata.mjs"), &request, &root)
            .await
            .unwrap(),
        Presence::Absent
    );
    fs::write(
        &node,
        "#!/bin/sh\nprintf '{\"state\":\"absent\",\"extra\":true}'\n",
    )
    .unwrap();
    assert!(inspect(&node, &root.join("metadata.mjs"), &request, &root)
        .await
        .is_err());
    fs::write(&node, "#!/bin/sh\nhead -c 9000 /dev/zero\n").unwrap();
    assert!(inspect(&node, &root.join("metadata.mjs"), &request, &root)
        .await
        .is_err());
    assert!(!root.join("dsh").exists());
    let checked: PathBuf = root.canonicalize().unwrap();
    assert!(
        checked.is_absolute()
            && checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-worker-")
    );
    fs::remove_dir_all(checked).unwrap();
}

#[tokio::test]
#[ignore = "requires explicit private DSH_OCEAN_TEST_RUNTIME_ROOT"]
async fn rust_metadata_worker_resolves_real_embedded_sdk_helpers_without_profile_creation() {
    let sdk =
        PathBuf::from(std::env::var_os("DSH_OCEAN_TEST_RUNTIME_ROOT").expect("explicit SDK root"));
    assert!(sdk.is_absolute() && sdk.canonicalize().unwrap() == sdk);
    let root = std::env::temp_dir().join(format!(
        "ocean-locale-worker-real-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let tree = dsh_ocean::builtin_locale_cache::materialize(&root.join("cache")).unwrap();
    let request = Request {
        install_anchor: sdk.join("harness/node_modules/@deepseek-ai/dsh/package.json"),
        profile_dir: root.join("dsh/profiles/web"),
        home: root.join("dsh"),
        cwd: root.clone(),
        telemetry_disabled_env: Some("1".into()),
    };
    assert_eq!(
        inspect(
            &sdk.join("runtime/node/bin/node"),
            &tree.join("builtin-locale-sdk-preflight.mjs"),
            &request,
            &root
        )
        .await
        .unwrap(),
        Presence::Absent
    );
    assert!(!root.join("dsh").exists());
    assert!(tree.starts_with(&root));
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(tree.join("lib"), fs::Permissions::from_mode(0o700)).unwrap();
    let checked = root.canonicalize().unwrap();
    assert!(
        checked.parent() == Some(std::env::temp_dir().as_path())
            && checked
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ocean-locale-worker-real-")
    );
    fs::remove_dir_all(checked).unwrap();
}
