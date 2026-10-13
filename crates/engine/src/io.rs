//! Reading and writing the files a command names by path.
//!
//! Every such read and write goes through this module, so the automation roots
//! ([`crate::file_access`]) are checked in one place. A command module that called `std::fs`
//! itself would bypass them: a test in `file_access` keeps that from happening.
//!
//! A path the roots refuse reads as a file that isn't there where the caller only asks whether
//! it is ([`is_file`], [`is_dir`], [`exists`], [`metadata`]), and as the roots' own message where
//! it asks for the bytes.

use std::path::Path;

/// What [`metadata`] returns, for callers that may not name `std::fs` themselves.
pub use std::fs::Metadata;

/// The bytes of the file at `path`, inside the automation read root when there is one.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>, String> {
    let path = path.as_ref();
    crate::file_access::check_read(path)?;
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// [`read_file`] with the file system's own error, for callers that tell "missing" from other
/// failures. A refused path is a `PermissionDenied` error carrying the roots' message.
pub fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    std::fs::read(path)
}

/// The text of the file at `path`, inside the automation read root when there is one.
pub fn read_to_string(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    crate::file_access::check_read(path)?;
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
pub fn write_file(path: impl AsRef<Path>, bytes: &[u8]) -> Result<(), String> {
    let path = path.as_ref();
    crate::file_access::check_write(path)?;
    // Replaced atomically where there is a real file system (see `atomic`).
    #[cfg(not(target_arch = "wasm32"))]
    let written = crate::atomic::write_atomic(path, bytes);
    #[cfg(target_arch = "wasm32")]
    let written = std::fs::write(path, bytes);
    written.map_err(|e| format!("{}: {e}", path.display()))
}

/// Atomically replace the file at `path` with the contents of `staged`, a private temporary file
/// the engine made itself (so it is not checked), inside the automation write root when there is
/// one.
#[cfg(not(target_arch = "wasm32"))]
pub fn replace_with(path: impl AsRef<Path>, staged: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    let mut input = std::fs::File::open(staged)?;
    crate::atomic::write_atomic_with(path, |file| std::io::copy(&mut input, file).map(|_| ()))
}

/// Could the file at `path` be opened for writing (without truncating it)? Inside the automation
/// write root when there is one.
pub fn writable(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    std::fs::OpenOptions::new().write(true).open(path).map(drop)
}

/// The canonical form of `path` (links resolved), inside the automation read root when there is
/// one.
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<std::path::PathBuf> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    std::fs::canonicalize(path)
}

/// What the file system says about `path` (links followed), inside the automation read root when
/// there is one. A refused path is a `PermissionDenied` error carrying the roots' message.
pub fn metadata(path: impl AsRef<Path>) -> std::io::Result<Metadata> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    std::fs::metadata(path)
}

/// [`metadata`] of a regular file: anything else at `path` (a FIFO, a device, a folder) is a
/// `NotFound` error. Only the path is looked at; nothing is opened.
pub fn file_metadata(path: impl AsRef<Path>) -> std::io::Result<Metadata> {
    let path = path.as_ref();
    let m = metadata(path)?;
    if m.is_file() { Ok(m) } else { Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}: not a file", path.display()))) }
}

/// The bytes of the regular file at `path`, no more than `limit` of them, with the metadata of the
/// file that was opened, inside the automation read root when there is one.
///
/// The file is opened first and the opened file is checked (so it can't be swapped between the
/// check and the read), and on Unix it is opened without waiting for a writer (`O_NONBLOCK`), so
/// a FIFO doesn't block. A FIFO, a device or a folder is a `NotFound` error and a file larger than
/// `limit` a `FileTooLarge` one, before anything is read.
pub fn read_regular(path: impl AsRef<Path>, limit: u64) -> std::io::Result<(Vec<u8>, Metadata)> {
    use std::io::{Error, ErrorKind, Read};
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(|e| Error::new(ErrorKind::PermissionDenied, e))?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let mut file = options.open(path)?;
    let m = file.metadata()?;
    if !m.is_file() {
        return Err(Error::new(ErrorKind::NotFound, "not a file"));
    }
    let too_large = || Error::new(ErrorKind::FileTooLarge, format!("larger than {} MiB", limit / (1024 * 1024)));
    if m.len() > limit {
        return Err(too_large());
    }
    // One byte over the limit at most, in case the file grew since it was opened.
    let mut bytes = Vec::new();
    (&mut file).take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok((bytes, m))
}

/// Is there a file at `path` that automation may read?
pub fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_file())
}

/// Is there a folder at `path` that automation may read?
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_dir())
}

/// Is there anything at `path` that automation may read?
pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

/// The entries of the folder at `path`, inside the automation read root when there is one.
pub fn read_dir(path: impl AsRef<Path>) -> std::io::Result<std::fs::ReadDir> {
    let path = path.as_ref();
    crate::file_access::check_read(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    std::fs::read_dir(path)
}
