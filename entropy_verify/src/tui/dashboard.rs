/// Renders the complete Entropy Verify interface:
/// - Header with title, phase, and target info
/// - Progress bar with percentage
/// - Throughput sparkline and metrics
/// - Worker stats and error counter
/// - Hotkey legend

use crate::app::{App, Phase};
use crate::tui::widgets::{PhaseIndicator, ThroughputSparkline};
use crate::units::{self, UnitMode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use ratatui::Frame;

/// Render the complete dashboard.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Main vertical layout: Header | Progress | Throughput | Stats | Hotkeys
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(4),  // Header
            Constraint::Length(4),  // Progress
            Constraint::Length(6),  // Throughput
            Constraint::Length(4),  // Stats
            Constraint::Length(3),  // Hotkeys
        ])
        .split(area);

    render_header(frame, chunks[0], app);
    render_progress(frame, chunks[1], app);
    render_throughput(frame, chunks[2], app);
    render_stats(frame, chunks[3], app);
    render_hotkeys(frame, chunks[4], app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " ENTROPY VERIFY v1.0.0 ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split header into two columns.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(inner);

    // Left: Target info
    let target_info = format!(
        "Target: {}  │  FS: {}  │  Capacity: {}",
        app.volume_info.mount_point.display(),
        app.volume_info.fs_type,
        units::format_bytes(app.volume_info.total_bytes, app.unit_mode),
    );
    let target_line = Paragraph::new(Line::from(Span::styled(
        target_info,
        Style::default().fg(Color::White),
    )));
    frame.render_widget(target_line, cols[0]);

    // Right: Phase indicator
    let (phase_label, phase_color) = match app.phase {
        Phase::Idle => ("IDLE", Color::Gray),
        Phase::Writing => ("WRITING", Color::Yellow),
        Phase::Verifying => ("VERIFYING", Color::Cyan),
        Phase::Complete => ("COMPLETE", Color::Green),
        Phase::Failed => ("FAILED", Color::Red),
    };

    let indicator = PhaseIndicator::new(phase_label, phase_color, app.tick);
    frame.render_widget(indicator, cols[1]);
}

fn render_progress(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Progress ",
            Style::default().fg(Color::White),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (bytes_done, total_bytes, label_prefix) = match app.phase {
        Phase::Writing => (
            app.write_metrics_bytes(),
            app.total_bytes,
            "Written",
        ),
        Phase::Verifying => (
            app.verify_metrics_bytes(),
            app.total_bytes,
            "Verified",
        ),
        Phase::Complete => (app.total_bytes, app.total_bytes, "Complete"),
        _ => (0, app.total_bytes.max(1), "Pending"),
    };

    let ratio = if total_bytes > 0 {
        (bytes_done as f64 / total_bytes as f64).min(1.0)
    } else {
        0.0
    };

    let label = format!(
        "{}: {} / {}  ({:.1}%)",
        label_prefix,
        units::format_bytes(bytes_done, app.unit_mode),
        units::format_bytes(total_bytes, app.unit_mode),
        ratio * 100.0,
    );

    let gauge_color = match app.phase {
        Phase::Writing => Color::Yellow,
        Phase::Verifying => Color::Cyan,
        Phase::Complete => Color::Green,
        Phase::Failed => Color::Red,
        _ => Color::Gray,
    };

    let gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(gauge_color)
                .bg(Color::DarkGray),
        )
        .ratio(ratio)
        .label(label);

    frame.render_widget(gauge, inner);
}

fn render_throughput(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Throughput ",
            Style::default().fg(Color::White),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split into sparkline area and text metrics.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner);

    // Left: Sparkline
    let sparkline = ThroughputSparkline::new(&app.throughput_history)
        .bar_color(Color::Cyan);
    frame.render_widget(sparkline, cols[0]);

    // Right: Text metrics
    let elapsed = app.elapsed_secs();
    let throughput_str = units::format_throughput(app.current_throughput, app.unit_mode);
    let eta_str = if app.current_throughput > 0.0 && !matches!(app.phase, Phase::Complete | Phase::Idle) {
        let remaining_bytes = app.remaining_bytes() as f64;
        let eta_secs = (remaining_bytes / app.current_throughput) as u64;
        units::format_duration(eta_secs)
    } else if matches!(app.phase, Phase::Complete) {
        "Done".to_string()
    } else {
        "---".to_string()
    };

    let metrics_text = vec![
        Line::from(vec![
            Span::styled("Throughput  ", Style::default().fg(Color::DarkGray)),
            Span::styled(&throughput_str, Style::default().fg(Color::Cyan).bold()),
        ]),
        Line::from(vec![
            Span::styled("Elapsed     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                units::format_duration(elapsed),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("ETA         ", Style::default().fg(Color::DarkGray)),
            Span::styled(eta_str, Style::default().fg(Color::Yellow)),
        ]),
    ];

    let metrics = Paragraph::new(metrics_text);
    frame.render_widget(metrics, cols[1]);
}

fn render_stats(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Statistics ",
            Style::default().fg(Color::White),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let errors = app.total_errors();
    let error_color = if errors > 0 { Color::Red } else { Color::Green };
    let error_str = if errors > 0 {
        format!("{}", errors)
    } else {
        "0".to_string()
    };

    let paused_str = if app.is_paused { " [PAUSED]" } else { "" };
    let files_done = app.files_completed();

    let stats_text = vec![
        Line::from(vec![
            Span::styled("Workers: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", app.num_threads),
                Style::default().fg(Color::White),
            ),
            Span::styled("  │  QD: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", app.queue_depth),
                Style::default().fg(Color::White),
            ),
            Span::styled("  │  Errors: ", Style::default().fg(Color::DarkGray)),
            Span::styled(error_str, Style::default().fg(error_color).bold()),
            Span::styled(paused_str, Style::default().fg(Color::Yellow).bold()),
        ]),
        Line::from(vec![
            Span::styled("Block Size: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                units::format_bytes(app.block_size as u64, app.unit_mode),
                Style::default().fg(Color::White),
            ),
            Span::styled("  │  Files: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}/{}", files_done, app.total_files),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    let stats = Paragraph::new(stats_text).wrap(Wrap { trim: false });
    frame.render_widget(stats, inner);
}

fn render_hotkeys(frame: &mut Frame, area: Rect, app: &App) {
    let unit_label = match app.unit_mode {
        UnitMode::Decimal => "GB⟷GiB",
        UnitMode::Binary => "GiB⟷GB",
    };

    let hotkeys = Line::from(vec![
        Span::styled(" [Tab] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled(unit_label, Style::default().fg(Color::DarkGray)),
        Span::styled("  [P] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Pause", Style::default().fg(Color::DarkGray)),
        Span::styled("  [Q] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Quit", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let paragraph = Paragraph::new(hotkeys).block(block);
    frame.render_widget(paragraph, area);
}
