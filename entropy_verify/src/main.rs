/// Entropy Verify — Entry Point
///
/// Ultra-fast cross-platform storage validation utility.
/// A modern Rust alternative to H2testw, optimized for NVMe SSDs.

mod app;
mod config;
mod crypto;
mod engine;
mod report;
mod safety;
mod threading;
mod tui;
mod units;

use app::{App, Phase};
use clap::Parser;
use config::{Config, TestPlan};
use crypto::datagen::BlockGenerator;
use engine::writer::{SharedWriteMetrics, WriteWorkerConfig};
use engine::verifier::{SharedVerifyMetrics, VerifyWorkerConfig};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let interactive = std::env::args().len() == 1;
    if let Err(e) = run(interactive) {
        // If the TUI is active, it will have been restored before we get here.
        eprintln!("\n❌ Entropy Verify failed: {:#}", e);
        if interactive {
            press_enter_to_exit();
        }
        std::process::exit(1);
    }
}

fn press_enter_to_exit() {
    use std::io::Write;
    print!("\nPress Enter to exit...");
    let _ = std::io::stdout().flush();
    let mut temp = String::new();
    let _ = std::io::stdin().read_line(&mut temp);
}

fn run(interactive: bool) -> anyhow::Result<()> {
    let config = if interactive {
        use std::io::Write;
        println!("Entropy Verify — Storage Validation Utility");
        println!("-------------------------------------------");
        print!("Enter the target directory to test (e.g., E:\\): ");
        std::io::stdout().flush()?;

        let mut target_dir = String::new();
        std::io::stdin().read_line(&mut target_dir)?;
        let target_dir = target_dir.trim().to_string();
        if target_dir.is_empty() {
            anyhow::bail!("No target directory specified.");
        }

        Config {
            target_dir: std::path::PathBuf::from(target_dir),
            block_size: config::DEFAULT_BLOCK_SIZE,
            file_size: config::DEFAULT_FILE_SIZE,
            queue_depth: config::DEFAULT_QUEUE_DEPTH,
            threads: None,
            write_only: false,
            verify_only: false,
            seed: None,
        }
    } else {
        Config::parse()
    };

    // --- Safety validation ---
    let volume_info = safety::guardrails::validate_target(&config.target_dir)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // --- Resolve seed ---
    let seed = config.resolve_seed()?;

    // --- Compute test plan ---
    let plan = TestPlan::compute(volume_info.available_bytes, &config)?;

    eprintln!(
        "Entropy Verify: {} files, {} blocks, {} to test on {}",
        plan.total_files,
        plan.total_blocks,
        units::format_bytes(plan.total_bytes, units::UnitMode::Decimal),
        volume_info.mount_point.display(),
    );

    // --- Initialize app state ---
    let mut app = App::new(config.clone(), plan.clone(), volume_info, seed);

    // --- Register Ctrl-C handler ---
    let shutdown = app.shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown.store(true, Ordering::SeqCst);
    })?;

    // --- Initialize TUI ---
    let mut terminal = ratatui::init();
    terminal.clear()?;

    // --- Run the test ---
    let result = run_test(&mut terminal, &mut app, &config, &plan, seed);

    // --- Restore terminal ---
    ratatui::restore();

    // --- Generate report (even on error) ---
    let corruptions = app.corruptions.clone();
    match report::markdown::generate_report(&app, &corruptions) {
        Ok(path) => eprintln!("📄 Report saved to: {}", path),
        Err(e) => eprintln!("⚠️  Failed to save report: {}", e),
    }

    // --- Cleanup on success ---
    if app.phase == Phase::Complete && corruptions.is_empty() {
        eprintln!("🧹 Cleaning up test files...");
        cleanup_test_files(&config.target_dir, &plan);
        eprintln!("✅ All test files deleted.");
    } else if !corruptions.is_empty() {
        eprintln!(
            "⚠️  {} corrupted block(s) detected. Test files retained for analysis.",
            corruptions.len()
        );
    }

    if interactive {
        press_enter_to_exit();
    }

    // Propagate any run_test error.
    result
}

