use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::NockApp;
use crate::kernel::form::Kernel;
use crate::save::SaveableCheckpoint;

fn load_jam_bytes(jam: &str) -> Vec<u8> {
    // Try multiple possible locations for the jam file
    let possible_paths = [
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-jams")
            .join(jam),
        Path::new("open/crates/nockapp/test-jams").join(jam),
        // Add other potential paths
    ];

    possible_paths
        .iter()
        .find_map(|path| fs::read(path).ok())
        .unwrap_or_else(|| panic!("Failed to read {} file from any known location", jam))
}

pub async fn setup_nockapp(jam: &str) -> (TempDir, NockApp) {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let jam_bytes = load_jam_bytes(jam);

    let kernel_f = move |_| async move {
        let kernel = Kernel::load(&jam_bytes, None, vec![], Default::default(), None).await?;
        Ok::<Kernel<SaveableCheckpoint>, crate::CrownError>(kernel)
    };
    (
        temp_dir,
        NockApp::new(kernel_f)
            .await
            .expect("Could not create NockApp"),
    )
}

#[cfg(test)]
pub mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use bytes::Bytes;
    use nockvm::ext::noun_equality;
    use nockvm::jets::util::slot;
    use nockvm::mem::NockStack;
    use nockvm::noun::{CellHandle, Noun, NounAllocator, NounSpace, D, T};
    use nockvm::serialization::{cue, jam};
    use nockvm::unifying_equality::unifying_equality;
    use nockvm_macros::tas;
    use tempfile::TempDir;
    use tracing_test::traced_test;

    use super::{load_jam_bytes, setup_nockapp};
    use crate::kernel::form::{Kernel, PmaConfig};
    use crate::nockapp::wire::{SystemWire, Wire};
    use crate::noun::slab::{slab_equality, NockJammer, NounSlab};
    use crate::save::CheckpointBootstrapReader;
    use crate::test_support::TestArena;
    use crate::utils::{
        NOCK_STACK_SIZE, NOCK_STACK_SIZE_HUGE, NOCK_STACK_SIZE_LARGE, NOCK_STACK_SIZE_MEDIUM,
        NOCK_STACK_SIZE_SMALL, NOCK_STACK_SIZE_TINY,
    };
    use crate::NounExt;

    fn summarize_samples(label: &str, samples: &[Duration]) -> (f64, f64, f64) {
        if samples.is_empty() {
            println!("perf: {}: no samples", label);
            return (0.0, 0.0, 0.0);
        }
        let mut min = samples[0];
        let mut max = samples[0];
        let mut total_us: u128 = 0;
        for sample in samples {
            if *sample < min {
                min = *sample;
            }
            if *sample > max {
                max = *sample;
            }
            total_us += sample.as_micros();
        }
        let count = samples.len() as f64;
        let avg_ms = (total_us as f64) / count / 1000.0;
        let min_ms = (min.as_micros() as f64) / 1000.0;
        let max_ms = (max.as_micros() as f64) / 1000.0;
        println!(
            "perf: {}: n={}, avg_ms={:.3}, min_ms={:.3}, max_ms={:.3}",
            label,
            samples.len(),
            avg_ms,
            min_ms,
            max_ms
        );
        (avg_ms, min_ms, max_ms)
    }

    #[tokio::test(flavor = "current_thread")]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    #[ignore]
    async fn test_poke_peek_perf_workload() {
        std::env::set_var("NOCK_PMA_TIMING", "1");
        std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");

        let _test_arena = TestArena::default();
        let kernel_bytes = if let Some(path) = std::env::var_os("NOCKAPP_PERF_KERNEL_JAM") {
            let path = PathBuf::from(path);
            fs::read(&path)
                .unwrap_or_else(|err| panic!("Failed to read kernel jam from {:?}: {err}", path))
        } else {
            load_jam_bytes("test-ker.jam")
        };

        let checkpoint = if let Some(path) = std::env::var_os("NOCKAPP_PERF_CHECKPOINT_DIR") {
            let path = PathBuf::from(path);
            CheckpointBootstrapReader::<NockJammer>::new(path.clone())
                .load_latest(None)
                .await
                .unwrap_or_else(|err| panic!("Failed to load checkpoint from {:?}: {err}", path))
        } else {
            None
        };

        let stack_choice =
            std::env::var("NOCKAPP_PERF_STACK").unwrap_or_else(|_| "normal".to_string());
        let pma_words_override = std::env::var("NOCKAPP_PERF_PMA_WORDS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok());
        let mut checkpoint = checkpoint;
        let (stack_words, kernel, _pma_dir) = match stack_choice.as_str() {
            "tiny" => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE_TINY);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state_tiny(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE_TINY, kernel, pma_dir)
            }
            "small" => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE_SMALL);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state_small(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE_SMALL, kernel, pma_dir)
            }
            "medium" => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE_MEDIUM);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state_medium(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE_MEDIUM, kernel, pma_dir)
            }
            "large" => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE_LARGE);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state_large(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE_LARGE, kernel, pma_dir)
            }
            "huge" => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE_HUGE);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state_huge(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE_HUGE, kernel, pma_dir)
            }
            _ => {
                let pma_dir = TempDir::new().expect("Failed to create temp PMA dir");
                let pma_path_0 = pma_dir.path().join("0.pma");
                let pma_path_1 = pma_dir.path().join("1.pma");
                let pma_words = pma_words_override.unwrap_or(NOCK_STACK_SIZE);
                let pma_config = Some(PmaConfig {
                    path_0: pma_path_0,
                    path_1: pma_path_1,
                    words: pma_words,
                    open_existing: false,
                    create_snapshots: false,
                    rotating_snapshot_interval_event_time: None,
                    restore_manifest: None,
                    gc_interval: None,
                });
                let kernel = Kernel::load_with_hot_state(
                    &kernel_bytes,
                    checkpoint.take(),
                    &[],
                    vec![],
                    Default::default(),
                    pma_config,
                )
                .await
                .expect("Failed to load kernel");
                (NOCK_STACK_SIZE, kernel, pma_dir)
            }
        };

        let pma_timing = kernel
            .serf
            .pma_timing
            .clone()
            .expect("NOCK_PMA_TIMING must be set before setup");
        let _ = pma_timing.take_samples();

        let iters: usize = std::env::var("NOCKAPP_PERF_ITERS")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(20);
        let warmup: usize = std::env::var("NOCKAPP_PERF_WARMUP")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(1);

        let poke_jam_path = std::env::var_os("NOCKAPP_PERF_POKE_JAM");
        let peek_jam_path = std::env::var_os("NOCKAPP_PERF_PEEK_JAM");
        let use_custom_jam = poke_jam_path.is_some() || peek_jam_path.is_some();
        if poke_jam_path.is_some() != peek_jam_path.is_some() {
            panic!("Both NOCKAPP_PERF_POKE_JAM and NOCKAPP_PERF_PEEK_JAM must be set together");
        }

        let (poke_jam, peek_jam) =
            if let (Some(poke_path), Some(peek_path)) = (poke_jam_path, peek_jam_path) {
                let poke_path = PathBuf::from(poke_path);
                let peek_path = PathBuf::from(peek_path);
                let poke_jam = fs::read(&poke_path).unwrap_or_else(|err| {
                    panic!("Failed to read poke jam from {:?}: {err}", poke_path)
                });
                let peek_jam = fs::read(&peek_path).unwrap_or_else(|err| {
                    panic!("Failed to read peek jam from {:?}: {err}", peek_path)
                });
                (poke_jam, peek_jam)
            } else {
                (Vec::new(), Vec::new())
            };

        let mut stack = if use_custom_jam {
            None
        } else {
            Some(NockStack::new(NOCK_STACK_SIZE, 0))
        };
        let mut poke_wall = Vec::with_capacity(iters);
        let mut peek_wall = Vec::with_capacity(iters);

        println!(
            "perf: stack_words={}, iters={}, warmup_skipped={}, custom_jam={}",
            stack_words, iters, warmup, use_custom_jam
        );

        let make_slab_from_jam = |jam: &[u8]| -> NounSlab {
            let mut slab = NounSlab::new();
            let noun = slab
                .cue_into(Bytes::copy_from_slice(jam))
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
            slab.set_root(noun);
            slab
        };

        for i in 1..=iters {
            let poke = if use_custom_jam {
                make_slab_from_jam(&poke_jam)
            } else {
                let poke_noun = D(tas!(b"inc"));
                let mut slab = NounSlab::new();
                let space = NounSpace::empty();
                slab.copy_into(poke_noun, &space);
                slab
            };
            let wire = SystemWire.to_wire();
            let poke_start = Instant::now();
            let _ = kernel.poke(wire, poke).await.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            poke_wall.push(poke_start.elapsed());

            let peek = if use_custom_jam {
                make_slab_from_jam(&peek_jam)
            } else {
                let stack = stack.as_mut().expect("stack");
                let space = stack.noun_space();
                let peek_noun = T(stack, &[D(tas!(b"state")), D(0)]);
                let mut slab = NounSlab::new();
                slab.copy_into(peek_noun, &space);
                slab
            };
            let peek_start = Instant::now();
            let mut res = kernel.peek(peek).await.unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            peek_wall.push(peek_start.elapsed());

            if use_custom_jam {
                if i == 1 {
                    let jammed = res.jam();
                    let mut roundtrip = NounSlab::<NockJammer>::new();
                    let noun = roundtrip.cue_into(jammed).unwrap_or_else(|err| {
                        panic!(
                            "Panicked with {err:?} at {}:{} (git sha: {:?})",
                            file!(),
                            line!(),
                            option_env!("GIT_SHA")
                        )
                    });
                    roundtrip.set_root(noun);
                    assert!(
                        slab_equality(&res, &roundtrip),
                        "peek roundtrip mismatch: res={:?} roundtrip={:?}",
                        res,
                        roundtrip
                    );
                }
            } else {
                let res_space = res.noun_space();
                res.modify_noun(|r| {
                    let cell = slot(r, 7, &res_space)
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .as_cell()
                        .unwrap_or_else(|err| {
                            panic!(
                                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        });
                    CellHandle::new(cell, &res_space).tail().noun()
                });

                let comp = {
                    let mut slab = NounSlab::<NockJammer>::new();
                    let space = NounSpace::empty();
                    slab.copy_into(D(i as u64), &space);
                    slab
                };

                assert!(
                    slab_equality(&res, &comp),
                    "res: {:?} != comp: {:?}",
                    res,
                    comp
                );
            }
        }

        let mut pma_samples = pma_timing.take_samples();
        assert_eq!(
            pma_samples.len(),
            iters,
            "expected one PMA timing sample per poke"
        );

        let skip = warmup.min(iters);
        let poke_wall = if poke_wall.len() > skip {
            &poke_wall[skip..]
        } else {
            &[]
        };
        let peek_wall = if peek_wall.len() > skip {
            &peek_wall[skip..]
        } else {
            &[]
        };
        if pma_samples.len() > skip {
            pma_samples = pma_samples.split_off(skip);
        } else {
            pma_samples.clear();
        }

        let event_samples: Vec<Duration> = pma_samples.iter().map(|s| s.event).collect();
        let pma_copy_samples: Vec<Duration> = pma_samples.iter().map(|s| s.pma_copy).collect();
        let total_samples: Vec<Duration> =
            pma_samples.iter().map(|s| s.event + s.pma_copy).collect();

        println!(
            "perf: pokes={}, peeks={}, warmup_skipped={}",
            iters, iters, skip
        );
        let (_event_avg, _, _) = summarize_samples("poke_event", &event_samples);
        let (pma_avg, _, _) = summarize_samples("poke_pma_copy", &pma_copy_samples);
        let (total_avg, _, _) = summarize_samples("poke_event_plus_pma", &total_samples);
        summarize_samples("poke_wall", poke_wall);
        summarize_samples("peek_wall", peek_wall);
        if total_avg > 0.0 {
            println!(
                "perf: poke_pma_share_avg_pct={:.1}",
                (pma_avg / total_avg) * 100.0
            );
        }
    }

    // Tests for fallback to previous checkpoint if checkpoint is corrupt
    // TODO: ask about this test and reframe it for 'Saver'
    /*
    #[tokio::test]
    #[traced_test]
    #[cfg_attr(miri, ignore)]
    async fn test_nockapp_corrupt_check() {
        let (temp, mut nockapp) = setup_nockapp("test-ker.jam").await;
        assert_eq!(nockapp.kernel.serf.event_number.load(Ordering::SeqCst), 0);

        // Save a valid checkpoint
        save_nockapp(&mut nockapp).await;

        // Generate an invalid checkpoint by incrementing the event number
        let mut invalid = nockapp
            .kernel
            .checkpoint()
            .await
            .expect("Could not get kernel checkpoint");
        invalid.event_num += 1;
        assert!(!invalid.validate());

        // The invalid checkpoint has a higher event number than the valid checkpoint
        let mut checkpoint_stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let valid = jam_paths
            .load_checkpoint(&mut checkpoint_stack)
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        assert!(valid.event_num < invalid.event_num);

        // Save the corrupted checkpoint, because of the toggle buffer, we will write to jam file 1
        assert!(!jam_paths.1.exists());
        let jam_path = &jam_paths.1;
        let jam_bytes = invalid.encode().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        tokio::fs::write(jam_path, jam_bytes)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });

        // The loaded checkpoint will be the valid one
        let chk = jam_paths
            .load_checkpoint(&mut checkpoint_stack)
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        assert!(chk.event_num == valid.event_num);
    }
    */

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_stack() {
        let _test_arena = TestArena::default();
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut jam_stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let arvo_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Could not get kernel state slab");
        let mut arvo = arvo_slab.copy_to_stack(&mut jam_stack);
        let j = jam(&mut jam_stack, arvo);
        let mut c = cue(&mut jam_stack, j).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        // new nockstack
        unsafe { assert!(unifying_equality(&mut jam_stack, &mut arvo, &mut c)) }
    }

    // This actually gets used to test with miri
    // but when it was successful it took too long.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_jam_equality_slab_no_driver() {
        let _test_arena = TestArena::default();
        let bytes = include_bytes!("../../test-jams/test-ker.jam");
        let mut slab1: NounSlab = NounSlab::new();
        slab1
            .cue_into(Bytes::from(Vec::from(bytes)))
            .unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        let jammed_bytes = slab1.jam();
        let mut slab2: NounSlab = NounSlab::new();
        let _c = slab2.cue_into(jammed_bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        assert!(slab_equality(&slab1, &slab2));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_slab() {
        let _test_arena = TestArena::default();
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut state_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Could not get kernel state slab");
        let bytes = state_slab.jam();
        let c = state_slab.cue_into(bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let space = state_slab.noun_space();
        let root = unsafe { state_slab.root() };
        assert!(noun_equality(root.in_space(&space), c.in_space(&space)));
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(miri, ignore)]
    async fn test_jam_equality_slab_stack() {
        let _test_arena = TestArena::default();
        let (_temp, nockapp) = setup_nockapp("test-ker.jam").await;
        let kernel = nockapp.kernel;
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let state_slab = kernel
            .serf
            .get_kernel_state_slab()
            .await
            .expect("Failed to get kernel state slab");
        // Use slab to jam
        let bytes = state_slab.jam();
        // Use the stack to cue
        let mut c = Noun::cue_bytes(&mut stack, &bytes).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        let mut state_stack = state_slab.copy_to_stack(&mut stack);
        unsafe {
            // check for equality
            assert!(unifying_equality(&mut stack, &mut state_stack, &mut c))
        }
    }
}
