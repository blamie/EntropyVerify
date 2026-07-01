// writer engine

use crate::app::EngineEvent;
use crate::config::TestPlan;
use crate::crypto::datagen::BlockGenerator;
use crate::engine::block;
use crate::engine::io::PlatformEngine;
use crossbeam_channel::Sender;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub struct SharedWriteMetrics {
    pub bytes_written: AtomicU64,
    pub blocks_written: AtomicU64,
    pub files_completed: AtomicU64,
    pub write_errors: AtomicU64,
}

impl SharedWriteMetrics {
    pub fn new() -> Self {
        Self {
            bytes_written: AtomicU64::new(0),
            blocks_written: AtomicU64::new(0),
            files_completed: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
        }
    }
}

pub struct WriteWorkerConfig {
    pub target_dir: PathBuf,
    pub block_size: usize,
    pub queue_depth: usize,
    pub plan: TestPlan,
    pub generator: BlockGenerator,
    pub thread_id: u16,
    pub metrics: Arc<SharedWriteMetrics>,
    pub events: Sender<EngineEvent>,
    pub shutdown: Arc<AtomicBool>,
    pub pause: Arc<(Mutex<bool>, Condvar)>,
    pub work_rx: crossbeam_channel::Receiver<u32>,
}

pub fn write_worker(cfg: WriteWorkerConfig) {
    let mut engine = match PlatformEngine::new(cfg.queue_depth, cfg.block_size) {
        Ok(e) => e,
        Err(e) => {
            let _ = cfg.events.send(EngineEvent::FatalError(format!(
                "Worker {} failed to create I/O engine: {}",
                cfg.thread_id, e
            )));
            return;
        }
    };

    // work loop
    while let Ok(file_index) = cfg.work_rx.recv() {
        if cfg.shutdown.load(Ordering::Relaxed) {
            break;
        }

        let file_path = cfg
            .target_dir
            .join(TestPlan::file_name(file_index));
        let num_blocks = cfg.plan.blocks_for_file(file_index);

        if let Err(e) = engine.open_file(&file_path, true) {
            let _ = cfg.events.send(EngineEvent::WriteError {
                file_index,
                block_index: 0,
                error: format!("Failed to create file: {}", e),
            });
            cfg.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let mut had_error = false;

        for block_idx in 0..num_blocks {
            
            if cfg.shutdown.load(Ordering::Relaxed) {
                break;
            }

            check_pause(&cfg.pause);

            // get free slot
            let slot = match engine.acquire_slot() {
                Ok(s) => s,
                Err(e) => {
                    let _ = cfg.events.send(EngineEvent::WriteError {
                        file_index,
                        block_index: block_idx,
                        error: format!("Slot acquisition failed: {}", e),
                    });
                    cfg.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
                    had_error = true;
                    break;
                }
            };

            // stamp block header & payload
            {
                let buf = engine.slot_buffer_mut(slot);
                block::prepare_block(buf, file_index, block_idx, cfg.thread_id, &cfg.generator);
            }

            let offset = block_idx as u64 * cfg.block_size as u64;
            if let Err(e) = engine.submit_write(slot, offset, block_idx) {
                let _ = cfg.events.send(EngineEvent::WriteError {
                    file_index,
                    block_index: block_idx,
                    error: format!("Write submission failed: {}", e),
                });
                cfg.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
                had_error = true;
                break;
            }

            cfg.metrics
                .bytes_written
                .fetch_add(cfg.block_size as u64, Ordering::Relaxed);
            cfg.metrics.blocks_written.fetch_add(1, Ordering::Relaxed);
        }

        // flush file
        if let Err(e) = engine.close_file() {
            if !had_error {
                let _ = cfg.events.send(EngineEvent::WriteError {
                    file_index,
                    block_index: num_blocks,
                    error: format!("File close/flush failed: {}", e),
                });
                cfg.metrics.write_errors.fetch_add(1, Ordering::Relaxed);
            }
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