fn run_test(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    config: &Config,
    plan: &TestPlan,
    seed: [u8; 16],
) -> anyhow::Result<()> {
    let generator = BlockGenerator::new(seed);
    let num_threads = config.num_threads();
    let total_cores = threading::affinity::available_cores();

    // ==================== WRITE PHASE ====================
    if !config.verify_only {
        app.start_writing();

        // Create work queue with file indices.
        let (work_tx, work_rx) = crossbeam_channel::bounded::<u32>(plan.total_files as usize);
        for file_idx in 0..plan.total_files {
            work_tx.send(file_idx)?;
        }
        drop(work_tx); // Close sender so workers exit when queue is drained.

        // Build shared write metrics that workers will use directly.
        let shared_write = Arc::new(SharedWriteMetrics::new());

        // Spawn worker threads.
        app.total_workers = num_threads as u32;
        let mut handles = Vec::with_capacity(num_threads);

        for i in 0..num_threads {
            let worker_cfg = WriteWorkerConfig {
                target_dir: config.target_dir.clone(),
                block_size: config.block_size as usize,
                queue_depth: config.queue_depth as usize,
                plan: plan.clone(),
                generator: generator.clone(),
                thread_id: i as u16,
                metrics: shared_write.clone(),
                events: app.event_tx.clone(),
                shutdown: app.shutdown.clone(),
                pause: app.pause.clone(),
                work_rx: work_rx.clone(),
            };

            let core_idx = threading::affinity::core_for_worker(i, total_cores);
            let handle = std::thread::Builder::new()
                .name(format!("vv-write-{}", i))
                .spawn(move || {
                    threading::affinity::pin_to_core(core_idx);
                    engine::writer::write_worker(worker_cfg);
                })?;
            handles.push(handle);
        }

        // TUI loop for write phase.
        tui_loop(terminal, app, Some(&shared_write), None)?;

        // Wait for all writer threads to finish.
        for h in handles {
            let _ = h.join();
        }

        // Sync write metrics into the app.
        app.total_bytes_written = shared_write.bytes_written.load(Ordering::Relaxed);
        app.write_metrics.bytes_written.store(
            app.total_bytes_written,
            Ordering::Relaxed,
        );
        app.write_metrics.files_completed.store(
            shared_write.files_completed.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );

        if app.shutdown.load(Ordering::Relaxed) {
            app.phase = Phase::Failed;
            return Ok(());
        }
    }

    // ==================== VERIFY PHASE ====================
    if !config.write_only && app.phase != Phase::Failed {
        app.start_verifying();

        let (work_tx, work_rx) = crossbeam_channel::bounded::<u32>(plan.total_files as usize);
        for file_idx in 0..plan.total_files {
            work_tx.send(file_idx)?;
        }
        drop(work_tx);

        let shared_verify = Arc::new(SharedVerifyMetrics::new());

        app.total_workers = num_threads as u32;
        app.workers_done = 0;
        let mut handles = Vec::with_capacity(num_threads);

        for i in 0..num_threads {
            let worker_cfg = VerifyWorkerConfig {
                target_dir: config.target_dir.clone(),
                block_size: config.block_size as usize,
                queue_depth: config.queue_depth as usize,
                plan: plan.clone(),
                generator: generator.clone(),
                thread_id: i as u16,
                metrics: shared_verify.clone(),
                events: app.event_tx.clone(),
                shutdown: app.shutdown.clone(),
                pause: app.pause.clone(),
                work_rx: work_rx.clone(),
            };

            let core_idx = threading::affinity::core_for_worker(i, total_cores);
            let handle = std::thread::Builder::new()
                .name(format!("vv-verify-{}", i))
                .spawn(move || {
                    threading::affinity::pin_to_core(core_idx);
                    engine::verifier::verify_worker(worker_cfg);
                })?;
            handles.push(handle);
        }

        // TUI loop for verify phase.
        tui_loop(terminal, app, None, Some(&shared_verify))?;

        for h in handles {
            let _ = h.join();
        }

        // Sync verify metrics.
        app.total_bytes_verified = shared_verify.bytes_verified.load(Ordering::Relaxed);
        app.verify_metrics.bytes_verified.store(
            app.total_bytes_verified,
            Ordering::Relaxed,
        );
        app.verify_metrics.files_completed.store(
            shared_verify.files_completed.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );

        if app.shutdown.load(Ordering::Relaxed) {
            app.phase = Phase::Failed;
            return Ok(());
        }

        app.complete();
    } else if !config.verify_only {
        // Write-only mode: mark complete after writing.
        if let Some(start) = app.write_start {
            app.write_duration_secs = start.elapsed().as_secs_f64();
        }
        app.phase = Phase::Complete;
    }

    // Final render to show completion state.
    terminal.draw(|frame| tui::dashboard::render(frame, app))?;
    std::thread::sleep(Duration::from_secs(2));

    Ok(())
}

