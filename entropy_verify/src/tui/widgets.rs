/// Custom TUI widgets: throughput sparkline, phase indicator badge.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// A rolling throughput sparkline that renders a compact bar chart
/// of recent throughput samples.
pub struct ThroughputSparkline<'a> {
    data: &'a [u64],
    max_value: u64,
    bar_color: Color,
    bg_color: Color,
}

impl<'a> ThroughputSparkline<'a> {
    pub fn new(data: &'a [u64]) -> Self {
        let max_value = data.iter().copied().max().unwrap_or(1).max(1);
        Self {
            data,
            max_value,
            bar_color: Color::Cyan,
            bg_color: Color::DarkGray,
        }
    }

    pub fn bar_color(mut self, color: Color) -> Self {
        self.bar_color = color;
        self
    }

    #[allow(dead_code)]
    pub fn bg_color(mut self, color: Color) -> Self {
        self.bg_color = color;
        self
    }
}

impl<'a> Widget for ThroughputSparkline<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let bar_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let width = area.width as usize;
        let height = area.height;

        // Take the most recent `width` samples.
        let start = if self.data.len() > width {
            self.data.len() - width
        } else {
            0
        };
        let visible = &self.data[start..];

        for (i, &value) in visible.iter().enumerate() {
            let x = area.x + i as u16;
            if x >= area.x + area.width {
                break;
            }

            // Determine bar height (fractional across the height).
            let ratio = value as f64 / self.max_value as f64;
            let bar_height = (ratio * (height as f64 * 8.0)).round() as u16;

            for row in 0..height {
                let y = area.y + height - 1 - row;
                let cell_fill = bar_height.saturating_sub(row * 8).min(8);

                let ch = if cell_fill >= 8 {
                    '█'
                } else if cell_fill > 0 {
                    bar_chars[cell_fill as usize - 1]
                } else {
                    ' '
                };

                let color = if cell_fill > 0 {
                    // Gradient from green (low) through cyan to white (peak)
                    if ratio > 0.8 {
                        Color::White
                    } else if ratio > 0.5 {
                        Color::Cyan
                    } else {
                        Color::Green
                    }
                } else {
                    self.bg_color
                };

                buf.set_string(x, y, ch.to_string(), Style::default().fg(color));
            }
        }
    }
}

/// A simple pulsing phase indicator dot.
pub struct PhaseIndicator {
    pub label: String,
    pub color: Color,
    pub tick: u64,
}

impl PhaseIndicator {
    pub fn new(label: &str, color: Color, tick: u64) -> Self {
        Self {
            label: label.to_string(),
            color,
            tick,
        }
    }
}

impl Widget for PhaseIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height == 0 {
            return;
        }

        // Pulsing dot: cycles between ● and ○ every 5 ticks.
        let dot = if (self.tick / 5) % 2 == 0 { "●" } else { "○" };
        let text = format!("{} {}", dot, self.label);

        buf.set_string(area.x, area.y, &text, Style::default().fg(self.color));
    }
}
