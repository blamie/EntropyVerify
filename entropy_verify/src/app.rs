// state management

use crate::config::{Config, TestPlan};
use crate::safety::platform::VolumeInfo;
use crate::units::UnitMode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Writing,
    Verifying,
    Complete,
    Failed,
}

// events from workers
#[derive(Debug)]
pub enum EngineEvent {
    WriteError {
        file_index: u32,
        block_index: u32,
        error: String,
    },
    ReadError {
        file_index: u32,
        block_index: u32,
        error: String,
    },
    CorruptBlock {
        file_index: u32,
        block_index: u32,
        expected: [u8; 32],
        actual: [u8; 32],
    },
    WorkerDone,
    FatalError(String),
}

pub struct App {
    
    pub config: Config,
    pub volume_info: VolumeInfo,
    pub seed: [u8; 16],

    pub phase: Phase,

    pub write_metrics: Arc<WriteMetrics>,
    pub verify_metrics: Arc<VerifyMetrics>,

    pub shutdown: Arc<AtomicBool>,
    pub pause: Arc<(Mutex<bool>, Condvar)>,
    pub is_paused: bool,

    pub event_rx: crossbeam_channel::Receiver<EngineEvent>,
    pub event_tx: crossbeam_channel::Sender<EngineEvent>,

    pub unit_mode: UnitMode,
    pub tick: u64,
    pub throughput_history: Vec<u64>,
    pub current_throughput: f64,
    pub last_bytes_sample: u64,
    pub last_sample_time: Instant,
    pub start_time: Instant,

    pub write_start: Option<Instant>,
    pub write_duration_secs: f64,
    pub verify_start: Option<Instant>,
    pub verify_duration_secs: f64,

    pub total_bytes: u64,
    pub total_files: u32,
    pub total_bytes_written: u64,
    pub total_bytes_verified: u64,
    pub peak_write_throughput: f64,
    pub peak_verify_throughput: f64,

    pub corruptions: Vec<crate::report::markdown::CorruptionRecord>,

    pub num_threads: usize,
    pub queue_depth: u32,
    pub block_size: usize,
    pub workers_done: u32,
    pub total_workers: u32,
}

pub struct WriteMetrics {
    pub bytes_written: AtomicU64,
    #[allow(dead_code)]
    pub blocks_written: AtomicU64,
    pub files_completed: AtomicU64,
    pub errors: AtomicU64,
}

