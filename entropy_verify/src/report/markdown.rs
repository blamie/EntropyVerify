/// Produces a detailed `entropy_verify_report_[timestamp].md` file
/// with drive info, performance metrics, and any corruption details.

use crate::app::{App, Phase};
use crate::units::{self, UnitMode};
use chrono::Local;
use std::io::Write;

/// Data about a corrupted block, collected during verification.
#[derive(Debug, Clone)]
pub struct CorruptionRecord {
    pub file_index: u32,
    pub block_index: u32,
    pub expected_hash: [u8; 32],
    pub actual_hash: [u8; 32],
}

/// Generate and save the Markdown report.
pub fn generate_report(app: &App, corruptions: &[CorruptionRecord]) -> anyhow::Result<String> {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("entropy_verify_report_{}.md", timestamp);
    let report_path = app.config.target_dir.join(&filename);

    let content = build_report(app, corruptions);

    let mut file = std::fs::File::create(&report_path)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;

    Ok(report_path.display().to_string())
}

fn build_report(app: &App, corruptions: &[CorruptionRecord]) -> String {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mode = UnitMode::Decimal;

    let status = match app.phase {
        Phase::Complete if corruptions.is_empty() => "✅ **PASSED** — All blocks verified successfully.",
        Phase::Complete => "❌ **FAILED** — Data corruption detected.",
        Phase::Failed => "❌ **FAILED** — Test terminated due to errors.",
        _ => "⚠️ **INTERRUPTED** — Test did not complete.",
    };

    let seed_hex = app.seed.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<String>();

    let write_duration = app.write_duration_secs;
    let verify_duration = app.verify_duration_secs;

    let write_throughput_avg = if write_duration > 0.0 {
        app.total_bytes_written as f64 / write_duration
    } else {
        0.0
    };

    let verify_throughput_avg = if verify_duration > 0.0 {
        app.total_bytes_verified as f64 / verify_duration
    } else {
        0.0
    };

    let mut report = String::with_capacity(4096);

    // Title
    report.push_str("# Entropy Verify Report\n\n");
    report.push_str(&format!("**Date:** {}  \n", now));
    report.push_str("**Tool Version:** 1.0.0  \n");
    report.push_str(&format!("**Status:** {}  \n\n", status));

    // Drive Information
    report.push_str("## Drive Information\n\n");
    report.push_str("| Property | Value |\n");
    report.push_str("|---|---|\n");
    report.push_str(&format!(
        "| Path | `{}` |\n",
        app.volume_info.mount_point.display()
    ));
    report.push_str(&format!("| Filesystem | {} |\n", app.volume_info.fs_type));
    report.push_str(&format!(
        "| Volume Label | {} |\n",
        if app.volume_info.label.is_empty() {
            "(none)"
        } else {
            &app.volume_info.label
        }
    ));
    report.push_str(&format!(
        "| Reported Capacity | {} ({}) |\n",
        units::format_bytes_raw(app.volume_info.total_bytes),
        units::format_bytes(app.volume_info.total_bytes, mode)
    ));
    report.push_str(&format!(
        "| Tested Capacity | {} ({}) |\n",
        units::format_bytes_raw(app.total_bytes),
        units::format_bytes(app.total_bytes, mode)
    ));
    report.push('\n');

    // Performance Summary
    report.push_str("## Performance Summary\n\n");
    report.push_str("| Metric | Write Phase | Verify Phase |\n");
    report.push_str("|---|---|---|\n");
    report.push_str(&format!(
        "| Duration | {} | {} |\n",
        units::format_duration(write_duration as u64),
        units::format_duration(verify_duration as u64),
    ));
    report.push_str(&format!(
        "| Throughput (avg) | {} | {} |\n",
        units::format_throughput(write_throughput_avg, mode),
        units::format_throughput(verify_throughput_avg, mode),
    ));
    report.push_str(&format!(
        "| Peak Throughput | {} | {} |\n",
        units::format_throughput(app.peak_write_throughput, mode),
        units::format_throughput(app.peak_verify_throughput, mode),
    ));
    report.push_str(&format!(
        "| Total Data | {} | {} |\n",
        units::format_bytes(app.total_bytes_written, mode),
        units::format_bytes(app.total_bytes_verified, mode),
    ));
    report.push('\n');

    // Configuration
    report.push_str("## Configuration\n\n");
    report.push_str("| Parameter | Value |\n");
    report.push_str("|---|---|\n");
    report.push_str(&format!(
        "| Block Size | {} |\n",
        units::format_bytes(app.block_size as u64, UnitMode::Binary)
    ));
    report.push_str(&format!(
        "| File Size | {} |\n",
        units::format_bytes(app.config.file_size, UnitMode::Binary)
    ));
    report.push_str(&format!("| Queue Depth | {} |\n", app.queue_depth));
    report.push_str(&format!("| Worker Threads | {} |\n", app.num_threads));
    report.push_str(&format!("| AES-CTR Seed | `0x{}` |\n", seed_hex));
    report.push('\n');

    // Corrupted Blocks
    report.push_str("## Corrupted Blocks\n\n");
    if corruptions.is_empty() {
        report.push_str("> None detected. ✅\n\n");
    } else {
        report.push_str(&format!(
            "**{} corrupted block(s) detected:**\n\n",
            corruptions.len()
        ));
        report.push_str("| File Index | Block Index | Expected Hash (first 8 bytes) | Actual Hash (first 8 bytes) |\n");
        report.push_str("|---|---|---|---|\n");
        for c in corruptions {
            let expected_hex: String = c.expected_hash[..8]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            let actual_hex: String = c.actual_hash[..8]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            report.push_str(&format!(
                "| {} | {} | `{}…` | `{}…` |\n",
                c.file_index, c.block_index, expected_hex, actual_hex
            ));
        }
        report.push('\n');
    }

    // Footer
    report.push_str("---\n\n");
    report.push_str("*Generated by [Entropy Verify](https://github.com/blamie/EntropyVerify) v1.0.0*\n");

    report
}
