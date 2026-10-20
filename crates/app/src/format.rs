//! Human-readable sizes (decimal, like Finder) and dates.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// `YYYY-MM-DD` (UTC), empty for unknown.
pub fn date(t: Option<SystemTime>) -> String {
    let Some(secs) = t
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
    else {
        return String::new();
    };
    // Civil-from-days (Howard Hinnant).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(5_000_000_000), "5.0 GB");
        assert_eq!(size(2_340_000_000_000), "2.3 TB");
    }

    #[test]
    fn dates() {
        assert_eq!(date(None), "");
        assert_eq!(date(Some(UNIX_EPOCH)), "1970-01-01");
        assert_eq!(
            date(Some(UNIX_EPOCH + Duration::from_secs(1_792_454_400))),
            "2026-10-20"
        );
        assert_eq!(
            date(Some(UNIX_EPOCH + Duration::from_secs(951_782_400))),
            "2000-02-29"
        );
    }
}
