/// Keyboard and terminal event handling for the TUI.

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::time::Duration;

/// Actions that can be triggered by keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    /// Quit the application gracefully.
    Quit,
    /// Toggle unit display (Decimal ↔ Binary).
    ToggleUnits,
    /// Toggle pause/resume workers.
    TogglePause,
    /// No action (tick elapsed with no relevant input).
    Tick,
}

/// Poll for keyboard events with the given timeout.
///
/// Returns `AppAction::Tick` if the timeout expires without input.
pub fn poll_event(timeout: Duration) -> std::io::Result<AppAction> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            // Only react to key press events (not release/repeat).
            if key.kind == KeyEventKind::Press {
                return Ok(match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => AppAction::Quit,
                    KeyCode::Esc => AppAction::Quit,
                    KeyCode::Tab => AppAction::ToggleUnits,
                    KeyCode::Char('p') | KeyCode::Char('P') => AppAction::TogglePause,
                    _ => AppAction::Tick,
                });
            }
        }
    }
    Ok(AppAction::Tick)
}
