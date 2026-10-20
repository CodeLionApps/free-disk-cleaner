//! Full Disk Access detection: macOS gives no API, so try to read a TCC-protected location.

use std::path::{Path, PathBuf};

/// Locations readable only with Full Disk Access.
pub fn probe_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari"),
    ]
}

/// Granted if any probe that exists can be read. `read` returns Ok(true) readable, Ok(false)
/// missing, Err on permission denied.
pub fn has_full_disk_access(
    probes: &[PathBuf],
    read: impl Fn(&Path) -> std::io::Result<bool>,
) -> bool {
    let mut any_denied = false;
    for p in probes {
        match read(p) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(_) => any_denied = true,
        }
    }
    // ponytail: nothing exists and nothing denied (fresh account) → treat as granted, scan will show errors.
    !any_denied
}

pub fn real_read(p: &Path) -> std::io::Result<bool> {
    match std::fs::metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => std::fs::read_dir(p).map(|_| true),
        Ok(_) => std::fs::File::open(p).map(|_| true),
    }
}

pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn detection() {
        let probes = probe_paths(Path::new("/h"));
        let denied = |_: &Path| Err(Error::from(ErrorKind::PermissionDenied));
        assert!(!has_full_disk_access(&probes, denied));
        assert!(has_full_disk_access(&probes, |_| Ok(true)));
        assert!(has_full_disk_access(&probes, |_| Ok(false)));
        // first denied, second readable → granted
        let mixed = |p: &Path| {
            if p.ends_with("TCC.db") {
                Err(Error::from(ErrorKind::PermissionDenied))
            } else {
                Ok(true)
            }
        };
        assert!(has_full_disk_access(&probes, mixed));
    }
}