impl WriteMetrics {
    pub fn new() -> Self {
        Self {
            bytes_written: AtomicU64::new(0),
            blocks_written: AtomicU64::new(0),
            files_completed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

pub struct VerifyMetrics {
    pub bytes_verified: AtomicU64,
    #[allow(dead_code)]
    pub blocks_verified: AtomicU64,
    pub files_completed: AtomicU64,
    pub errors: AtomicU64,
    pub corrupted: AtomicU64,
}

impl VerifyMetrics {
    pub fn new() -> Self {
        Self {
            bytes_verified: AtomicU64::new(0),
            blocks_verified: AtomicU64::new(0),
            files_completed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            corrupted: AtomicU64::new(0),
        }
    }
}

impl App {
    
    pub fn new(config: Config, plan: TestPlan, volume_info: VolumeInfo, seed: [u8; 16]) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let num_threads = config.num_threads();
        let queue_depth = config.queue_depth;
        let block_size = config.block_size as usize;
        let total_bytes = plan.total_bytes;
        let total_files = plan.total_files;
        let now = Instant::now();

        App {
            config,
            volume_info,
            seed,
            phase: Phase::Idle,
            write_metrics: Arc::new(WriteMetrics::new()),
            verify_metrics: Arc::new(VerifyMetrics::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            pause: Arc::new((Mutex::new(false), Condvar::new())),
            is_paused: false,
            event_rx,
            event_tx,
            unit_mode: UnitMode::Decimal,
            tick: 0,
            throughput_history: Vec::with_capacity(120),
            current_throughput: 0.0,
            last_bytes_sample: 0,
            last_sample_time: now,
            start_time: now,
            write_start: None,
            write_duration_secs: 0.0,
            verify_start: None,
            verify_duration_secs: 0.0,
            total_bytes,
            total_files,
            total_bytes_written: 0,
            total_bytes_verified: 0,
            peak_write_throughput: 0.0,
            peak_verify_throughput: 0.0,
            corruptions: Vec::new(),
            num_threads,
            queue_depth,
            block_size,
            workers_done: 0,
            total_workers: 0,
        }
    }

    // throughput tick calculation
    pub fn update_metrics(&mut self) {
        let now = Instant::now();
        let current_bytes = match self.phase {
            Phase::Writing => self.write_metrics.bytes_written.load(Ordering::Relaxed),
            Phase::Verifying => self.verify_metrics.bytes_verified.load(Ordering::Relaxed),
            _ => self.last_bytes_sample,
        };

        let elapsed = now.duration_since(self.last_sample_time).as_secs_f64();
        if elapsed > 0.05 {
            // min 50ms to avoid jitter
            let delta_bytes = current_bytes.saturating_sub(self.last_bytes_sample);
            self.current_throughput = delta_bytes as f64 / elapsed;
            self.last_bytes_sample = current_bytes;
            self.last_sample_time = now;

            match self.phase {
                Phase::Writing => {
                    if self.current_throughput > self.peak_write_throughput {
                        self.peak_write_throughput = self.current_throughput;
                    }
                }
                Phase::Verifying => {
                    if self.current_throughput > self.peak_verify_throughput {
                        self.peak_verify_throughput = self.current_throughput;
                    }
                }
                _ => {}
            }

            // keep 120 samples for sparkline
            let throughput_mbps = (self.current_throughput / 1_000_000.0) as u64;
            self.throughput_history.push(throughput_mbps);
            if self.throughput_history.len() > 120 {
                self.throughput_history.remove(0);
            }
        }

        self.tick += 1;
    }

    pub fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                EngineEvent::WriteError { file_index, block_index, error } => {
                    self.write_metrics.errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "Write error at file {} block {}: {}",
                        file_index, block_index, error
                    );
                }
                EngineEvent::ReadError { file_index, block_index, error } => {
                    self.verify_metrics.errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "Read error at file {} block {}: {}",
                        file_index, block_index, error
                    );
                }
                EngineEvent::CorruptBlock {
                    file_index,
                    block_index,
                    expected,
                    actual,
                } => {
                    self.corruptions.push(crate::report::markdown::CorruptionRecord {
                        file_index,
                        block_index,
                        expected_hash: expected,
                        actual_hash: actual,
                    });
                }
                EngineEvent::WorkerDone => {
                    self.workers_done += 1;
                }
                EngineEvent::FatalError(msg) => {
                    eprintln!("FATAL: {}", msg);
                    self.phase = Phase::Failed;
                }
            }
        }
    }

    pub fn all_workers_done(&self) -> bool {
        self.workers_done >= self.total_workers
    }

    pub fn start_writing(&mut self) {
        self.phase = Phase::Writing;
        self.write_start = Some(Instant::now());
        self.workers_done = 0;
        self.last_bytes_sample = 0;
        self.last_sample_time = Instant::now();
    }

    pub fn start_verifying(&mut self) {
        
        if let Some(start) = self.write_start {
            self.write_duration_secs = start.elapsed().as_secs_f64();
        }
        self.total_bytes_written = self.write_metrics.bytes_written.load(Ordering::Relaxed);

        self.phase = Phase::Verifying;
        self.verify_start = Some(Instant::now());
        self.workers_done = 0;
        self.last_bytes_sample = 0;
        self.last_sample_time = Instant::now();
        self.throughput_history.clear();
    }

    pub fn complete(&mut self) {
        if let Some(start) = self.verify_start {
            self.verify_duration_secs = start.elapsed().as_secs_f64();
        }
        self.total_bytes_verified = self.verify_metrics.bytes_verified.load(Ordering::Relaxed);
        self.phase = Phase::Complete;
    }

    pub fn toggle_pause(&mut self) {
        self.is_paused = !self.is_paused;
        let (lock, cvar) = self.pause.as_ref();
        let mut paused = lock.lock().unwrap();
        *paused = self.is_paused;
        if !self.is_paused {
            cvar.notify_all();
        }
    }

    pub fn elapsed_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub fn write_metrics_bytes(&self) -> u64 {
        self.write_metrics.bytes_written.load(Ordering::Relaxed)
    }

    pub fn verify_metrics_bytes(&self) -> u64 {
        self.verify_metrics.bytes_verified.load(Ordering::Relaxed)
    }

    pub fn remaining_bytes(&self) -> u64 {
        match self.phase {
            Phase::Writing => self.total_bytes.saturating_sub(self.write_metrics_bytes()),
            Phase::Verifying => self.total_bytes.saturating_sub(self.verify_metrics_bytes()),
            _ => 0,
        }
    }

    pub fn total_errors(&self) -> u64 {
        self.write_metrics.errors.load(Ordering::Relaxed)
            + self.verify_metrics.errors.load(Ordering::Relaxed)
            + self.corruptions.len() as u64
    }

    pub fn files_completed(&self) -> u64 {
        match self.phase {
            Phase::Writing => self.write_metrics.files_completed.load(Ordering::Relaxed),
            Phase::Verifying => self.verify_metrics.files_completed.load(Ordering::Relaxed),
            Phase::Complete => self.total_files as u64,
            _ => 0,
        }
    }
}
