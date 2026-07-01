// Entry point

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
    let mut config = if interactive {
        // tui selection
        let mut terminal = ratatui::init();
        terminal.clear()?;
        let selection = tui::drive_selection::select_drive(&mut terminal);
        ratatui::restore();

        let target_dir = match selection {
            Ok(Some(path)) => path,
            Ok(None) => {
                std::process::exit(0);
            }
            Err(e) => {
                anyhow::bail!("Drive selection failed: {}", e);
            }
        };

        Config {
            target_dir,
            block_size: config::DEFAULT_BLOCK_SIZE,
            file_size: config::DEFAULT_FILE_SIZE,
            queue_depth: config::DEFAULT_QUEUE_DEPTH,
            threads: None,
            write_only: false,
            verify_only: false,
            seed: None,
            allow_c_drive_testing: false,
        }
    } else {
        Config::parse()
    };

    // Normalize and canonicalize config.target_dir strictly to avoid relative C:\ fallbacks on Windows
    config.target_dir = safety::guardrails::normalize_and_canonicalize_target(&config.target_dir)
        .map_err(|e| anyhow::anyhow!("Failed to resolve target directory path: {}", e))?;

    let volume_info = safety::guardrails::validate_target(&config.target_dir)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let seed = config.resolve_seed()?;

    let plan = TestPlan::compute(volume_info.available_bytes, &config)?;

    // check system drive limits
    if safety::platform::is_on_system_drive(&config.target_dir)
        && plan.total_bytes >= 100 * 1024 * 1024 * 1024
        && !config.allow_c_drive_testing
    {
        anyhow::bail!(
            "SAFETY BLOCK: Target '{}' is on the system drive and test size is {} (>= 100 GiB). \
             This is blocked to protect your OS drive from wear and disk-full issues. \
             To bypass this block, explicitly pass the --allow-c-drive-testing flag.",
            config.target_dir.display(),
            units::format_bytes(plan.total_bytes, units::UnitMode::Decimal)
        );
    }

    eprintln!(
        "Entropy Verify: {} files, {} blocks, {} to test on {}",
        plan.total_files,
        plan.total_blocks,
        units::format_bytes(plan.total_bytes, units::UnitMode::Decimal),
        volume_info.mount_point.display(),
    );

    let mut app = App::new(config.clone(), plan.clone(), volume_info, seed);

    let shutdown = app.shutdown.clone();
    ctrlc::set_handler(move || {
        shutdown.store(true, Ordering::SeqCst);
    })?;

    let mut terminal = ratatui::init();
    terminal.clear()?;

    let result = run_test(&mut terminal, &mut app, &config, &plan, seed);

    ratatui::restore();

    // generate md report
    let corruptions = app.corruptions.clone();
    match report::markdown::generate_report(&app, &corruptions) {
        Ok(path) => eprintln!("📄 Report saved to: {}", path),
        Err(e) => eprintln!("⚠️  Failed to save report: {}", e),
    }

    // cleanup temp files on success
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

    if !config.verify_only {
        app.start_writing();

        // file work queue
        let (work_tx, work_rx) = crossbeam_channel::bounded::<u32>(plan.total_files as usize);
        for file_idx in 0..plan.total_files {
            work_tx.send(file_idx)?;
        }
        drop(work_tx); // Close sender so workers exit when queue is drained.

        let shared_write = Arc::new(SharedWriteMetrics::new());

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

        tui_loop(terminal, app, Some(&shared_write), None)?;

        for h in handles {
            let _ = h.join();
        }

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

        tui_loop(terminal, app, None, Some(&shared_verify))?;

        for h in handles {
            let _ = h.join();
        }

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
        
        if let Some(start) = app.write_start {
            app.write_duration_secs = start.elapsed().as_secs_f64();
        }
        app.phase = Phase::Complete;
    }

    terminal.draw(|frame| tui::dashboard::render(frame, app))?;
    std::thread::sleep(Duration::from_secs(2));

    Ok(())
}

// event loop for tui updates
fn tui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    write_metrics: Option<&Arc<SharedWriteMetrics>>,
    verify_metrics: Option<&Arc<SharedVerifyMetrics>>,
) -> anyhow::Result<()> {
    loop {
        
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

        app.drain_events();

        app.update_metrics();

        terminal.draw(|frame| tui::dashboard::render(frame, app))?;

        match tui::events::poll_event(Duration::from_millis(100))? {
            tui::events::AppAction::Quit => {
                app.shutdown.store(true, Ordering::SeqCst);
                // resume workers so they can shut down
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

        if app.all_workers_done() {
            break;
        }

        if app.shutdown.load(Ordering::Relaxed) {
            app.phase = Phase::Failed;
            return Ok(());
        }
    }

    Ok(())
}

// delete temp chunks
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
