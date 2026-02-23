use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::binary_diff;
use crate::patch_format::{ApplySummary, DiffChunk, PatchManifest, PatchOp, FORMAT_VERSION, MAGIC};
use crate::util::{self, EntryKind};

/// Returns true for file types that are already compressed or otherwise incompressible,
/// where computing a binary diff would yield no meaningful savings.
fn is_incompressible(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            // Images
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "ico" | "tiff" | "tif" | "avif"
            // Video
            | "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v"
            // Audio
            | "mp3" | "aac" | "ogg" | "flac" | "opus" | "m4a" | "wma"
            // Archives
            | "zip" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar"
            // Office (zip-based containers)
            | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp"
            // Fonts
            | "woff" | "woff2"
            // Other
            | "pdf"
        )
    )
}

/// Stream-hash a file using BLAKE3.
/// Uses a 256 KB BufReader to reduce syscall overhead vs the default 8 KB.
fn hash_file_streaming(path: &Path) -> Result<blake3::Hash> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open file for hashing: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut reader, &mut hasher)
        .with_context(|| format!("Failed to hash file: {}", path.display()))?;
    Ok(hasher.finalize())
}

/// Create a patch file by comparing old_dir and new_dir.
/// Uses Tokio for concurrent directory walks and Rayon for parallel hashing/diffing.
pub async fn create_patch(
    old_dir: &Path,
    new_dir: &Path,
    output: &Path,
) -> Result<ApplySummary> {
    // Stage 1: Walk both directories concurrently
    let old_dir_owned = old_dir.to_path_buf();
    let new_dir_owned = new_dir.to_path_buf();

    let (old_entries, new_entries) = tokio::try_join!(
        tokio::task::spawn_blocking(move || util::walk_directory(&old_dir_owned)),
        tokio::task::spawn_blocking(move || util::walk_directory(&new_dir_owned)),
    )?;

    let old_entries = old_entries?;
    let new_entries = new_entries?;

    // Stage 2: Classify changes using index-based lookups (no references across spawn_blocking)
    let old_map: HashMap<String, usize> = old_entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.relative_path.clone(), i))
        .collect();
    let new_map: HashMap<String, usize> = new_entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.relative_path.clone(), i))
        .collect();

    let old_paths = util::path_set(&old_entries);
    let new_paths = util::path_set(&new_entries);

    let mut dirs_to_create: Vec<String> = Vec::new();
    let mut files_to_add: Vec<usize> = Vec::new(); // indices into new_entries
    let mut files_maybe_modified: Vec<(usize, usize)> = Vec::new(); // (old_idx, new_idx)
    let mut files_to_delete: Vec<String> = Vec::new();
    let mut dirs_to_delete: Vec<String> = Vec::new();

    for path in new_paths.difference(&old_paths) {
        let idx = new_map[path];
        match new_entries[idx].kind {
            EntryKind::Dir => dirs_to_create.push(path.clone()),
            EntryKind::File => files_to_add.push(idx),
        }
    }

    for path in old_paths.difference(&new_paths) {
        let idx = old_map[path];
        match old_entries[idx].kind {
            EntryKind::Dir => dirs_to_delete.push(path.clone()),
            EntryKind::File => files_to_delete.push(path.clone()),
        }
    }

    for path in old_paths.intersection(&new_paths) {
        let old_idx = old_map[path];
        let new_idx = new_map[path];
        if old_entries[old_idx].kind == EntryKind::File
            && new_entries[new_idx].kind == EntryKind::File
        {
            files_maybe_modified.push((old_idx, new_idx));
        }
    }

    // Stage 3+4 merged: stream-hash to confirm changes, then mmap+diff only confirmed-modified files.
    // If sizes differ the file is definitely changed: skip hashing old (saves one file read).
    struct DiffInput {
        rel_path: String,
        old_path: std::path::PathBuf,
        new_path: std::path::PathBuf,
        sizes_differ: bool,
    }

    let diff_inputs: Vec<DiffInput> = files_maybe_modified
        .iter()
        .map(|&(oi, ni)| DiffInput {
            rel_path: old_entries[oi].relative_path.clone(),
            old_path: old_entries[oi].full_path.clone(),
            new_path: new_entries[ni].full_path.clone(),
            sizes_differ: old_entries[oi].size != new_entries[ni].size,
        })
        .collect();

    let add_inputs: Vec<(String, std::path::PathBuf)> = files_to_add
        .iter()
        .map(|&ni| {
            (
                new_entries[ni].relative_path.clone(),
                new_entries[ni].full_path.clone(),
            )
        })
        .collect();

    let num_files_added = add_inputs.len();

    // Stage 3+4: Hash + diff (Rayon par_iter inside spawn_blocking).
    // Hash phase uses 256 KB BufReader to reduce syscall overhead.
    // sizes_differ → skip hashing old file (definitely changed).
    // Identical hash → skip diff entirely.
    let (diff_results, add_results) = tokio::try_join!(
        tokio::task::spawn_blocking(
            move || -> Result<Vec<(String, Vec<DiffChunk>, [u8; 32])>> {
                Ok(diff_inputs
                    .par_iter()
                    .map(|input| -> Result<Option<(String, Vec<DiffChunk>, [u8; 32])>> {
                        let new_hash_blake3 = hash_file_streaming(&input.new_path)?;
                        if !input.sizes_differ {
                            let old_hash = hash_file_streaming(&input.old_path)?;
                            if old_hash == new_hash_blake3 {
                                return Ok(None);
                            }
                        }
                        let new_hash = *new_hash_blake3.as_bytes();

                        let chunks = if is_incompressible(&input.new_path) {
                            let new_data = util::mmap_file(&input.new_path)?;
                            vec![DiffChunk::Insert { data: new_data.to_vec() }]
                        } else {
                            let old_data = util::mmap_file(&input.old_path)?;
                            let new_data = util::mmap_file(&input.new_path)?;
                            binary_diff::compute_diff(&old_data, &new_data)
                        };

                        Ok(Some((input.rel_path.clone(), chunks, new_hash)))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect())
            }
        ),
        tokio::task::spawn_blocking(move || -> Result<Vec<(String, Vec<u8>, [u8; 32])>> {
            add_inputs
                .par_iter()
                .map(|(rel_path, full_path)| -> Result<(String, Vec<u8>, [u8; 32])> {
                    let mmap = util::mmap_file(full_path)?;
                    let hash = util::hash_bytes(&mmap);
                    Ok((rel_path.clone(), mmap.to_vec(), hash))
                })
                .collect()
        }),
    )?;

    let diff_results = diff_results?;
    let add_results = add_results?;
    let num_files_modified = diff_results.len();

    // Stage 5: Assemble operations in correct order
    let mut operations: Vec<PatchOp> = Vec::new();

    // 1. CreateDir (parent-first)
    util::sort_dirs_parent_first(&mut dirs_to_create);
    for path in &dirs_to_create {
        operations.push(PatchOp::CreateDir {
            path: path.clone(),
        });
    }

    // 2. AddFile
    for (path, data, hash) in add_results {
        operations.push(PatchOp::AddFile {
            path,
            data,
            blake3_hash: hash,
        });
    }

    // 3. ModifyFile
    for (path, diff_chunks, new_hash) in diff_results {
        operations.push(PatchOp::ModifyFile {
            path,
            diff_chunks,
            new_blake3_hash: new_hash,
        });
    }

    // 4. DeleteFile
    for path in &files_to_delete {
        operations.push(PatchOp::DeleteFile {
            path: path.clone(),
        });
    }

    // 5. DeleteDir (deepest-first)
    util::sort_dirs_deepest_first(&mut dirs_to_delete);
    for path in &dirs_to_delete {
        operations.push(PatchOp::DeleteDir {
            path: path.clone(),
        });
    }

    let manifest = PatchManifest {
        version: FORMAT_VERSION,
        operations,
    };

    // Serialize, compress, write
    let encoded =
        bincode::serialize(&manifest).context("Failed to serialize patch manifest")?;

    let compressed =
        zstd::bulk::compress(&encoded, 3).context("Failed to compress patch data")?;

    let mut file = std::fs::File::create(output)
        .with_context(|| format!("Failed to create output file: {}", output.display()))?;
    file.write_all(MAGIC)?;
    file.write_all(&compressed)?;
    file.flush()?;

    let summary = ApplySummary {
        dirs_created: dirs_to_create.len(),
        files_added: num_files_added,
        files_modified: num_files_modified,
        files_deleted: files_to_delete.len(),
        dirs_deleted: dirs_to_delete.len(),
    };

    Ok(summary)
}
