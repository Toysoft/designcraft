//! Crash recovery (InDesign's automatic recovery data): unsaved changes of every open document are
//! written to a recovery folder (`file.recovery.save`, called on a timer by the app); saving or
//! closing a document removes its file; after a crash `file.recovery.open` reopens what was left
//! there as unsaved documents that remember where they were saved.
//!
//! A recovery entry is `<uid>.designcraft` (the document) plus `<uid>.json` (`{"path", "title",
//! "saved"}`: the original file, the title, and when it was written).
//!
//! Document uids start at 1 in every process, so each running app keeps its entries in a folder
//! of its own in the recovery folder (`run_dir`) and holds the lock in it for as long as it runs.
//! An app that starts moves into its own folder what apps that have ended left in theirs, and
//! leaves the folder of one still running alone.
//!
//! Each file is replaced atomically, but the pair is not a transaction. The document is published
//! and made durable before its metadata so a crash can leave metadata missing or stale, never a
//! listed metadata record without a complete primary document. Listing and opening recovery data
//! therefore treat metadata as optional.

#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{DocState, EngineError, Result, Session};

/// The platform's per-user recovery folder.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/DesignCraft/Recovery"));
    }
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("DesignCraft").join("Recovery"));
    }
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join("designcraft/recovery"))
}

/// This process's folder in a recovery folder, with the lock that says it is still running.
#[cfg(not(target_arch = "wasm32"))]
struct Run {
    root: PathBuf,
    dir: PathBuf,
    lock: std::fs::File,
    #[cfg(unix)]
    directory: std::fs::File,
}

#[cfg(not(target_arch = "wasm32"))]
static RUNS: std::sync::Mutex<Vec<Run>> = std::sync::Mutex::new(Vec::new());

/// The lock in a run's folder.
#[cfg(not(target_arch = "wasm32"))]
const LOCK: &str = "lock";

/// This run's own folder in the recovery folder `root`, made on the first call. Document uids
/// start at 1 in every process, so two running apps writing to one folder wrote over each
/// other's entries. The folder's lock is held until the process ends: an app that starts leaves
/// the folder of one that is still running alone.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_dir(root: &Path) -> Result<PathBuf> {
    let mut runs = RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(run) = runs.iter().find(|r| r.root == root) {
        return Ok(run.dir.clone());
    }
    secure_recovery_dir(root)?;
    for n in 0..64 {
        let dir = root.join(format!("run-{}-{n}", std::process::id()));
        // Only a folder this run makes is its own. One that is there is another app's, or was
        // left by a run that ended under this process id.
        #[cfg(unix)]
        let made = {
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700).create(&dir)
        };
        #[cfg(not(unix))]
        let made = std::fs::create_dir(&dir);
        match made {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(EngineError::Other(format!("{}: {e}", dir.display()))),
        }
        let locked = validate_recovery_dir(&dir).and_then(|()| {
            let lock = new_lock(&dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
            match lock.try_lock() {
                Ok(()) => {
                    validate_lock(&dir, &lock)?;
                    Ok(Some(lock))
                }
                Err(std::fs::TryLockError::WouldBlock) => Ok(None),
                Err(std::fs::TryLockError::Error(e)) => Err(EngineError::Other(format!("{}: {e}", dir.display()))),
            }
        });
        match locked {
            // An app that is starting can take a folder this new for one a run left, and remove
            // it before the lock is held: then the lock is gone, or that app holds it.
            Ok(Some(lock)) => {
                #[cfg(unix)]
                let directory = std::fs::File::open(&dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
                runs.push(Run {
                    root: root.to_path_buf(),
                    dir: dir.clone(),
                    lock,
                    #[cfg(unix)]
                    directory,
                });
                return Ok(dir);
            }
            Ok(_) => {}
            Err(e) => {
                // Never unlink a lock that another starting run may now own.
                let _ = std::fs::remove_dir(&dir);
                return Err(e);
            }
        }
    }
    Err(EngineError::Other(format!("{}: no free folder for this run", root.display())))
}

