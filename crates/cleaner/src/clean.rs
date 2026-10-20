//! Deletion pipeline: revalidate right before deleting, then Trash (default) or direct removal.

use crate::{CommandRunner, FileSystem, Trasher, command_for, parse_docker_size};
use fdc_core::rules::{Roots, match_path};
use fdc_core::safety::{Deletable, DirectDeleteToken, Status, Target};
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

/// How a path item is removed. Direct removal needs a token, which only a Safe path item yields:
///
/// ```compile_fail
/// let _ = fdc_cleaner::Method::Direct(fdc_core::safety::DirectDeleteToken(()));
/// ```
#[derive(Debug)]
pub enum Method {
    Trash,
    Direct(DirectDeleteToken),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Size from the scan / listing.
    pub estimated: u64,
    /// Space actually freed now. Trash = 0 (freed only after the Trash is emptied).
    pub reclaimed: u64,
    pub trashed: bool,
    /// Docker command ran: its virtual disk file may not shrink right away.
    pub docker_disk_note: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CleanError {
    #[error("{0} no longer exists")]
    Vanished(PathBuf),
    #[error("{0} is now a symlink")]
    Symlink(PathBuf),
    #[error("{path} no longer matches rule {rule_id}")]
    RuleMismatch {
        path: PathBuf,
        rule_id: &'static str,
    },
    #[error("{path} is inside protected {ancestor}")]
    ProtectedAncestor { path: PathBuf, ancestor: PathBuf },
    #[error("{0} resolves outside the allowed area")]
    Escapes(PathBuf),
    #[error("{0} can only be moved to the Trash")]
    NotDirectDeletable(PathBuf),
    #[error("item is not a path")]
    NotAPath,
    #[error("item is not a command")]
    NotACommand,
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("`{command}` failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
}

pub struct Cleaner<F, T, C> {
    pub fs: F,
    pub trasher: T,
    pub runner: C,
    pub roots: Roots,
}

impl<F: FileSystem, T: Trasher, C: CommandRunner> Cleaner<F, T, C> {
    pub fn new(fs: F, trasher: T, runner: C, roots: Roots) -> Self {
        Self {
            fs,
            trasher,
            runner,
            roots,
        }
    }

    /// Deletes a path item after re-checking it against the disk and the rule table.
    pub fn delete(&self, item: &Deletable, method: Method) -> Result<Outcome, CleanError> {
        let res = self.delete_inner(item, &method);
        match &res {
            Ok(o) => tracing::info!(path = ?item.path(), ?method, ?o, "deleted"),
            Err(e) => tracing::warn!(path = ?item.path(), ?method, %e, "delete rejected"),
        }
        res
    }

    fn delete_inner(&self, item: &Deletable, method: &Method) -> Result<Outcome, CleanError> {
        let path = item.path().ok_or(CleanError::NotAPath)?;
        let direct = matches!(method, Method::Direct(_));
        // The token is not bound to an item; never let one from a Safe item delete another one.
        if direct && item.direct_delete_token().is_none() {
            return Err(CleanError::NotDirectDeletable(path.into()));
        }
        let meta = self.revalidate(path, item.info().rule_id, direct)?;
        let io_err = |source| CleanError::Io {
            path: path.into(),
            source,
        };
        let estimated = item.info().size;
        if !direct {
            self.trasher.trash(path).map_err(io_err)?;
            return Ok(Outcome {
                estimated,
                reclaimed: 0,
                trashed: true,
                docker_disk_note: false,
            });
        }
        if meta.is_dir() {
            self.fs.remove_dir_all(path).map_err(io_err)?;
        } else {
            self.fs.remove_file(path).map_err(io_err)?;
        }
        // ponytail: reclaimed = scan-time allocated size; re-measure before deleting if it drifts.
        Ok(Outcome {
            estimated,
            reclaimed: estimated,
            trashed: false,
            docker_disk_note: false,
        })
    }

    /// Scan results are stale by the time the user clicks: re-stat, re-match, re-resolve.
    // ponytail: still a TOCTOU window between these checks and the delete call; acceptable for a
    // single-user desktop app, openat-style deletion if that ever matters.
    fn revalidate(
        &self,
        path: &Path,
        rule_id: &'static str,
        direct: bool,
    ) -> Result<Metadata, CleanError> {
        let roots = &self.roots;
        let io_err = |source| CleanError::Io {
            path: path.into(),
            source,
        };
        let meta = match self.fs.symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CleanError::Vanished(path.into()));
            }
            r => r.map_err(io_err)?,
        };
        if meta.file_type().is_symlink() {
            return Err(CleanError::Symlink(path.into()));
        }

