use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::path::Path;

use crate::binary_patch;
use crate::patch_format::{ApplySummary, PatchManifest, PatchOp, MAGIC};
use crate::util;

/// Apply a patch file to the target directory.
/// Uses Rayon for parallel file operations where safe.
pub async fn apply_patch(target_dir: &Path, patch_path: &Path) -> Result<ApplySummary> {
    // mmap the patch file, check magic, then stream-decompress into bincode
    // (avoids allocating a full decompressed Vec)
    let raw = util::mmap_file(patch_path)?;

    if raw.len() < MAGIC.len() || &raw[..MAGIC.len()] != MAGIC {
        bail!("Invalid patch file: missing magic header");
    }

    let decoder =
        zstd::Decoder::new(&raw[MAGIC.len()..]).context("Failed to create zstd decoder")?;
    let manifest: PatchManifest =
        bincode::deserialize_from(decoder).context("Failed to deserialize patch manifest")?;

    if manifest.version != crate::patch_format::FORMAT_VERSION {
        bail!(
            "Unsupported patch version: {} (expected {})",
            manifest.version,
            crate::patch_format::FORMAT_VERSION
        );
    }

    // Group operations by type (owned, not borrowed)
    let mut create_dirs: Vec<PatchOp> = Vec::new();
    let mut add_files: Vec<PatchOp> = Vec::new();
    let mut modify_files: Vec<PatchOp> = Vec::new();
    let mut delete_files: Vec<PatchOp> = Vec::new();
    let mut delete_dirs: Vec<PatchOp> = Vec::new();

    for op in manifest.operations {
        match &op {
            PatchOp::CreateDir { .. } => create_dirs.push(op),
            PatchOp::AddFile { .. } => add_files.push(op),
            PatchOp::ModifyFile { .. } => modify_files.push(op),
            PatchOp::DeleteFile { .. } => delete_files.push(op),
            PatchOp::DeleteDir { .. } => delete_dirs.push(op),
        }
    }

    let num_create_dirs = create_dirs.len();
    let num_add_files = add_files.len();
    let num_modify_files = modify_files.len();
    let num_delete_files = delete_files.len();
    let num_delete_dirs = delete_dirs.len();

    let target = target_dir
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize target: {}", target_dir.display()))?;

    // 1. Create directories (sequential, parent-first - already ordered)
    for op in &create_dirs {
        if let PatchOp::CreateDir { path } = op {
            let full = target.join(path);
            std::fs::create_dir_all(&full)
                .with_context(|| format!("Failed to create directory: {}", full.display()))?;
        }
    }

    // Pre-process deletions: if an entire directory subtree is being removed, use
    // remove_dir_all on the subtree root instead of thousands of individual deletions.
    // A directory is in delete_dirs only when it has no presence in new_dir, so every
    // file and subdir inside it is also being removed — bulk removal is always safe.
    let deleted_dir_set: std::collections::HashSet<String> = delete_dirs
        .iter()
        .filter_map(|op| {
            if let PatchOp::DeleteDir { path } = op {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect();

    // Root deleted dirs: those whose immediate parent is not also being deleted.
    let root_deleted_dirs: Vec<String> = deleted_dir_set
        .iter()
        .filter(|dir| {
            let p = std::path::Path::new(dir.as_str());
            match p.parent() {
                None => true,
                Some(parent) => {
                    let s = parent.to_str().unwrap_or("");
                    s.is_empty() || !deleted_dir_set.contains(s)
                }
            }
        })
        .cloned()
        .collect();

    // Orphan files: individual files in kept directories not covered by any root.
    let orphan_delete_files: Vec<PatchOp> = delete_files
        .into_iter()
        .filter(|op| {
            if let PatchOp::DeleteFile { path } = op {
                let mut cur = std::path::Path::new(path.as_str());
                while let Some(parent) = cur.parent() {
                    let s = parent.to_str().unwrap_or("");
                    if s.is_empty() {
                        break;
                    }
                    if deleted_dir_set.contains(s) {
                        return false; // covered by remove_dir_all on an ancestor
                    }
                    cur = parent;
                }
            }
            true
        })
        .collect();

    // 2+3+4. Add, modify, and delete files in parallel.
    // These three phases operate on disjoint path sets by construction:
    //   AddFile:    new_paths − old_paths
    //   ModifyFile: new_paths ∩ old_paths
    //   DeleteFile: old_paths − new_paths
    // so it is safe to run them concurrently.
    let target_for_add = target.clone();
    let target_for_modify = target.clone();
    let target_for_delete = target.clone();
    let (r_add, r_modify, r_delete) = tokio::try_join!(
        tokio::task::spawn_blocking(move || -> Result<()> {
            add_files.par_iter().try_for_each(|op| -> Result<()> {
                if let PatchOp::AddFile {
                    path,
                    data,
                    blake3_hash,
                } = op
                {
                    let full = target_for_add.join(path);

                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    std::fs::write(&full, data)
                        .with_context(|| format!("Failed to write file: {}", full.display()))?;

                    let actual_hash = util::hash_bytes(data);
                    if actual_hash != *blake3_hash {
                        bail!("Hash mismatch for added file: {}", path);
                    }
                }
                Ok(())
            })
        }),
        tokio::task::spawn_blocking(move || -> Result<()> {
            modify_files.par_iter().try_for_each(|op| -> Result<()> {
                if let PatchOp::ModifyFile {
                    path,
                    diff_chunks,
                    new_blake3_hash,
                } = op
                {
                    let full = target_for_modify.join(path);

                    // Scope the mmap so it is dropped before we write back to the same file.
                    // On Windows, writing to a file with an open mapping is an error (os error 1224).
                    let new_data = {
                        let old_mmap = util::mmap_file(&full)?;
                        binary_patch::apply_diff(&old_mmap, diff_chunks)
                    };

                    let actual_hash = util::hash_bytes(&new_data);
                    if actual_hash != *new_blake3_hash {
                        bail!("Hash mismatch after patching file: {}", path);
                    }

                    std::fs::write(&full, &new_data).with_context(|| {
                        format!("Failed to write patched file: {}", full.display())
                    })?;
                }
                Ok(())
            })
        }),
        tokio::task::spawn_blocking(move || -> Result<()> {
            // Bulk-remove entire deleted subtrees in parallel across roots.
            root_deleted_dirs.par_iter().try_for_each(|dir| -> Result<()> {
                let full = target_for_delete.join(dir);
                match std::fs::remove_dir_all(&full) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(anyhow::Error::from(e)).with_context(|| {
                        format!("Failed to remove directory tree: {}", full.display())
                    }),
                }
            })?;
            // Delete orphan files (in kept directories) in parallel.
            orphan_delete_files.par_iter().try_for_each(|op| -> Result<()> {
                if let PatchOp::DeleteFile { path } = op {
                    let full = target_for_delete.join(path);
                    match std::fs::remove_file(&full) {
                        Ok(()) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(anyhow::Error::from(e)).with_context(|| {
                            format!("Failed to delete file: {}", full.display())
                        }),
                    }?;
                }
                Ok(())
            })
        }),
    )?;
    r_add?;
    r_modify?;
    r_delete?;

    let summary = ApplySummary {
        dirs_created: num_create_dirs,
        files_added: num_add_files,
        files_modified: num_modify_files,
        files_deleted: num_delete_files,
        dirs_deleted: num_delete_dirs,
    };

    Ok(summary)
}