#[cfg(not(target_arch = "wasm32"))]
fn new_lock(dir: &Path) -> std::io::Result<std::fs::File> {
    let mut new = std::fs::OpenOptions::new();
    new.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        new.mode(0o600);
    }
    let mut file = new.open(dir.join(LOCK))?;
    // A marker distinguishes the held handle from a replacement at the same pathname on
    // platforms without portable file-identity metadata. Empty older-version locks remain valid.
    static NEXT_LOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_nanos();
    let marker = format!("{}:{}:{now}", std::process::id(), NEXT_LOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    file.write_all(marker.as_bytes())?;
    file.sync_all()?;
    Ok(file)
}

/// Check the current lock name without accepting a symlink or a lock held through an unlinked
/// handle. An advisory lock on an old handle does not protect a replacement at the same name.
#[cfg(not(target_arch = "wasm32"))]
fn validate_lock(dir: &Path, lock: &std::fs::File) -> Result<()> {
    let path = dir.join(LOCK);
    let fail = || EngineError::Other(format!("{}: this run no longer owns its recovery lock", path.display()));
    let before = std::fs::symlink_metadata(&path).map_err(|_| fail())?;
    if is_link_like(&before) || !before.file_type().is_file() {
        return Err(fail());
    }
    let named = std::fs::File::open(&path).map_err(|_| fail())?;
    let held = lock.metadata().map_err(|_| fail())?;
    let current = named.metadata().map_err(|_| fail())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if held.dev() != before.dev() || held.ino() != before.ino() || held.dev() != current.dev() || held.ino() != current.ino() {
            return Err(fail());
        }
    }
    #[cfg(not(unix))]
    let _ = (held, current);
    if lock_marker(lock).map_err(|_| fail())? != lock_marker(&named).map_err(|_| fail())? {
        return Err(fail());
    }
    let after = std::fs::symlink_metadata(&path).map_err(|_| fail())?;
    if is_link_like(&after) || !after.file_type().is_file() {
        return Err(fail());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if held.dev() != after.dev() || held.ino() != after.ino() {
            return Err(fail());
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn lock_marker(file: &std::fs::File) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek};
    let mut file = file.try_clone()?;
    file.seek(std::io::SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(129).read_to_end(&mut bytes)?;
    if bytes.len() > 128 {
        return Err(std::io::Error::other("invalid recovery lock marker"));
    }
    Ok(bytes)
}

/// A run's folder that was removed while it runs is made again by the next save: so is its lock.
#[cfg(not(target_arch = "wasm32"))]
fn relock(dir: &Path, created: bool) -> Result<()> {
    let mut runs = RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(run) = runs.iter_mut().find(|r| r.dir == dir) else { return Ok(()) };
    validate_recovery_dir(&run.root)?;
    #[cfg(unix)]
    let directory = std::fs::symlink_metadata(dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    // Reclaim a newly recreated folder only when this operation actually created it. Existing
    // replacements, including ones without a lock, may contain somebody else's documents.
    let same_directory = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = run.directory.metadata().map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
            held.dev() == directory.dev() && held.ino() == directory.ino()
        }
        #[cfg(not(unix))]
        {
            // Without stable directory identity, a valid held lock is the proof of ownership.
            // A missing lock cannot authorize an existing folder, even on coarse-timestamp disks.
            validate_lock(dir, &run.lock).is_ok()
        }
    };
    if !created && !same_directory {
        return Err(EngineError::Other(format!("{}: this run no longer owns its recovery folder", dir.display())));
    }
    match std::fs::symlink_metadata(dir.join(LOCK)) {
        Ok(_) => return validate_lock(dir, &run.lock),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(EngineError::Other(format!("{}: {e}", dir.display()))),
    }
    let lock = new_lock(dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    lock.try_lock().map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    validate_lock(dir, &lock)?;
    #[cfg(unix)]
    let replacement_directory = std::fs::File::open(dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held = replacement_directory.metadata().map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
        if held.dev() != directory.dev() || held.ino() != directory.ino() {
            return Err(EngineError::Other(format!("{}: recovery folder changed while restoring its lock", dir.display())));
        }
    }
    validate_lock(dir, &lock)?;
    run.lock = lock;
    #[cfg(unix)]
    {
        run.directory = replacement_directory;
    }
    Ok(())
}

/// Move into `own`, this run's folder, what is left to recover beside it: the entries of runs
/// that have ended, and those written straight into the recovery folder before runs had folders.
#[cfg(not(target_arch = "wasm32"))]
fn take_over(own: &Path) {
    let root = {
        let runs = RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(run) = runs.iter().find(|r| r.dir == own) else { return };
        run.root.clone()
    };
    move_entries(&root, own);
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    for e in rd.flatten() {
        let dir = e.path();
        if dir == own || !e.file_name().to_string_lossy().starts_with("run-") || validate_recovery_dir(&dir).is_err() {
            continue;
        }
        // Opened for writing: over NFS an exclusive lock on a file opened for reading is refused.
        let Ok(metadata) = std::fs::symlink_metadata(dir.join(LOCK)) else { continue };
        if is_link_like(&metadata) || !metadata.file_type().is_file() {
            continue;
        }
        let Ok(lock) = std::fs::OpenOptions::new().read(true).write(true).open(dir.join(LOCK)) else { continue };
        match lock.try_lock() {
            // Free: the run has ended. Holding it keeps another starting app out.
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!("recovery data in {} is left there: {e}", dir.display());
                continue;
            }
        }
        if let Err(e) = validate_lock(&dir, &lock) {
            log::warn!("recovery data in {} is left there: {e}", dir.display());
            continue;
        }
        move_entries(&dir, own);
        remove_emptied(&dir);
    }
}