        let protected = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&roots.fs_root))
            .filter(|a| match_path(a, roots).is_some_and(|r| r.status == Status::DoNotTouch))
            .last(); // shallowest = the protected area itself, for the error message
        if let Some(ancestor) = protected {
            return Err(CleanError::ProtectedAncestor {
                path: path.into(),
                ancestor: ancestor.into(),
            });
        }

        let still_ok = match_path(path, roots).is_some_and(|r| {
            r.id == rule_id
                && r.status != Status::DoNotTouch
                && (!direct || r.status == Status::Safe)
        });
        if !still_ok {
            return Err(CleanError::RuleMismatch {
                path: path.into(),
                rule_id,
            });
        }

        // No symlinked component between the anchor and the item: the resolved path must be the
        // resolved anchor plus the same relative path. Catches symlinked ancestors escaping the root.
        let anchor = [&roots.home, &roots.fs_root]
            .into_iter()
            .find(|a| path.starts_with(a))
            .ok_or_else(|| CleanError::Escapes(path.into()))?;
        let rel = path.strip_prefix(anchor).expect("starts_with checked");
        let expected = self.fs.canonicalize(anchor).map_err(io_err)?.join(rel);
        if self.fs.canonicalize(path).map_err(io_err)? != expected {
            return Err(CleanError::Escapes(path.into()));
        }
        Ok(meta)
    }

    /// Runs the exact [`command_for`] the item's target. Irreversible: the UI must warn first.
    pub fn run_command(&self, item: &Deletable) -> Result<Outcome, CleanError> {
        let Target::Command(target) = &item.info().target else {
            return Err(CleanError::NotACommand);
        };
        let cmd = command_for(target);
        let failed = |stderr: String| CleanError::CommandFailed {
            command: cmd.to_string(),
            stderr,
        };
        let out = self.runner.run(&cmd).map_err(|e| failed(e.to_string()))?;
        if !out.success {
            tracing::warn!(command = %cmd, stderr = %out.stderr.trim(), "command failed");
            return Err(failed(out.stderr.trim().into()));
        }
        tracing::info!(command = %cmd, "command ran");
        let estimated = item.info().size;
        let docker = cmd.program == "docker";
        let reclaimed = if docker {
            docker_reclaimed(&out.stdout)
        } else {
            // ponytail: tmutil/simctl print no size; trust the listing's estimate once the command succeeded.
            estimated
        };
        Ok(Outcome {
            estimated,
            reclaimed,
            trashed: false,
            docker_disk_note: docker,
        })
    }
}

/// "Total reclaimed space: 1.2GB" (image/container/volume prune) or "Total:\t1.2GB" (builder prune).
fn docker_reclaimed(stdout: &str) -> u64 {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| matches!(k.trim(), "Total reclaimed space" | "Total"))
        .and_then(|(_, v)| parse_docker_size(v.trim()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn docker_reclaimed_formats() {
        assert_eq!(
            super::docker_reclaimed("x\nTotal reclaimed space: 1.5GB\n"),
            1_500_000_000
        );
        assert_eq!(
            super::docker_reclaimed("ID\tRECLAIMABLE\nTotal:\t12.5MB\n"),
            12_500_000
        );
        assert_eq!(super::docker_reclaimed("Total reclaimed space: 0B"), 0);
        assert_eq!(super::docker_reclaimed("nothing"), 0);
    }
}
