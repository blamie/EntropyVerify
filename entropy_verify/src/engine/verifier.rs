// verifier engine

use crate::app::EngineEvent;
use crate::config::TestPlan;
use crate::crypto::datagen::BlockGenerator;
use crate::engine::block;
use crate::engine::io::PlatformEngine;
use crossbeam_channel::Sender;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub struct SharedVerifyMetrics {
    pub bytes_verified: AtomicU64,
    pub blocks_verified: AtomicU64,
    pub files_completed: AtomicU64,
    pub verify_errors: AtomicU64,
    pub corrupted_blocks: AtomicU64,
}

impl SharedVerifyMetrics {
    pub fn new() -> Self {
        Self {
            bytes_verified: AtomicU64::new(0),
            blocks_verified: AtomicU64::new(0),
            files_completed: AtomicU64::new(0),
            verify_errors: AtomicU64::new(0),
            corrupted_blocks: AtomicU64::new(0),
        }
    }
}

pub struct VerifyWorkerConfig {
    pub target_dir: PathBuf,
    pub block_size: usize,
    pub queue_depth: usize,
    pub plan: TestPlan,
    pub generator: BlockGenerator,
    pub thread_id: u16,
    pub metrics: Arc<SharedVerifyMetrics>,
    pub events: Sender<EngineEvent>,
    pub shutdown: Arc<AtomicBool>,
    pub pause: Arc<(Mutex<bool>, Condvar)>,
    pub work_rx: crossbeam_channel::Receiver<u32>,
}

pub fn verify_worker(cfg: VerifyWorkerConfig) {
    let mut engine = match PlatformEngine::new(cfg.queue_depth, cfg.block_size) {
        Ok(e) => e,
        Err(e) => {
            let _ = cfg.events.send(EngineEvent::FatalError(format!(
                "Verify worker {} failed to create I/O engine: {}",
                cfg.thread_id, e
            )));
            return;
        }
    };

    while let Ok(file_index) = cfg.work_rx.recv() {
        if cfg.shutdown.load(Ordering::Relaxed) {
            break;
        }

        let file_path = cfg.target_dir.join(TestPlan::file_name(file_index));
        let num_blocks = cfg.plan.blocks_for_file(file_index);

        if let Err(e) = engine.open_file(&file_path, false) {
            let _ = cfg.events.send(EngineEvent::ReadError {
                file_index,
                block_index: 0,
                error: format!("Failed to open file for verification: {}", e),
            });
            cfg.metrics.verify_errors.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        // queue reads
        let qd = engine.queue_depth();
        let mut submitted: u32 = 0;
        let mut verified: u32 = 0;

        while submitted < num_blocks && (submitted - verified) < qd as u32 {
            if cfg.shutdown.load(Ordering::Relaxed) {
                break;
            }

            let slot = match engine.acquire_slot() {
                Ok(s) => s,
                Err(_) => break,
            };

            let offset = submitted as u64 * cfg.block_size as u64;
            if let Err(e) = engine.submit_read(slot, offset, submitted) {
                let _ = cfg.events.send(EngineEvent::ReadError {
                    file_index,
                    block_index: submitted,
                    error: format!("Read submission failed: {}", e),
                });
                cfg.metrics.verify_errors.fetch_add(1, Ordering::Relaxed);
                break;
            }
            submitted += 1;
        }

        // wait and verify
        while verified < submitted {
            if cfg.shutdown.load(Ordering::Relaxed) {
                break;
            }

            check_pause(&cfg.pause);

            let completed_slot = match engine.wait_completion() {
                Ok(s) => s,
                Err(e) => {
                    let _ = cfg.events.send(EngineEvent::ReadError {
                        file_index,
                        block_index: verified,
                        error: format!("Read completion failed: {}", e),
                    });
                    cfg.metrics.verify_errors.fetch_add(1, Ordering::Relaxed);
                    verified += 1;
                    continue;
                }
            };

            let buf = engine.slot_buffer(completed_slot);
            match block::verify_block(buf, &cfg.generator) {
                Ok(_header) => {
                    
                }
                Err(e) => {
                    cfg.metrics.corrupted_blocks.fetch_add(1, Ordering::Relaxed);
                    let (expected, actual) = match &e {
                        block::VerifyError::HashMismatch {
                            expected, actual, ..
                        }
                        | block::VerifyError::DataCorrupted {
                            expected_hash: expected,
                            actual_hash: actual,
                            ..
                        } => (*expected, *actual),
                        _ => ([0u8; 32], [0u8; 32]),
                    };
                    let _ = cfg.events.send(EngineEvent::CorruptBlock {
                        file_index,
                        block_index: engine.slot_block_index(completed_slot),
                        expected,
                        actual,
                    });
                }
            }

            verified += 1;
            cfg.metrics
                .bytes_verified
                .fetch_add(cfg.block_size as u64, Ordering::Relaxed);
            cfg.metrics.blocks_verified.fetch_add(1, Ordering::Relaxed);

            if submitted < num_blocks && !cfg.shutdown.load(Ordering::Relaxed) {
                let offset = submitted as u64 * cfg.block_size as u64;
                if let Err(e) = engine.submit_read(completed_slot, offset, submitted) {
                    let _ = cfg.events.send(EngineEvent::ReadError {
                        file_index,
                        block_index: submitted,
                        error: format!("Read submission failed: {}", e),
                    });
                    cfg.metrics.verify_errors.fetch_add(1, Ordering::Relaxed);
                } else {
                    submitted += 1;
                }
            }
        }

        if let Err(e) = engine.close_file() {
            let _ = cfg.events.send(EngineEvent::ReadError {
                file_index,
                block_index: num_blocks,
                error: format!("File close failed: {}", e),
            });
        }

        cfg.metrics.files_completed.fetch_add(1, Ordering::Relaxed);
    }

    let _ = cfg.events.send(EngineEvent::WorkerDone);
}

fn check_pause(pause: &Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = pause.as_ref();
    let mut paused = lock.lock().unwrap();
    while *paused {
        paused = cvar.wait(paused).unwrap();
    }
}