/// Remove the folder of a run that ended, with what the app left in it beside documents. A
/// document still in it, or a file the app did not write, keeps the folder as it is.
#[cfg(not(target_arch = "wasm32"))]
fn remove_emptied(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut left = Vec::new();
    for e in rd {
        let Ok(e) = e else { return };
        let path = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        let numbered = path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.parse::<u64>().is_ok());
        // Metadata whose document has gone, and a file a crash left half written.
        if (numbered && name.ends_with(".json")) || (name.starts_with(".designcraft-recovery-") && name.ends_with(".tmp")) {
            left.push(path);
        } else if name != LOCK {
            return;
        }
    }
    for path in left {
        let _ = std::fs::remove_file(path);
    }
    // The lock last: until then the folder is still that of a run that ended.
    let _ = std::fs::remove_file(dir.join(LOCK));
    let _ = std::fs::remove_dir(dir);
}

#[cfg(not(target_arch = "wasm32"))]
fn move_entries(from: &Path, own: &Path) {
    for (uid, _) in list(from) {
        let (doc, meta) = files(from, uid);
        // Under a uid no document of this run has or gets.
        let (own_doc, own_meta) = files(own, crate::NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        // The document first: its metadata is optional.
        if std::fs::rename(&doc, &own_doc).is_ok() {
            let _ = std::fs::rename(&meta, &own_meta);
        }
    }
}

fn files(dir: &Path, uid: u64) -> (PathBuf, PathBuf) {
    (dir.join(format!("{uid}.designcraft")), dir.join(format!("{uid}.json")))
}

/// Remove a document's recovery entry (it was saved or closed).
pub fn discard(dir: &Path, uid: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if validate_recovery_dir(dir).is_err() || relock(dir, false).is_err() {
        return;
    }
    let (d, m) = files(dir, uid);
    let _ = std::fs::remove_file(d);
    let _ = std::fs::remove_file(m);
}

/// Write recovery entries for the dirty documents (and drop those of clean ones). Returns how
/// many were written.
pub fn save(s: &Session, dir: &Path) -> Result<usize> {
    let created = secure_recovery_dir(dir)?;
    #[cfg(not(target_arch = "wasm32"))]
    relock(dir, created)?;
    #[cfg(target_arch = "wasm32")]
    let _ = created;
    let mut n = 0;
    for d in &s.docs {
        if !d.is_dirty() {
            discard(dir, d.uid);
            continue;
        }
        let (doc, meta) = files(dir, d.uid);
        let bytes = crate::cmd::to_bytes(d.preview_stash.as_deref().unwrap_or(d.doc.as_ref()));
        // Write then rename, so a crash mid-write never leaves a torn file.
        write_recovery_file(&doc, &bytes)?;
        let m = json!({"path": d.path, "title": d.title(), "saved": designcraft_doc::vars::now()});
        write_recovery_file(&meta, m.to_string().as_bytes())?;
        n += 1;
    }
    Ok(n)
}

#[cfg(not(target_arch = "wasm32"))]
fn secure_recovery_dir(dir: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => return validate_recovery_dir(dir).map(|()| false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(EngineError::Other(format!("{}: {e}", dir.display()))),
    }

    // Create parent directories privately without changing existing caller-owned permissions.
    // Create the leaf separately so a creation race can never be mistaken for our own folder.
    #[cfg(unix)]
    let create_result = {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
        }
        std::fs::DirBuilder::new().mode(0o700).create(dir)
    };
    #[cfg(not(unix))]
    let create_result = {
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
        }
        std::fs::create_dir(dir)
    };
    let created = match create_result {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(EngineError::Other(format!("{}: {e}", dir.display()))),
    };
    validate_recovery_dir(dir)?;
    Ok(created)
}

