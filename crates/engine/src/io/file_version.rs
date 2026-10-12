//! Content identity for a session's native file. No polling or typing-path filesystem work.
use super::FileConflict;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_BYTES: u64 = 2 << 30;

#[derive(Clone, Debug)]
pub(crate) struct FileVersion {
    path: PathBuf,
    resolved: PathBuf,
    handle: Option<Arc<same_file::Handle>>,
    digest: [u8; 32],
}

/// Keep the logical spelling: a retargeted parent symlink still names the session's file.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve aliases even when an externally deleted final entry no longer canonicalizes.
fn resolved_spelling(path: &Path) -> Option<PathBuf> {
    match (path.parent().and_then(|p| fs::canonicalize(p).ok()), path.file_name()) {
        (Some(parent), Some(name)) => Some(parent.join(name)),
        _ => None,
    }
}

fn regular(path: &Path) -> std::io::Result<File> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(std::io::Error::other("expected a regular file no larger than 2 GB"));
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(std::io::Error::other("expected a regular file no larger than 2 GB"));
    }
    Ok(file)
}

impl FileVersion {
    pub(super) fn read(path: &Path) -> Result<(Vec<u8>, Self), String> {
        let read = || -> std::io::Result<_> {
            let mut file = regular(path)?;
            let mut bytes = Vec::new();
            (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(std::io::Error::other("file grew beyond 2 GB"));
            }
            let handle = same_file::Handle::from_file(file)?;
            let version = Self {
                path: absolute(path),
                resolved: fs::canonicalize(path)?,
                handle: Some(Arc::new(handle)),
                digest: Sha256::digest(&bytes).into(),
            };
            Ok((bytes, version))
        };
        read().map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Path spellings, symlinks and hard links to the open file share its expected content.
    pub(super) fn owns(&self, path: &Path) -> Result<bool, FileConflict> {
        let absolute = absolute(path);
        if absolute == self.path || absolute == self.resolved {
            return Ok(true);
        }
        if fs::canonicalize(path).ok().or_else(|| resolved_spelling(path)).is_some_and(|p| p == self.resolved) {
            return Ok(true);
        }
        match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Ok(meta) if meta.is_file() => {}
            _ => return Err(FileConflict { path: path.to_path_buf() }),
        }
        let Some(original) = &self.handle else { return Err(FileConflict { path: path.to_path_buf() }) };
        // same-file's Windows comparison includes size: refresh the retained file handle's
        // metadata so a changed-size hard link cannot bypass ownership. An unreadable alias
        // cannot safely be treated as an unrelated Save As destination.
        original
            .as_file()
            .try_clone()
            .and_then(same_file::Handle::from_file)
            .and_then(|original| same_file::Handle::from_path(path).map(|current| original == current))
            .map_err(|_| FileConflict { path: path.to_path_buf() })
    }

    pub(super) fn verify(&self, path: &Path) -> Result<(), FileConflict> {
        let check = || -> std::io::Result<bool> {
            let file = regular(path)?;
            let mut reader = BufReader::new(file).take(MAX_BYTES + 1);
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            let mut total = 0u64;
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > MAX_BYTES {
                    return Ok(false);
                }
                // Read's successful byte count is bounded by the supplied buffer.
                digest.update(&buffer[..n]);
            }
            Ok(<[u8; 32]>::from(digest.finalize()) == self.digest)
        };
        if check().unwrap_or(false) { Ok(()) } else { Err(FileConflict { path: path.to_path_buf() }) }
    }

    /// Keep the identity of serialized bytes, never a second read that might capture an
    /// external writer's version. Handle lookup is best effort after a committed replacement.
    pub(super) fn saved(path: &Path, bytes: &[u8]) -> Self {
        Self {
            path: absolute(path),
            resolved: fs::canonicalize(path).unwrap_or_else(|_| absolute(path)),
            handle: same_file::Handle::from_path(path).ok().map(Arc::new),
            digest: Sha256::digest(bytes).into(),
        }
    }
}