/// Run the TUI event loop, updating metrics from the shared atomic counters.
/// Trait-erased: accepts either write or verify metrics (one will be None).
fn tui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    write_metrics: Option<&Arc<SharedWriteMetrics>>,
    verify_metrics: Option<&Arc<SharedVerifyMetrics>>,
) -> anyhow::Result<()> {
    loop {
        // Sync atomic metrics into the app's own counters for the TUI.
        if let Some(wm) = write_metrics {
            app.write_metrics
                .bytes_written
                .store(wm.bytes_written.load(Ordering::Relaxed), Ordering::Relaxed);
            app.write_metrics
                .files_completed
                .store(wm.files_completed.load(Ordering::Relaxed), Ordering::Relaxed);
            app.write_metrics
                .errors
                .store(wm.write_errors.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        if let Some(vm) = verify_metrics {
            app.verify_metrics
                .bytes_verified
                .store(vm.bytes_verified.load(Ordering::Relaxed), Ordering::Relaxed);
            app.verify_metrics
                .files_completed
                .store(vm.files_completed.load(Ordering::Relaxed), Ordering::Relaxed);
            app.verify_metrics
                .errors
                .store(vm.verify_errors.load(Ordering::Relaxed), Ordering::Relaxed);
            app.verify_metrics
                .corrupted
                .store(vm.corrupted_blocks.load(Ordering::Relaxed), Ordering::Relaxed);
        }

        // Process engine events (corruption reports, worker completions, etc.)
        app.drain_events();

        // Update throughput calculation.
        app.update_metrics();

        // Render the TUI.
        terminal.draw(|frame| tui::dashboard::render(frame, app))?;

        // Poll for keyboard events (100ms timeout = 10 FPS).
        match tui::events::poll_event(Duration::from_millis(100))? {
            tui::events::AppAction::Quit => {
                app.shutdown.store(true, Ordering::SeqCst);
                // Unpause workers so they can exit.
                let (lock, cvar) = app.pause.as_ref();
                let mut paused = lock.lock().unwrap();
                *paused = false;
                cvar.notify_all();
                drop(paused);
                app.phase = Phase::Failed;
                return Ok(());
            }
            tui::events::AppAction::ToggleUnits => {
                app.unit_mode = app.unit_mode.toggle();
            }
            tui::events::AppAction::TogglePause => {
                app.toggle_pause();
            }
            tui::events::AppAction::Tick => {}
        }

        // Check if all workers finished.
        if app.all_workers_done() {
            break;
        }

        // Check for shutdown.
        if app.shutdown.load(Ordering::Relaxed) {
            app.phase = Phase::Failed;
            return Ok(());
        }
    }

    Ok(())
}

/// Delete all test files generated by Entropy Verify.
fn cleanup_test_files(target_dir: &std::path::Path, plan: &TestPlan) {
    for file_idx in 0..plan.total_files {
        let path = target_dir.join(TestPlan::file_name(file_idx));
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("Warning: failed to delete {}: {}", path.display(), e);
            }
        }
    }
}