#[cfg(target_arch = "wasm32")]
fn secure_recovery_dir(_dir: &Path) -> Result<bool> {
    Err(EngineError::Other("recovery files are unavailable on the web".into()))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_recovery_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Err(EngineError::Other(format!("{} has no parent folder", path.display())));
    };
    validate_recovery_dir(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".designcraft-recovery-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file().set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    }
    tmp.as_file_mut().write_all(bytes).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    tmp.as_file_mut().sync_all().map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    // Validate the non-followed leaf again immediately before renaming the randomized temporary
    // file to its fixed sibling destination.
    validate_recovery_dir(dir)?;
    tmp.persist(path).map_err(|e| EngineError::Other(format!("{}: {}", path.display(), e.error)))?;
    sync_recovery_dir(dir)?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_recovery_dir(dir: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    if is_link_like(&metadata) {
        return Err(EngineError::Other(format!("{} must not be a symbolic link or reparse point", dir.display())));
    }
    if !metadata.file_type().is_dir() {
        return Err(EngineError::Other(format!("{} is not a directory", dir.display())));
    }
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), windows))]
fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    // This catches all reparse-point leaves, including junctions, rather than relying solely on
    // FileType::is_symlink().
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_recovery_dir(dir: &Path) -> Result<()> {
    validate_recovery_dir(dir)?;

    #[cfg(unix)]
    {
        let directory = std::fs::File::open(dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
        directory.sync_all().map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    }

    #[cfg(not(unix))]
    {
        // Rust does not expose a portable durable-directory handle. Try where the platform
        // supports opening directories, but do not turn a fully persisted file into a reported
        // failure where it does not.
        if let Ok(directory) = std::fs::File::open(dir) {
            let _ = directory.sync_all();
        }
    }

    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn write_recovery_file(_path: &Path, _bytes: &[u8]) -> Result<()> {
    Err(EngineError::Other("recovery files are unavailable on the web".into()))
}

/// Recovery entries in `dir`: (uid, metadata).
pub fn list(dir: &Path) -> Vec<(u64, Value)> {
    let mut out = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    if validate_recovery_dir(dir).is_err() || relock(dir, false).is_err() {
        return out;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("designcraft") {
            continue;
        }
        let Some(uid) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) else { continue };
        // Metadata is a file on disk: anything but an object (corrupt, hand-edited) counts as none.
        let meta = std::fs::read_to_string(p.with_extension("json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(Value::is_object)
            .unwrap_or(json!({}));
        out.push((uid, meta));
    }
    out.sort_by_key(|e| e.0);
    out
}

/// Reopen every recovery entry as an unsaved document (and remove the entries: the new session
/// writes its own). Returns the opened documents' indices.
pub fn open(s: &mut Session, dir: &Path) -> Result<Vec<usize>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        validate_recovery_dir(dir)?;
        relock(dir, false)?;
    }
    let mut opened = Vec::new();
    // For this run's own folder, first bring in what runs that ended left in theirs.
    #[cfg(not(target_arch = "wasm32"))]
    take_over(dir);
    for (uid, meta) in list(dir) {
        #[cfg(not(target_arch = "wasm32"))]
        validate_recovery_dir(dir)?;
        let (doc, _) = files(dir, uid);
        let Ok(bytes) = std::fs::read(&doc) else { continue };
        let mut d = match crate::cmd::from_bytes(&bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let path = meta.get("path").and_then(Value::as_str).map(str::to_string);
        // Skipped font files are logged; the document opens without them.
        let fonts = path.as_deref().and_then(|p| crate::cmd::load_document_fonts(&mut d, p).0);
        let mut st = DocState::new(d, path);
        st.fonts = fonts;
        // Unsaved: the copy on disk (if any) is older than what was recovered.
        st.saved_doc = std::sync::Arc::new((*st.doc).clone());
        st.revision += 1;
        opened.push(s.add_document(st));
        discard(dir, uid);
    }
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirty_session() -> Session {
        let mut session = Session::new();
        session.execute("file.new", &json!({})).unwrap();
        session.execute("frame.create", &json!({"rect": [10, 10, 100, 100]})).unwrap();
        session
    }

    #[test]
    fn recovers_unsaved_documents() {
        let dir = std::env::temp_dir().join(format!("dc-recovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        s.execute("file.new", &json!({})).unwrap();
        // Only the edited document is written.
        s.execute("frame.create", &json!({"rect": [10, 10, 100, 100]})).unwrap();
        assert_eq!(save(&s, &dir).unwrap(), 1);
        assert_eq!(list(&dir).len(), 1);
        // "Crash": a new session reopens it, unsaved, with the item.
        let mut s2 = Session::new();
        let opened = open(&mut s2, &dir).unwrap();
        assert_eq!(opened.len(), 1);
        let st = s2.doc().unwrap();
        assert!(st.is_dirty());
        assert_eq!(st.doc.spreads[0].items.len(), 1);
        assert!(list(&dir).is_empty(), "entries are consumed");
        // Saving a document drops its entry.
        s.recovery_dir = Some(dir.clone());
        assert_eq!(save(&s, &dir).unwrap(), 1);
        let path = dir.join("saved.designcraft");
        s.execute("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
        assert!(list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn recovery_data_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("recovery");
        let s = dirty_session();
        assert_eq!(save(&s, &dir).unwrap(), 1);

        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            let entry = entry.unwrap();
            assert_eq!(entry.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn existing_recovery_directory_is_used() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("recovery");
        std::fs::create_dir(&dir).unwrap();

        assert_eq!(save(&dirty_session(), &dir).unwrap(), 1);
        assert_eq!(list(&dir).len(), 1);
        assert!(std::fs::symlink_metadata(&dir).unwrap().file_type().is_dir());
    }

    #[test]
    fn non_directory_recovery_path_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recovery");
        std::fs::write(&path, b"not a directory").unwrap();

        assert!(save(&dirty_session(), &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a directory");
    }

    #[cfg(unix)]
    #[test]
    fn existing_recovery_directory_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("recovery");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();

        assert_eq!(save(&dirty_session(), &dir).unwrap(), 1);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o750);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_recovery_directory_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let link = root.path().join("recovery");
        std::fs::create_dir(&destination).unwrap();
        symlink(&destination, &link).unwrap();

        assert!(save(&dirty_session(), &link).is_err());
        assert!(std::fs::read_dir(&destination).unwrap().next().is_none());
        std::fs::write(destination.join("7.designcraft"), b"unchanged").unwrap();
        std::fs::write(destination.join("7.json"), b"{}").unwrap();
        assert!(list(&link).is_empty());
        discard(&link, 7);
        assert_eq!(std::fs::read(destination.join("7.designcraft")).unwrap(), b"unchanged");
        assert!(open(&mut Session::new(), &link).is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_run_has_a_locked_folder_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("recovery");
        let own = run_dir(&dir).unwrap();
        assert_eq!(own.parent(), Some(dir.as_path()));
        assert_eq!(run_dir(&dir).unwrap(), own, "one for the whole run");
        let lock = std::fs::File::open(own.join(LOCK)).unwrap();
        assert!(matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)), "held while the run lives");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&own).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(own.join(LOCK)).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    /// The recovery folder was cleared while the app runs: the next save makes the run's folder
    /// again, and its lock with it.
    #[cfg(unix)]
    #[test]
    fn a_removed_run_folder_is_made_again_with_its_lock() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        std::fs::remove_dir_all(&own).unwrap();
        assert_eq!(save(&dirty_session(), &own).unwrap(), 1);
        let lock = std::fs::File::open(own.join(LOCK)).unwrap();
        assert!(matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    }

    /// Replacing the name of a live run's lock must not authorize writes under the old handle.
    #[cfg(unix)]
    #[test]
    fn save_rejects_a_run_whose_lock_was_replaced() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        std::fs::remove_file(own.join(LOCK)).unwrap();
        let replacement = new_lock(&own).unwrap();
        replacement.try_lock().unwrap();
        assert!(save(&dirty_session(), &own).is_err());
        assert!(list(&own).is_empty(), "another owner's folder must not receive our recovery data");
    }

    /// A blocked attempt to recreate a run's lock must stop the save before it writes documents.
    #[cfg(unix)]
    #[test]
    fn save_rejects_a_run_whose_lock_cannot_be_recreated() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        std::fs::remove_file(own.join(LOCK)).unwrap();
        std::fs::create_dir(own.join(LOCK)).unwrap();
        assert!(save(&dirty_session(), &own).is_err());
        assert!(list(&own).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discarding_a_document_does_not_touch_a_replaced_runs_entries() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        std::fs::remove_file(own.join(LOCK)).unwrap();
        let replacement = new_lock(&own).unwrap();
        replacement.try_lock().unwrap();
        let (doc, meta) = files(&own, 1);
        std::fs::write(&doc, b"the replacement owner's document").unwrap();
        std::fs::write(&meta, b"{}").unwrap();
        discard(&own, 1);
        assert_eq!(std::fs::read(doc).unwrap(), b"the replacement owner's document");
        assert!(meta.exists());
    }

    #[cfg(unix)]
    #[test]
    fn saving_a_run_rejects_a_symlinked_lock() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        std::fs::remove_file(own.join(LOCK)).unwrap();
        let unrelated = root.path().join("unrelated");
        std::fs::write(&unrelated, b"unrelated file").unwrap();
        std::os::unix::fs::symlink(&unrelated, own.join(LOCK)).unwrap();
        assert!(save(&dirty_session(), &own).is_err());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"unrelated file");
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_run_directory_is_not_claimed_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let own = run_dir(root.path()).unwrap();
        let session = dirty_session();
        let uid = session.docs[0].uid;
        std::fs::remove_dir_all(&own).unwrap();
        std::fs::create_dir(&own).unwrap();
        let (doc, _) = files(&own, uid);
        std::fs::write(&doc, b"another owner's document").unwrap();
        assert!(save(&session, &own).is_err());
        assert_eq!(std::fs::read(doc).unwrap(), b"another owner's document");
        assert!(!own.join(LOCK).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_run_gets_no_folder_in_a_symlinked_recovery_folder() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let link = root.path().join("recovery");
        std::fs::create_dir(&destination).unwrap();
        symlink(&destination, &link).unwrap();

        assert!(run_dir(&link).is_err());
        assert!(std::fs::read_dir(&destination).unwrap().next().is_none());
    }

    fn unsaved(text: &str) -> Session {
        let mut session = Session::new();
        session.execute("file.new", &json!({})).unwrap();
        session.execute("frame.create", &json!({"rect": [10, 10, 100, 100], "content": "text", "text": text})).unwrap();
        session
    }

    fn texts(s: &Session) -> Vec<String> {
        let mut texts: Vec<String> = s.documents().iter().flat_map(|d| d.doc.stories.values().map(|st| st.text.clone())).collect();
        texts.sort();
        texts
    }

    /// What a run that has ended leaves: its folder with its lock, free now, and the entry of
    /// one unsaved document, numbered `uid` as that process numbered it.
    fn run_left(root: &Path, name: &str, text: &str, uid: u64) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOCK), b"").unwrap();
        let mut s = unsaved(text);
        s.docs[0].uid = uid;
        assert_eq!(save(&s, &dir).unwrap(), 1);
        dir
    }

    /// Every process numbers its first document 1: two running apps both wrote `1.designcraft`
    /// into the one recovery folder, and only the document written last was recovered.
    #[test]
    fn documents_of_two_apps_that_ended_are_both_recovered() {
        let root = tempfile::tempdir().unwrap();
        let first = run_left(root.path(), "run-7-0", "first app", 1);
        let second = run_left(root.path(), "run-8-0", "second app", 1);
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 2);
        assert_eq!(texts(&s), ["first app", "second app"]);
        assert!(s.documents().iter().all(DocState::is_dirty));
        assert!(!first.exists() && !second.exists(), "nothing is left of the runs that ended");
    }

    /// An app that started reopened the unsaved documents of one still running, and removed
    /// its entries.
    #[test]
    fn a_running_apps_recovery_data_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let running = run_left(root.path(), "run-7-0", "still open", 1);
        let lock = std::fs::File::open(running.join(LOCK)).unwrap();
        lock.try_lock().unwrap();
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert!(open(&mut s, &own).unwrap().is_empty());
        assert_eq!(list(&running).len(), 1);
        // Once it has ended, its document is recovered.
        drop(lock);
        assert_eq!(open(&mut s, &own).unwrap().len(), 1);
        assert_eq!(texts(&s), ["still open"]);
        assert!(!running.exists());
    }

    /// A process id comes round again: the folder a run left under it is not this run's.
    #[test]
    fn a_folder_left_under_this_process_id_is_not_reused() {
        let root = tempfile::tempdir().unwrap();
        let left = run_left(root.path(), &format!("run-{}-0", std::process::id()), "left behind", 1);
        let own = run_dir(root.path()).unwrap();
        assert_ne!(own, left);
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 1);
        assert_eq!(texts(&s), ["left behind"]);
    }

    /// Entries written straight into the recovery folder, as before runs had folders.
    #[test]
    fn recovery_data_of_an_earlier_version_is_recovered() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(save(&unsaved("earlier version"), root.path()).unwrap(), 1);
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 1);
        assert_eq!(texts(&s), ["earlier version"]);
        assert!(list(root.path()).is_empty());
    }

    /// An entry that can't be opened stays on disk, in this run's folder now.
    #[test]
    fn an_unreadable_entry_of_a_run_that_ended_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let left = run_left(root.path(), "run-7-0", "readable", 1);
        std::fs::write(left.join("2.designcraft"), b"not a document").unwrap();
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 1);
        assert!(list(&own).iter().any(|e| std::fs::read(files(&own, e.0).0).unwrap() == b"not a document"));
        assert!(!left.exists());
    }

    /// Only a folder named as a run's, with a lock that is free, is a run that ended.
    #[test]
    fn what_is_no_run_that_ended_is_passed_over() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("run-1-0"), b"a file").unwrap();
        // An app that is starting has made its folder and not yet its lock.
        let starting = root.path().join("run-2-0");
        std::fs::create_dir(&starting).unwrap();
        let other = run_left(root.path(), "backup", "not a run", 1);
        #[cfg(unix)]
        let (_elsewhere, linked) = {
            let elsewhere = tempfile::tempdir().unwrap();
            let linked = run_left(elsewhere.path(), "run-3-0", "behind a link", 1);
            std::os::unix::fs::symlink(&linked, root.path().join("run-3-0")).unwrap();
            (elsewhere, linked)
        };
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert!(open(&mut s, &own).unwrap().is_empty());
        assert!(starting.exists());
        assert_eq!(list(&other).len(), 1);
        #[cfg(unix)]
        assert!(list(&linked).len() == 1 && root.path().join("run-3-0").is_symlink());
    }

    /// The metadata goes with its document: a document taken over remembers its file.
    #[test]
    fn a_document_taken_over_remembers_its_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("layout.designcraft");
        let recovery = root.path().join("recovery");
        let left = recovery.join("run-7-0");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join(LOCK), b"").unwrap();
        let mut a = unsaved("saved once");
        a.execute("file.saveAs", &json!({"path": file.to_string_lossy()})).unwrap();
        a.execute("frame.create", &json!({"rect": [10, 10, 100, 100]})).unwrap();
        assert_eq!(save(&a, &left).unwrap(), 1);
        let own = run_dir(&recovery).unwrap();
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 1);
        assert_eq!(s.doc().unwrap().path.as_deref(), Some(file.to_string_lossy().as_ref()));
    }

    /// The folder of a run that ended goes with what the app left in it. A file the app did not
    /// write keeps the folder, and is not deleted.
    #[test]
    fn only_what_the_app_left_goes_with_the_folder_of_a_run_that_ended() {
        let root = tempfile::tempdir().unwrap();
        let emptied = run_left(root.path(), "run-7-0", "moved", 1);
        std::fs::write(emptied.join("9.json"), b"{}").unwrap();
        std::fs::write(emptied.join(".designcraft-recovery-abc.tmp"), b"half written").unwrap();
        let kept = run_left(root.path(), "run-8-0", "moved too", 1);
        std::fs::write(kept.join("copy.designcraft"), b"a copy someone put here").unwrap();
        let own = run_dir(root.path()).unwrap();
        let mut s = Session::new();
        assert_eq!(open(&mut s, &own).unwrap().len(), 2);
        assert!(!emptied.exists());
        assert_eq!(std::fs::read(kept.join("copy.designcraft")).unwrap(), b"a copy someone put here");
        assert!(kept.join(LOCK).exists(), "still the folder of a run that ended");
    }

    /// A folder that is there is another app's or was left behind, lock or no lock: a run's own
    /// is one it made itself.
    #[test]
    fn a_folder_that_is_there_is_not_made_this_runs() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join(format!("run-{}-0", std::process::id()));
        std::fs::create_dir(&left).unwrap();
        std::fs::write(left.join("1.designcraft"), b"left with no lock").unwrap();
        let own = run_dir(root.path()).unwrap();
        assert_ne!(own, left);
        assert!(!left.join(LOCK).exists());
        assert_eq!(std::fs::read(left.join("1.designcraft")).unwrap(), b"left with no lock");
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    /// `file.recovery.list` adds `uid` to each entry's metadata: metadata that wasn't an object
    /// (a corrupt `.json` beside the recovery file) panicked on `m["uid"] = …`.
    #[test]
    fn corrupt_recovery_metadata_is_ignored() {
        let dir = std::env::temp_dir().join(format!("dc-recovery-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("7.designcraft"), b"not a document").unwrap();
        std::fs::write(dir.join("7.json"), b"[1, 2]").unwrap();
        let mut s = Session::new();
        s.recovery_dir = Some(dir.clone());
        let r = s.execute("file.recovery.list", &json!({})).unwrap();
        assert_eq!(r[0]["uid"], json!(7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_and_save_a_copy() {
        let dir = std::env::temp_dir().join(format!("dc-revert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, copy) = (dir.join("a.designcraft"), dir.join("copy.designcraft"));
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        assert!(s.execute("file.revert", &json!({})).is_err(), "never saved");
        s.execute("file.saveAs", &json!({"path": a.to_string_lossy()})).unwrap();
        s.execute("frame.create", &json!({"rect": [10, 10, 100, 100]})).unwrap();
        // A copy keeps the document unsaved and on its own file.
        s.execute("file.saveACopy", &json!({"path": copy.to_string_lossy()})).unwrap();
        assert!(copy.exists());
        assert!(s.doc().unwrap().is_dirty());
        assert_eq!(s.doc().unwrap().path.as_deref(), Some(a.to_string_lossy().as_ref()));
        // Revert drops the frame.
        s.execute("file.revert", &json!({})).unwrap();
        assert!(s.doc().unwrap().doc.spreads[0].items.is_empty());
        assert!(!s.doc().unwrap().is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
