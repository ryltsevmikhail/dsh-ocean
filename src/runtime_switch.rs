//! Durable owned-runtime switch. Callers retain the runtime permit and settings/floor guards.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Paths {
    pub prefix: PathBuf,
    pub home: PathBuf,
    pub backups: PathBuf,
}
/// Ordinary local starts/restarts refuse any pending entry, including corrupt or dangling.
pub fn ensure_no_pending(prefix: &Path) -> Result<()> {
    match fs::symlink_metadata(prefix.join(".runtime-pending.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect runtime recovery gate"),
        Ok(_) => anyhow::bail!(
            "pending runtime transaction requires explicit recovery; refusing local start/restart"
        ),
    }
}
impl Paths {
    pub fn pending(&self) -> PathBuf {
        self.prefix.join(".runtime-pending.json")
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct UnitImage {
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}
/// Implement only for a proven owned lease/unit, never Attached or a loopback hint.
/// stop must reap owned children; stopped must reject unknown writers. Unit methods
/// must compare exact images and refuse concurrent edits, including manager drop-ins.
pub trait Controller {
    fn ownership(&mut self, old: &Path) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn stopped(&mut self) -> Result<()>;
    fn start(&mut self, root: &Path) -> Result<()>;
    fn health(&mut self) -> Result<()>;
    fn unit(&mut self) -> Result<Option<UnitImage>> {
        Ok(None)
    }
    fn check_unit(&mut self, image: &Option<UnitImage>) -> Result<()> {
        anyhow::ensure!(
            image.is_none(),
            "unit controller must implement checked images"
        );
        Ok(())
    }
    fn switch_unit(&mut self, image: &Option<UnitImage>, _forward: bool) -> Result<()> {
        anyhow::ensure!(
            image.is_none(),
            "unit controller must implement checked replacement"
        );
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    home: PathBuf,
    backups: PathBuf,
    old: PathBuf,
    new: PathBuf,
    snapshot: Option<PathBuf>,
    unit: Option<UnitImage>,
}
/// Apply authenticated prepared bytes, retaining Prepared's prefix lock throughout.
/// Before import, acquire settings -> Runtime release-state transaction, then the
/// runtime permit. `record` must atomically record this exact Verified release via
/// that retained guard; on error it must not lower or partially replace a floor.
/// This engine is not a signature verifier or an ownership-discovery mechanism.
/// The controller is a trusted local adapter and health means authenticated live
/// health, not merely a listening port. Unknown writers must be excluded by consent.
pub fn apply(
    p: &Paths,
    prepared: &crate::runtime_bundle::Prepared,
    release: &crate::runtime_feed::Verified,
    control: Arc<crate::update_control::Control>,
    c: &mut impl Controller,
    record: impl FnMut() -> Result<()>,
) -> Result<()> {
    validate(p)?;
    ensure_no_pending(&p.prefix)?;
    c.ownership(&p.prefix.join(selected(p)?))?;
    prepared.verify(release)?;
    crate::runtime_health::check(prepared, release, control.clone())?;
    execute_controlled(p, prepared.root(), control, c, record)
}
fn execute_controlled(
    p: &Paths,
    root: &Path,
    control: Arc<crate::update_control::Control>,
    c: &mut impl Controller,
    record: impl FnMut() -> Result<()>,
) -> Result<()> {
    control.begin_commit()?;
    execute(p, root, c, record)
}
fn validate(p: &Paths) -> Result<()> {
    for path in [&p.prefix, &p.home, &p.backups] {
        anyhow::ensure!(path.is_absolute(), "absolute transaction paths required");
        crate::util::check_owned_ancestors(path.parent().context("missing parent")?)?;
    }
    crate::util::check_owned_ancestors(&p.prefix)?;
    anyhow::ensure!(
        !p.home.starts_with(&p.prefix) && !p.prefix.starts_with(&p.home),
        "home and prefix overlap"
    );
    Ok(())
}
fn target(path: &Path) -> Result<()> {
    let parts: Vec<_> = path.components().collect();
    anyhow::ensure!(
        parts.len() == 2
            && parts[0].as_os_str() == "generations"
            && parts[1]
                .as_os_str()
                .to_str()
                .is_some_and(|s| s.starts_with("gen.")
                    && s[4..].bytes().all(|b| b.is_ascii_alphanumeric())
                    && s.len() > 4),
        "unknown selector target"
    );
    Ok(())
}
fn selected(p: &Paths) -> Result<PathBuf> {
    let t = fs::read_link(p.prefix.join("current"))?;
    target(&t)?;
    crate::util::check_owned_ancestors(&p.prefix.join(&t))?;
    Ok(t)
}
fn sync(p: &Path) -> Result<()> {
    fs::File::open(p)?.sync_all()?;
    Ok(())
}
fn clear(p: &Paths) -> Result<()> {
    fs::remove_file(p.pending())?;
    sync(&p.prefix)
}
fn execute(
    p: &Paths,
    root: &Path,
    c: &mut impl Controller,
    mut record: impl FnMut() -> Result<()>,
) -> Result<()> {
    validate(p)?;
    ensure_no_pending(&p.prefix)?;
    let old = selected(p)?;
    let new = root.strip_prefix(&p.prefix)?.to_path_buf();
    target(&new)?;
    anyhow::ensure!(old != new, "candidate is already selected");
    crate::util::check_owned_ancestors(root)?;
    c.ownership(&p.prefix.join(&old))?;
    let mut j = Journal {
        schema: 1,
        home: p.home.clone(),
        backups: p.backups.clone(),
        old,
        new,
        snapshot: None,
        unit: c.unit()?,
    };
    c.check_unit(&j.unit)?;
    // create_new is the last guard against a concurrent/pending operation.
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p.pending())?;
    serde_json::to_writer(&file, &j)?;
    file.sync_all()?;
    sync(&p.prefix)?;
    c.stop()?;
    c.stopped()?;
    let snapshot = match crate::runtime_snapshot::capture(&p.home, &p.backups, || c.stopped()) {
        Ok(s) => s,
        Err(e) => {
            rollback(p, &j, c)?;
            return Err(e).context("snapshot failed; original runtime restored");
        }
    };
    j.snapshot = Some(snapshot.path().into());
    save(p, &j)?;
    let activation = (|| {
        c.stopped()?;
        c.check_unit(&j.unit)?;
        select(p, &j.old, &j.new)?;
        c.switch_unit(&j.unit, true)?;
        c.start(root)?;
        c.health()?;
        anyhow::ensure!(selected(p)? == j.new, "selector changed during activation");
        c.check_unit(&j.unit)?;
        record()
    })();
    if let Err(e) = activation {
        rollback(p, &j, c).context("rollback incomplete; pending recovery retained")?;
        return Err(e).context("candidate rejected; original restored");
    }
    clear(p)
}
fn rollback(p: &Paths, j: &Journal, c: &mut impl Controller) -> Result<()> {
    c.stop()?;
    c.stopped()?;
    let current = selected(p)?;
    anyhow::ensure!(
        current == j.old || current == j.new,
        "unknown concurrent selector; refusing overwrite"
    );
    c.check_unit(&j.unit)?;
    anyhow::ensure!(
        j.snapshot.is_some() || current == j.old,
        "candidate selected without durable snapshot; refusing recovery"
    );
    if let Some(path) = &j.snapshot {
        anyhow::ensure!(
            path.starts_with(&p.backups),
            "snapshot outside transaction backups"
        );
        crate::runtime_snapshot::open(&p.home, path)?.restore(|| c.stopped())?;
    }
    c.stopped()?;
    c.check_unit(&j.unit)?;
    select(p, &current, &j.old)?;
    c.switch_unit(&j.unit, false)?;
    if let Err(error) = c.start(&p.prefix.join(&j.old)).and_then(|_| c.health()) {
        c.stop()?;
        c.stopped()?;
        return Err(error);
    }
    clear(p)
}
/// Explicit rollback only. Caller holds the prefix lock and runtime permit;
/// ordinary start paths must reject `Paths::pending()` until this succeeds.
pub fn recover(p: &Paths, c: &mut impl Controller) -> Result<()> {
    validate(p)?;
    let metadata = fs::symlink_metadata(p.pending())?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1
            && metadata.len() < 1024 * 1024,
        "unsafe pending journal"
    );
    let j: Journal =
        serde_json::from_slice(&crate::release_feed::read_input(&p.pending(), 1024 * 1024)?)?;
    anyhow::ensure!(j.schema==1&&j.home==p.home&&j.backups==p.backups,"pending runtime journal scope changed; restore original home/backup configuration before recovery");
    target(&j.old)?;
    target(&j.new)?;
    c.ownership(&p.prefix.join(&j.old))?;
    rollback(p, &j, c)
}
fn save(p: &Paths, j: &Journal) -> Result<()> {
    let temp = p.prefix.join(format!(
        ".runtime-journal-{}",
        crate::downloads::unique_id()
    ));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    serde_json::to_writer(&file, j)?;
    file.sync_all()?;
    fs::rename(&temp, p.pending())?;
    sync(&p.prefix)
}
fn select(p: &Paths, old: &Path, new: &Path) -> Result<()> {
    target(new)?;
    anyhow::ensure!(selected(p)? == old, "selector changed; refusing overwrite");
    let temp = p.prefix.join(format!(
        ".runtime-current-{}",
        crate::downloads::unique_id()
    ));
    symlink(new, &temp)?;
    anyhow::ensure!(selected(p)? == old, "selector changed; refusing overwrite");
    fs::rename(temp, p.prefix.join("current"))?;
    sync(&p.prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        p: Paths,
        running: bool,
        bad: bool,
        corrupt: bool,
        interfere: bool,
        new_starts: usize,
        old_starts: usize,
        absent: bool,
        edit_unit: bool,
        stop_failure: bool,
        old_start_failure: bool,
    }
    impl Controller for Fake {
        fn ownership(&mut self, _: &Path) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            anyhow::ensure!(
                !(self.stop_failure && self.new_starts > 0),
                "owned stop failed"
            );
            self.running = false;
            Ok(())
        }
        fn unit(&mut self) -> Result<Option<UnitImage>> {
            if self.edit_unit {
                fs::write(self.p.prefix.join("unit"), b"before")?;
                Ok(Some(UnitImage {
                    before: b"before".to_vec(),
                    after: b"after".to_vec(),
                }))
            } else {
                Ok(None)
            }
        }
        fn check_unit(&mut self, image: &Option<UnitImage>) -> Result<()> {
            if let Some(i) = image {
                let bytes = fs::read(self.p.prefix.join("unit"))?;
                anyhow::ensure!(bytes == i.before || bytes == i.after, "unknown unit");
            }
            Ok(())
        }
        fn switch_unit(&mut self, image: &Option<UnitImage>, forward: bool) -> Result<()> {
            self.check_unit(image)?;
            if let Some(i) = image {
                fs::write(
                    self.p.prefix.join("unit"),
                    if forward { &i.after } else { &i.before },
                )?;
            }
            Ok(())
        }
        fn stopped(&mut self) -> Result<()> {
            anyhow::ensure!(!self.running, "running");
            Ok(())
        }
        fn start(&mut self, root: &Path) -> Result<()> {
            if root.ends_with("gen.old") {
                if self.absent {
                    assert!(!self.p.home.exists());
                } else {
                    assert_eq!(fs::read(self.p.home.join("config"))?, b"original");
                    assert_eq!(
                        fs::read(self.p.home.join("sessions/session"))?,
                        b"session-original"
                    );
                    assert_eq!(
                        fs::read(self.p.home.join("plugins/node_modules/module"))?,
                        b"plugin-original"
                    );
                }
                assert!(!self.p.home.join("created").exists());
                self.old_starts += 1;
            } else {
                self.new_starts += 1;
                if self.absent {
                    fs::create_dir(&self.p.home)?;
                } else {
                    fs::write(self.p.home.join("sessions/session"), b"changed")?;
                    fs::write(self.p.home.join("plugins/node_modules/module"), b"changed")?;
                }
                if self.edit_unit {
                    fs::write(self.p.prefix.join("unit"), b"concurrent")?;
                }
                fs::write(self.p.home.join("config"), b"changed")?;
                fs::write(self.p.home.join("created"), b"candidate")?;
                if self.corrupt {
                    let j: Journal = serde_json::from_slice(&fs::read(self.p.pending())?)?;
                    fs::write(j.snapshot.unwrap().join("data/config"), b"corrupt")?;
                }
                if self.interfere {
                    fs::remove_file(self.p.prefix.join("current"))?;
                    symlink("generations/gen.other", self.p.prefix.join("current"))?;
                }
            }
            self.running = true;
            anyhow::ensure!(
                !(root.ends_with("gen.old") && self.old_start_failure),
                "old start failed after spawn"
            );
            Ok(())
        }
        fn health(&mut self) -> Result<()> {
            anyhow::ensure!(
                !(self.bad && self.new_starts > 0 && self.old_starts == 0),
                "candidate unhealthy"
            );
            Ok(())
        }
    }
    fn fixture() -> (Paths, Fake) {
        let root =
            std::env::temp_dir().join(format!("ocean-switch-{}", crate::downloads::unique_id()));
        fs::create_dir(&root).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let p = Paths {
            prefix: root.join("prefix"),
            home: root.join("home"),
            backups: root.join("backups"),
        };
        fs::create_dir_all(p.prefix.join("generations/gen.old")).unwrap();
        fs::create_dir_all(p.prefix.join("generations/gen.new")).unwrap();
        fs::create_dir(&p.home).unwrap();
        fs::create_dir(&p.backups).unwrap();
        fs::write(p.home.join("config"), b"original").unwrap();
        symlink("generations/gen.old", p.prefix.join("current")).unwrap();
        fs::create_dir_all(p.home.join("sessions")).unwrap();
        fs::create_dir_all(p.home.join("plugins/node_modules")).unwrap();
        fs::write(p.home.join("sessions/session"), b"session-original").unwrap();
        fs::write(
            p.home.join("plugins/node_modules/module"),
            b"plugin-original",
        )
        .unwrap();
        let c = Fake {
            p: Paths {
                prefix: p.prefix.clone(),
                home: p.home.clone(),
                backups: p.backups.clone(),
            },
            running: true,
            bad: false,
            corrupt: false,
            interfere: false,
            new_starts: 0,
            old_starts: 0,
            absent: false,
            edit_unit: false,
            stop_failure: false,
            old_start_failure: false,
        };
        (p, c)
    }
    include!("../tests/runtime_switch.rs");
    runtime_switch_tests!();
}
