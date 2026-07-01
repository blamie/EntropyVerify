use std::path::PathBuf;
use std::time::Duration;
use sysinfo::Disks;
use ratatui::{
    layout::{Constraint, Direction, Layout, Alignment},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crate::units;

struct DiskItem {
    mount_point: PathBuf,
    fs_type: String,
    label: String,
    total_bytes: u64,
    available_bytes: u64,
}

pub fn select_drive(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<Option<PathBuf>> {
    // 1. Enumerate drives
    let disks = Disks::new_with_refreshed_list();
    let disk_items: Vec<DiskItem> = disks
        .list()
        .iter()
        .map(|disk| DiskItem {
            mount_point: disk.mount_point().to_path_buf(),
            fs_type: disk.file_system().to_string_lossy().to_string(),
            label: disk.name().to_string_lossy().to_string(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
        })
        .collect();

    // If there are no disks, return an error or default to current directory
    if disk_items.is_empty() {
        return Ok(Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))));
    }

    let mut state = ListState::default();
    state.select(Some(0));

    let mut error_msg: Option<String> = None;

    loop {
        terminal.draw(|f| render_drive_selection(f, &disk_items, &mut state, &error_msg))?;

        // Poll for events
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Clear error message on any keypress
                    error_msg = None;

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            return Ok(None);
                        }
                        KeyCode::Up => {
                            let selected = state.selected().unwrap_or(0);
                            if selected > 0 {
                                state.select(Some(selected - 1));
                            } else {
                                state.select(Some(disk_items.len() - 1));
                            }
                        }
                        KeyCode::Down => {
                            let selected = state.selected().unwrap_or(0);
                            if selected < disk_items.len() - 1 {
                                state.select(Some(selected + 1));
                            } else {
                                state.select(Some(0));
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(selected) = state.selected() {
                                let selected_path = disk_items[selected].mount_point.clone();
                                // Validate the target
                                match crate::safety::guardrails::validate_target(&selected_path) {
                                    Ok(_) => {
                                        return Ok(Some(selected_path));
                                    }
                                    Err(e) => {
                                        error_msg = Some(e.to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn render_drive_selection(
    f: &mut Frame,
    disks: &[DiskItem],
    state: &mut ListState,
    error_msg: &Option<String>,
) {
    let area = f.area();

    // Layout: Title (length 3), Main List (remaining), Help bar (length 3)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    // Title paragraph
    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let title_text = Paragraph::new(Line::from(vec![
        Span::styled(" ENTROPY VERIFY ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("│ Select Target Storage Drive", Style::default().fg(Color::White)),
    ]))
    .block(title_block)
    .alignment(Alignment::Left);
    f.render_widget(title_text, chunks[0]);

    // Build the list of drive options
    let list_items: Vec<ListItem> = disks
        .iter()
        .map(|disk| {
            let label_str = if disk.label.is_empty() {
                "".to_string()
            } else {
                format!(" ({})", disk.label)
            };
            let capacity_str = units::format_bytes(disk.total_bytes, units::UnitMode::Binary);
            let free_str = units::format_bytes(disk.available_bytes, units::UnitMode::Binary);

            let content = Line::from(vec![
                Span::styled(
                    format!("  [{}]", disk.mount_point.display()),
                    Style::default().fg(Color::Yellow).bold(),
                ),
                Span::styled(label_str, Style::default().fg(Color::White).bold()),
                Span::styled("  │  FS: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&disk.fs_type, Style::default().fg(Color::White)),
                Span::styled("  │  Capacity: ", Style::default().fg(Color::DarkGray)),
                Span::styled(capacity_str, Style::default().fg(Color::Cyan)),
                Span::styled("  │  Free: ", Style::default().fg(Color::DarkGray)),
                Span::styled(free_str, Style::default().fg(Color::Green)),
            ]);
            ListItem::new(content)
        })
        .collect();

    // Target Selection block
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Available Logical Drives ", Style::default().fg(Color::White)));

    let selected_style = Style::default()
        .bg(Color::Cyan)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let list = List::new(list_items)
        .block(list_block)
        .highlight_style(selected_style)
        .highlight_symbol("> ");

    // Check if we need to show error overlay or warning popup
    if let Some(err) = error_msg {
        let sub_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(4)])
            .split(chunks[1]);

        f.render_stateful_widget(list, sub_chunks[0], state);

        let err_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(Span::styled(" Safety Validation Failed ", Style::default().fg(Color::Red).bold()));
        let err_para = Paragraph::new(format!("❌ {}", err))
            .block(err_block)
            .wrap(Wrap { trim: false })
            .fg(Color::Red);
        f.render_widget(err_para, sub_chunks[1]);
    } else {
        f.render_stateful_widget(list, chunks[1], state);
    }

    // Help hotkey legend at bottom
    let help_line = Line::from(vec![
        Span::styled(" [↑/↓] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" [Enter] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Select Drive  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" [Q/Esc] ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Quit", Style::default().fg(Color::DarkGray)),
    ]);
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let help_para = Paragraph::new(help_line).block(help_block);
    f.render_widget(help_para, chunks[2]);
}
