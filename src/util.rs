use anyhow::{Context, Result};
use memmap2::Mmap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub relative_path: String,
    pub kind: EntryKind,
    pub full_path: PathBuf,
    /// File size in bytes (0 for directories). Free from the OS directory scan.
    pub size: u64,
}

/// Walk a directory tree and collect all entries with relative paths.
/// Paths use forward slashes for cross-platform consistency in the patch format.
pub fn walk_directory(root: &Path) -> Result<Vec<DirEntry>> {
    let root = root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", root.display()))?;

    let mut entries = Vec::new();

    for entry in WalkDir::new(&root).min_depth(1) {
        let entry = entry.with_context(|| format!("Failed to read directory entry in {}", root.display()))?;

        let full_path = entry.path().to_path_buf();
        let relative = full_path
            .strip_prefix(&root)
            .with_context(|| "Failed to compute relative path")?;

        let relative_str = relative
            .to_str()
            .with_context(|| format!("Non-UTF8 path: {}", relative.display()))?
            .replace('\\', "/");

        let kind = if entry.file_type().is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };

        let meta = entry
            .metadata()
            .with_context(|| format!("Failed to read metadata: {}", full_path.display()))?;
        let size = if kind == EntryKind::File { meta.len() } else { 0 };

        entries.push(DirEntry {
            relative_path: relative_str,
            kind,
            full_path,
            size,
        });
    }

    Ok(entries)
}

/// Memory-map a file for read-only access.
///
/// # Safety
/// The mapping is read-only. Callers must not concurrently truncate or replace
/// the underlying file while the `Mmap` is live.
pub fn mmap_file(path: &Path) -> Result<Mmap> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;
    // SAFETY: We only read from this mapping; no concurrent modification of these files.
    unsafe {
        Mmap::map(&file)
            .with_context(|| format!("Failed to memory-map file: {}", path.display()))
    }
}


/// Compute the BLAKE3 hash of a byte slice.
pub fn hash_bytes(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Collect just the relative paths as a set for fast lookup.
pub fn path_set(entries: &[DirEntry]) -> BTreeSet<String> {
    entries.iter().map(|e| e.relative_path.clone()).collect()
}

/// Sort directory paths so parents come before children.
pub fn sort_dirs_parent_first(dirs: &mut [String]) {
    dirs.sort();
}

/// Sort directory paths so children come before parents (for deletion).
pub fn sort_dirs_deepest_first(dirs: &mut [String]) {
    dirs.sort();
    dirs.reverse();
}
