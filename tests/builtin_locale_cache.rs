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
