//! Deterministic fixture tree mirroring a filesystem root (test mode: `fs_root = root`,
//! `home = root/Users/tester`). Small, real allocated sizes; names and sizes are fixed, mtimes are
//! relative to "now" (stale project = [`STALE_AGE`] old).

use std::ffi::CString;
use std::fs::{self, File, FileTimes};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const HOME_REL: &str = "Users/tester";
/// Marker that allows [`generate`] to wipe and rewrite a directory.
pub const MARKER: &str = ".fdc-fixture";

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

macro_rules! home {
    ($p:literal) => {
        concat!("Users/tester/", $p)
    };
}

/// Regular files: (path relative to root, size in bytes). Sizes are multiples of 4 KiB, so on APFS
/// allocated == logical.
pub const FILES: &[(&str, u64)] = &[
    (
        home!(
            "Library/Developer/Xcode/DerivedData/AppA-abc123/Build/Products/Debug-iphonesimulator/AppA.app/AppA"
        ),
        4 * MIB,
    ),
    (
        home!(
            "Library/Developer/Xcode/DerivedData/AppA-abc123/Build/Intermediates.noindex/AppA.build/main.o"
        ),
        2 * MIB,
    ),
    (
        home!(
            "Library/Developer/Xcode/DerivedData/AppA-abc123/Index.noindex/DataStore/v5/units.bin"
        ),
        MIB,
    ),
    (
        home!("Library/Developer/Xcode/DerivedData/AppB-def456/Build/Products/Debug/AppB"),
        3 * MIB,
    ),
    (
        home!("Library/Developer/Xcode/DerivedData/AppB-def456/Logs/Build/build.xcactivitylog"),
        256 * KIB,
    ),
    (
        home!(
            "Library/Developer/Xcode/Archives/2026-01-15/AppA 15-01-2026.xcarchive/Products/Applications/AppA.app/AppA"
        ),
        2 * MIB,
    ),
    (
        home!("Library/Developer/Xcode/Archives/2026-01-15/AppA 15-01-2026.xcarchive/Info.plist"),
        4 * KIB,
    ),
    (
        home!("Library/Developer/Xcode/iOS DeviceSupport/17.5 (21F79)/Symbols/usr/lib/dyld"),
        8 * MIB,
    ),
    (
        home!(
            "Library/Developer/CoreSimulator/Caches/dyld/24A335/com.apple.CoreSimulator.SimRuntime.iOS-17-5.21F79/dyld_sim_shared_cache_arm64e"
        ),
        4 * MIB,
    ),
    (
        home!(
            "Library/Developer/CoreSimulator/Devices/1F3B5A7C-0000-4000-8000-00000000A001/device.plist"
        ),
        4 * KIB,
    ),
    (
        home!(
            "Library/Developer/CoreSimulator/Devices/1F3B5A7C-0000-4000-8000-00000000A001/data/Containers/Data/Application/app.db"
        ),
        2 * MIB,
    ),
    (
        home!(
            "Library/Developer/CoreSimulator/Devices/1F3B5A7C-0000-4000-8000-00000000A002/device.plist"
        ),
        4 * KIB,
    ),
    (
        home!(
            "Library/Developer/CoreSimulator/Devices/1F3B5A7C-0000-4000-8000-00000000A002/data/Library/Caches/cache.bin"
        ),
        MIB,
    ),
    (home!("Library/Caches/com.apple.dt.Xcode/Cache.db"), MIB),
    (
        home!("Library/Caches/Homebrew/downloads/0a1b2c--node-22.9.0.arm64_sonoma.bottle.tar.gz"),
        3 * MIB,
    ),
    (
        home!("Library/Caches/Yarn/v6/npm-react-18.3.1-integrity/node_modules/react/index.js"),
        512 * KIB,
    ),
    (
        home!("Library/Caches/CocoaPods/Pods/Release/Alamofire/5.9.1/Source/Session.swift"),
        512 * KIB,
    ),
    (
        home!("Library/pnpm/store/v3/files/00/1a2b3c4d5e"),
        512 * KIB,
    ),
    (
        home!("Library/pnpm/global/5/node_modules/typescript/package.json"),
        4 * KIB,
    ),
    // Global npm root of an nvm Node: must never be offered as a project node_modules.
    (
        home!(".nvm/versions/node/v22.0.0/lib/node_modules/corepack/dist/yarn.js"),
        256 * KIB,
    ),
    (home!(".npm/_cacache/content-v2/sha512/ab/cdef0123"), MIB),
    (
        home!(".gradle/caches/modules-2/files-2.1/com.google.guava/guava/33.0.0/guava-33.0.0.jar"),
        2 * MIB,
    ),
    (
        home!(".gradle/wrapper/dists/gradle-8.7-bin/abc123/gradle-8.7-bin.zip"),
        2 * MIB,
    ),
    (home!(".android/avd/Pixel_8.ini"), 4 * KIB),
    (home!(".android/avd/Pixel_8.avd/userdata-qemu.img"), 4 * MIB),
    (
        home!("Library/Android/sdk/system-images/android-34/google_apis/arm64-v8a/system.img"),
        8 * MIB,
    ),
    (
        home!("Library/Mobile Documents/com~apple~CloudDocs/notes.txt"),
        64 * KIB,
    ),
    (home!("Library/Keychains/login.keychain-db"), 64 * KIB),
    (home!("Library/Logs/old.log"), 128 * KIB),
    (home!("Projects/fresh-app/package.json"), 4 * KIB),
    (home!("Projects/fresh-app/yarn.lock"), 64 * KIB),
    (home!("Projects/fresh-app/src/index.js"), 4 * KIB),
    (
        home!("Projects/fresh-app/node_modules/react/package.json"),
        4 * KIB,
    ),
    (
        home!("Projects/fresh-app/node_modules/react/index.js"),
        256 * KIB,
    ),
    (home!("Projects/stale-app/package.json"), 4 * KIB),
    (home!("Projects/stale-app/package-lock.json"), 64 * KIB),
    (
        home!("Projects/stale-app/node_modules/lodash/package.json"),
        4 * KIB,
    ),
    (
        home!("Projects/stale-app/node_modules/lodash/lodash.js"),
        512 * KIB,
    ),
    (home!("Projects/hardlinks/a.bin"), MIB),
    (home!("Projects/clones/orig.bin"), MIB),
    (home!("Projects/locked/secret.bin"), 64 * KIB),
    (home!("Documents/unknown-big-folder/blob1.bin"), 8 * MIB),
    (home!("Documents/unknown-big-folder/blob2.bin"), 4 * MIB),
    ("System/Library/CoreServices/fake.bin", 256 * KIB),
    ("private/var/vm/sleepimage", 2 * MIB),
    ("private/var/db/fake.db", 64 * KIB),
    (
        "Library/Developer/CoreSimulator/Volumes/iOS_21F79/fake.bin",
        MIB,
    ),
    ("Library/Caches/com.example/cache.bin", 256 * KIB),
    ("Library/Logs/DiagnosticReports/x.ips", 64 * KIB),
];

