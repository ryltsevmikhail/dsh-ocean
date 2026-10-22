#[allow(unused_macros)]
// Included by the engine's unit tests: the private seam cannot forge public Ready authority.
macro_rules! runtime_switch_tests {
    () => {
        #[test]
        fn ordinary_local_start_gate_refuses_corrupt_or_dangling_pending_entries() {
            let (p,_c)=fixture();ensure_no_pending(&p.prefix).unwrap();fs::write(p.pending(),b"corrupt pending journal").unwrap();assert!(ensure_no_pending(&p.prefix).is_err(),"corruption must not bypass stopped recovery gate");
            fs::remove_file(p.pending()).unwrap();symlink("missing-recovery-record",p.pending()).unwrap();assert!(ensure_no_pending(&p.prefix).is_err(),"dangling pending link must still block ordinary start");
        }
        #[test]
        fn cancelled_preparation_never_stops_or_commits_and_commit_cannot_be_cancelled() {
            let (p,mut c)=fixture();let control=Arc::new(crate::update_control::Control::default());assert!(control.cancel());
            assert!(execute_controlled(&p,&p.prefix.join("generations/gen.new"),control,&mut c,||Ok(())).is_err(),"cancelled preparation must not enter filesystem/runtime transaction");
            assert!(c.running);assert!(!p.pending().exists());assert_eq!(c.new_starts,0);
            let control=Arc::new(crate::update_control::Control::default());
            execute_controlled(&p,&p.prefix.join("generations/gen.new"),control.clone(),&mut c,||Ok(())).unwrap();
            assert_eq!(control.snapshot().phase,crate::update_control::Phase::Committing);assert!(!control.cancel(),"confirmed local transaction must not acknowledge cancellation after mutation");
        }
        #[test]
        fn recovery_refuses_a_changed_managed_home_even_before_snapshot_creation() {
            let (mut p,mut c)=fixture();
            let j=Journal{schema:1,home:p.home.clone(),backups:p.backups.clone(),old:PathBuf::from("generations/gen.old"),new:PathBuf::from("generations/gen.new"),snapshot:None,unit:None};
            save(&p,&j).unwrap();c.running=false;p.home=p.home.with_file_name("different-home");
            assert!(recover(&p,&mut c).is_err(),"pending recovery must bind the same managed data home, not restart against a changed scope");assert_eq!(c.old_starts,0);assert!(p.pending().exists());
        }
        #[test]
        fn success_selects_candidate_and_records_only_after_live_health() {
            let (p, mut c) = fixture();
            let mut floor = 0;
            execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || {
                floor += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(
                std::fs::read_link(p.prefix.join("current")).unwrap(),
                PathBuf::from("generations/gen.new")
            );
            assert_eq!(floor, 1);
            assert!(!p.pending().exists());
        }
        #[test]
        fn failed_health_restores_all_bytes_before_old_start() {
            let (p, mut c) = fixture();
            c.bad = true;
            let mut floor = 0;
            assert!(
                execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || {
                    floor += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(floor, 0);
            assert_eq!(std::fs::read(p.home.join("config")).unwrap(), b"original");
            assert!(!p.home.join("created").exists());
            assert_eq!(c.old_starts, 1);
            assert!(!p.pending().exists());
        }
        #[test]
        fn snapshot_failure_never_activates_candidate() {
            let (p, mut c) = fixture();
            std::os::unix::fs::symlink("/outside", p.home.join("unsafe")).unwrap();
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert_eq!(c.new_starts, 0);
            assert_eq!(c.old_starts, 1);
        }
        #[test]
        fn corrupt_snapshot_leaves_stopped_pending_without_old_start() {
            let (p, mut c) = fixture();
            c.corrupt = true;
            c.bad = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert!(p.pending().exists());
            assert!(!c.running);
            assert_eq!(c.old_starts, 0);
        }
        #[test]
        fn interference_is_not_overwritten() {
            let (p, mut c) = fixture();
            c.interfere = true;
            c.bad = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert_eq!(
                std::fs::read_link(p.prefix.join("current")).unwrap(),
                PathBuf::from("generations/gen.other")
            );
            assert!(p.pending().exists());
            assert!(!c.running);
            assert_eq!(c.old_starts, 0);
        }
        #[test]
        fn recovery_restores_interrupted_candidate_and_blocks_apply() {
            let (p, mut c) = fixture();
            let j = Journal {
                schema:1,home:p.home.clone(),backups:p.backups.clone(),
                old: PathBuf::from("generations/gen.old"),
                new: PathBuf::from("generations/gen.new"),
                snapshot: Some(
                    crate::runtime_snapshot::capture(&p.home, &p.backups, || Ok(()))
                        .unwrap()
                        .path()
                        .into(),
                ),
                unit: None,
            };
            save(&p, &j).unwrap();
            select(&p, &j.old, &j.new).unwrap();
            std::fs::write(p.home.join("config"), b"changed").unwrap();
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            recover(&p, &mut c).unwrap();
            assert_eq!(std::fs::read(p.home.join("config")).unwrap(), b"original");
            assert!(!p.pending().exists());
        }
        #[test]
        fn floor_failure_rolls_back_without_accepting_candidate() {
            let (p, mut c) = fixture();
            assert!(execute(
                &p,
                &p.prefix.join("generations/gen.new"),
                &mut c,
                || anyhow::bail!("floor write failed")
            )
            .is_err());
            assert_eq!(c.old_starts, 1);
            assert!(!p.pending().exists());
        }
        #[test]
        fn originally_absent_home_is_absent_before_old_restart() {
            let (p, mut c) = fixture();
            fs::remove_dir_all(&p.home).unwrap();
            c.absent = true;
            c.bad = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert!(!p.home.exists());
            assert_eq!(c.old_starts, 1);
        }
        #[test]
        fn concurrent_unit_image_is_not_replaced() {
            let (p, mut c) = fixture();
            c.edit_unit = true;
            c.bad = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert_eq!(fs::read(p.prefix.join("unit")).unwrap(), b"concurrent");
            assert_eq!(c.old_starts, 0);
            assert!(!c.running);
            assert!(p.pending().exists());
        }
        #[test]
        fn candidate_stop_failure_retains_pending_and_never_restarts_old() {
            let (p, mut c) = fixture();
            c.stop_failure = true;
            c.bad = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert!(p.pending().exists());
            assert_eq!(c.old_starts, 0);
        }
        #[test]
        fn recovery_without_snapshot_refuses_unexpected_candidate_selection() {
            let (p, mut c) = fixture();
            let j = Journal {
                schema:1,home:p.home.clone(),backups:p.backups.clone(),
                old: PathBuf::from("generations/gen.old"),
                new: PathBuf::from("generations/gen.new"),
                snapshot: None,
                unit: None,
            };
            save(&p, &j).unwrap();
            select(&p, &j.old, &j.new).unwrap();
            assert!(recover(&p, &mut c).is_err());
            assert!(p.pending().exists());
            assert_eq!(c.old_starts, 0);
            assert!(!c.running);
        }
        #[test]
        fn failed_old_restart_is_stopped_and_keeps_recovery_journal() {
            let (p, mut c) = fixture();
            c.bad = true;
            c.old_start_failure = true;
            assert!(execute(&p, &p.prefix.join("generations/gen.new"), &mut c, || Ok(())).is_err());
            assert!(!c.running);
            assert!(p.pending().exists());
        }
    };
}
