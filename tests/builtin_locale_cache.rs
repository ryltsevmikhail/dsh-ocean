use dsh_ocean::{
    builtin_locale_cache::materialize,
    builtin_locales::{bundle_id, resources},
};
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dsh-ocean-builtin-cache-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0.is_absolute()
                && self.0.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-builtin-cache-test-")
        );
        fn writable(p: &Path) {
            if fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
                for x in fs::read_dir(p).unwrap() {
                    writable(&x.unwrap().path());
                }
            }
        }
        writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn older_tree(cache: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let current = materialize(cache).unwrap();
    let license = current.join("LICENSE");
    fs::set_permissions(&license, fs::Permissions::from_mode(0o600)).unwrap();
    let mut text = fs::read(&license).unwrap();
    text.push(b'\n');
    fs::write(&license, text).unwrap();
    fs::set_permissions(&license, fs::Permissions::from_mode(0o400)).unwrap();
    let mut digest = Sha256::new();
    for resource in resources() {
        let bytes = fs::read(current.join(resource.path)).unwrap();
        digest.update((resource.path.len() as u64).to_be_bytes());
        digest.update(resource.path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    let id = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let old = cache.join(id);
    assert_eq!(
        current.canonicalize().unwrap().parent(),
        Some(cache.canonicalize().unwrap().as_path())
    );
    assert_eq!(old.parent(), Some(cache));
    assert!(!old.exists());
    fs::rename(current, &old).unwrap();
    old
}
#[test]
fn recognizes_exact_retained_private_content_without_substituting_current_resources() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let old = older_tree(&cache);
    let overlay = old.join("overlay.yml");
    let before = fs::metadata(old.join("LICENSE"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |candidate| candidate
            == overlay)
        .unwrap(),
        Some(overlay.clone())
    );
    let current = materialize(&cache).unwrap();
    assert_ne!(old, current);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
    assert_eq!(
        fs::metadata(old.join("LICENSE"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert_eq!(
        dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |candidate| candidate
            == overlay)
        .unwrap(),
        Some(overlay)
    );
}
#[test]
fn retained_match_refuses_changed_content_without_repair() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let old = older_tree(&cache);
    fs::set_permissions(old.join("LICENSE"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(old.join("LICENSE"), b"tampered").unwrap();
    fs::set_permissions(old.join("LICENSE"), fs::Permissions::from_mode(0o400)).unwrap();
    assert!(dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |_| true).is_err());
    assert_eq!(fs::read(old.join("LICENSE")).unwrap(), b"tampered");
}
#[test]
fn retained_match_refuses_writable_hardlinked_and_symlinked_resources() {
    for kind in ["writable", "hardlink", "symlink"] {
        let f = Fixture::new();
        let cache = f.0.join("cache");
        let old = older_tree(&cache);
        match kind {
            "writable" => {
                fs::set_permissions(old.join("lib"), fs::Permissions::from_mode(0o700)).unwrap()
            }
            "hardlink" => fs::hard_link(old.join("LICENSE"), f.0.join("linked-license")).unwrap(),
            "symlink" => {
                fs::set_permissions(&old, fs::Permissions::from_mode(0o700)).unwrap();
                let source = old.join("LICENSE");
                let moved = f.0.join("license-target");
                assert!(source.is_absolute() && source.parent() == Some(old.as_path()));
                assert_eq!(moved.parent(), Some(f.0.as_path()));
                assert!(!moved.exists());
                fs::rename(&source, &moved).unwrap();
                symlink(&moved, &source).unwrap();
                fs::set_permissions(&old, fs::Permissions::from_mode(0o500)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |_| true).is_err(),
            "{kind}"
        );
    }
}
#[test]
fn retained_lookup_is_read_only_and_ignores_unmatched_candidate_bytes() {
    let f = Fixture::new();
    let missing = f.0.join("absent");
    assert!(
        dsh_ocean::builtin_locale_cache::find_retained_overlay(&missing, |_| true)
            .unwrap()
            .is_none()
    );
    assert!(!missing.exists());
    let cache = f.0.join("cache");
    let old = older_tree(&cache);
    fs::set_permissions(old.join("LICENSE"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |_| false)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::metadata(old.join("LICENSE"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(dsh_ocean::builtin_locale_cache::find_retained_overlay(&cache, |_| true).is_err());
}
#[test]
fn publishes_exact_embedded_tree_and_reuses_it_without_rewriting() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let tree = materialize(&cache).unwrap();
    assert_eq!(tree, cache.join(bundle_id()));
    for resource in resources() {
        let path = tree.join(resource.path);
        assert_eq!(fs::read(&path).unwrap(), resource.bytes);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }
    assert_eq!(
        fs::metadata(&tree).unwrap().permissions().mode() & 0o777,
        0o500
    );
    let modified = fs::metadata(tree.join("package.json"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(materialize(&cache).unwrap(), tree);
    assert_eq!(
        fs::metadata(tree.join("package.json"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
}
#[test]
fn never_repairs_or_overwrites_an_existing_conflicting_content_addressed_tree() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let tree = materialize(&cache).unwrap();
    fs::set_permissions(tree.join("package.json"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(tree.join("package.json"), b"changed").unwrap();
    assert!(materialize(&cache).is_err());
    assert_eq!(fs::read(tree.join("package.json")).unwrap(), b"changed");
}
#[test]
fn refuses_linked_roots_and_existing_empty_destination_without_touching_other_files() {
    let f = Fixture::new();
    let actual = f.0.join("actual");
    fs::create_dir(&actual).unwrap();
    let link = f.0.join("redirect");
    symlink(&actual, &link).unwrap();
    assert!(materialize(&link).is_err());
    assert_eq!(fs::read_dir(&actual).unwrap().count(), 0);
    let cache = f.0.join("cache");
    let tree = cache.join(bundle_id());
    fs::create_dir_all(&tree).unwrap();
    fs::write(f.0.join("sentinel"), b"untouched").unwrap();
    assert!(materialize(&cache).is_err());
    assert_eq!(fs::read_dir(&tree).unwrap().count(), 0);
    assert_eq!(fs::read(f.0.join("sentinel")).unwrap(), b"untouched");
}

#[test]
fn concurrent_publishers_converge_without_replacing_the_winner_or_leaking_staging() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let handles = (0..4)
        .map(|_| {
            let path = cache.clone();
            std::thread::spawn(move || materialize(&path).unwrap())
        })
        .collect::<Vec<_>>();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), cache.join(bundle_id()));
    }
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
}

#[test]
fn extra_files_and_hardlinked_members_are_refused_and_kept_for_inspection() {
    let f = Fixture::new();
    let cache = f.0.join("cache");
    let tree = materialize(&cache).unwrap();
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(tree.join("extra"), b"do not remove").unwrap();
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(materialize(&cache).is_err());
    assert_eq!(fs::read(tree.join("extra")).unwrap(), b"do not remove");
    let other = f.0.join("other-cache");
    let clean = materialize(&other).unwrap();
    fs::hard_link(clean.join("lib/index.js"), f.0.join("host-alias.js")).unwrap();
    assert!(materialize(&other).is_err());
    assert!(f.0.join("host-alias.js").exists());
}
