/// Capacity unit display modes: Decimal (SI: GB/TB) vs Binary (IEC: GiB/TiB).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitMode {
    /// SI decimal units: KB (1000), MB (10^6), GB (10^9), TB (10^12)
    Decimal,
    /// IEC binary units: KiB (1024), MiB (2^20), GiB (2^30), TiB (2^40)
    Binary,
}

impl UnitMode {
    /// Toggle between Decimal and Binary.
    pub fn toggle(self) -> Self {
        match self {
            UnitMode::Decimal => UnitMode::Binary,
            UnitMode::Binary => UnitMode::Decimal,
        }
    }

    /// Short label for display ("GB" vs "GiB").
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            UnitMode::Decimal => "GB",
            UnitMode::Binary => "GiB",
        }
    }
}

/// Format a byte count as a human-readable string.
pub fn format_bytes(bytes: u64, mode: UnitMode) -> String {
    let (divisors, suffixes) = match mode {
        UnitMode::Decimal => (
            [1_000_000_000_000u64, 1_000_000_000, 1_000_000, 1_000, 1],
            ["TB", "GB", "MB", "KB", "B"],
        ),
        UnitMode::Binary => (
            [
                1u64 << 40,
                1u64 << 30,
                1u64 << 20,
                1u64 << 10,
                1,
            ],
            ["TiB", "GiB", "MiB", "KiB", "B"],
        ),
    };

    for (divisor, suffix) in divisors.iter().zip(suffixes.iter()) {
        if bytes >= *divisor {
            if *divisor == 1 {
                return format!("{} {}", bytes, suffix);
            }
            let value = bytes as f64 / *divisor as f64;
            return if value >= 100.0 {
                format!("{:.1} {}", value, suffix)
            } else if value >= 10.0 {
                format!("{:.2} {}", value, suffix)
            } else {
                format!("{:.3} {}", value, suffix)
            };
        }
    }

    format!("{} B", bytes)
}

/// Format a throughput value (bytes per second) as a human-readable string.
pub fn format_throughput(bytes_per_sec: f64, mode: UnitMode) -> String {
    let (divisors, suffixes) = match mode {
        UnitMode::Decimal => (
            [1_000_000_000.0f64, 1_000_000.0, 1_000.0],
            ["GB/s", "MB/s", "KB/s"],
        ),
        UnitMode::Binary => (
            [
                (1u64 << 30) as f64,
                (1u64 << 20) as f64,
                (1u64 << 10) as f64,
            ],
            ["GiB/s", "MiB/s", "KiB/s"],
        ),
    };

    for (divisor, suffix) in divisors.iter().zip(suffixes.iter()) {
        if bytes_per_sec >= *divisor {
            let value = bytes_per_sec / divisor;
            return if value >= 100.0 {
                format!("{:.0} {}", value, suffix)
            } else if value >= 10.0 {
                format!("{:.1} {}", value, suffix)
            } else {
                format!("{:.2} {}", value, suffix)
            };
        }
    }

    format!("{:.0} B/s", bytes_per_sec)
}

/// Format a duration as HH:MM:SS.
pub fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// Format a raw byte count with thousands separators (e.g., "1,073,741,824").
pub fn format_bytes_raw(bytes: u64) -> String {
    let s = bytes.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes_decimal() {
        assert_eq!(format_bytes(0, UnitMode::Decimal), "0 B");
        assert_eq!(format_bytes(999, UnitMode::Decimal), "999 B");
        assert_eq!(format_bytes(1_000, UnitMode::Decimal), "1.000 KB");
        assert_eq!(format_bytes(1_500_000, UnitMode::Decimal), "1.500 MB");
        assert_eq!(format_bytes(1_000_000_000, UnitMode::Decimal), "1.000 GB");
        assert_eq!(format_bytes(2_500_000_000_000, UnitMode::Decimal), "2.500 TB");
    }

    #[test]
    fn test_format_bytes_binary() {
        assert_eq!(format_bytes(1024, UnitMode::Binary), "1.000 KiB");
        assert_eq!(format_bytes(1_073_741_824, UnitMode::Binary), "1.000 GiB");
        assert_eq!(
            format_bytes(1_099_511_627_776, UnitMode::Binary),
            "1.000 TiB"
        );
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "00:00:00");
        assert_eq!(format_duration(61), "00:01:01");
        assert_eq!(format_duration(3661), "01:01:01");
    }

    #[test]
    fn test_format_bytes_raw() {
        assert_eq!(format_bytes_raw(0), "0");
        assert_eq!(format_bytes_raw(1_073_741_824), "1,073,741,824");
    }

    #[test]
    fn test_toggle() {
        assert_eq!(UnitMode::Decimal.toggle(), UnitMode::Binary);
        assert_eq!(UnitMode::Binary.toggle(), UnitMode::Decimal);
    }
}