/// Hardlink to `Projects/hardlinks/a.bin` (counted once).
pub const HARDLINK: &str = home!("Projects/hardlinks/b.bin");
/// APFS clone of `Projects/clones/orig.bin` (counted once).
pub const CLONE: &str = home!("Projects/clones/clone.bin");
/// Symlink pointing outside the fixture root.
pub const ESCAPE_LINK: &str = home!("Projects/escape-link");
pub const ESCAPE_TARGET: &str = "/tmp";
/// Directory with mode 000 (unreadable).
pub const LOCKED_DIR: &str = home!("Projects/locked");
/// Sparse file: large logical length, [`SPARSE_WRITTEN`] bytes actually allocated.
pub const SPARSE: &str = home!("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
pub const SPARSE_LOGICAL: u64 = 64 * 1024 * MIB;
pub const SPARSE_WRITTEN: u64 = MIB;
/// Everything under this dir (files and dirs) gets mtime/atime = now - [`STALE_AGE`].
pub const STALE_PROJECT: &str = home!("Projects/stale-app");
pub const STALE_AGE: Duration = Duration::from_secs(120 * 24 * 3600);

/// Creates the fixture tree in `root`. Refuses a non-empty directory without the [`MARKER`]; a
/// marked directory is wiped (its contents only) and regenerated.
pub fn generate(root: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    if fs::read_dir(root)?.next().is_some() {
        if !root.join(MARKER).is_file() {
            return Err(io::Error::other(format!(
                "{} is not empty and has no {MARKER} marker; refusing to write",
                root.display()
            )));
        }
        // Restore perms so the wipe can get into the locked dir.
        let _ = fs::set_permissions(root.join(LOCKED_DIR), fs::Permissions::from_mode(0o755));
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            // remove_dir_all does not follow symlinks; escape-link is removed, /tmp untouched.
            if fs::symlink_metadata(&path)?.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
    }
    fs::write(
        root.join(MARKER),
        b"FreeDiskCleaner test fixture. Safe to delete.\n",
    )?;

    for (i, &(rel, size)) in FILES.iter().enumerate() {
        write_file(&root.join(rel), size, i as u8)?;
    }
    fs::hard_link(
        root.join(home!("Projects/hardlinks/a.bin")),
        root.join(HARDLINK),
    )?;
    clonefile(
        &root.join(home!("Projects/clones/orig.bin")),
        &root.join(CLONE),
    )?;
    std::os::unix::fs::symlink(ESCAPE_TARGET, root.join(ESCAPE_LINK))?;

    let sparse = root.join(SPARSE);
    fs::create_dir_all(sparse.parent().unwrap())?;
    let f = File::create(&sparse)?;
    f.set_len(SPARSE_LOGICAL)?;
    (&f).write_all(&vec![0xD0; SPARSE_WRITTEN as usize])?;

    set_times_recursive(&root.join(STALE_PROJECT), SystemTime::now() - STALE_AGE)?;
    fs::set_permissions(root.join(LOCKED_DIR), fs::Permissions::from_mode(0o000))
}

fn write_file(path: &Path, size: u64, seed: u8) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    // Non-zero, per-file pattern so nothing is sparse or deduplicated.
    fs::write(path, vec![seed.wrapping_add(1); size as usize])
}

fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: two valid NUL-terminated paths.
    if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Children first, so setting a child's times does not bump the parent afterwards.
fn set_times_recursive(path: &Path, t: SystemTime) -> io::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            set_times_recursive(&entry?.path(), t)?;
        }
    }
    File::open(path)?.set_times(FileTimes::new().set_modified(t).set_accessed(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_macro_matches_home_rel() {
        assert_eq!(home!(""), format!("{HOME_REL}/"));
    }

    #[test]
    fn refuses_unmarked_non_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("precious.txt"), b"x").unwrap();
        assert!(generate(dir.path()).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn regenerates_marked_dir() {
        let dir = tempfile::tempdir().unwrap();
        generate(dir.path()).unwrap();
        fs::write(dir.path().join("leftover"), b"x").unwrap();
        generate(dir.path()).unwrap();
        assert!(!dir.path().join("leftover").exists());
        assert!(dir.path().join(MARKER).is_file());
        // Leave it deletable for tempdir cleanup.
        fs::set_permissions(
            dir.path().join(LOCKED_DIR),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
}
