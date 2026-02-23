use std::collections::HashMap;

use crate::patch_format::DiffChunk;
use crate::rolling_hash::RollingHash;

pub const BLOCK_SIZE: usize = 4096;

struct BlockSignature {
    rolling_hash: u32,
    strong_hash: blake3::Hash,
    offset: u64,
}

/// Compute a binary diff between `old` and `new` data.
///
/// Uses a block-matching algorithm (rsync-like):
/// 1. Split old data into fixed-size blocks
/// 2. Build a hash table from rolling hash -> block signatures
/// 3. Scan new data with a rolling hash, matching against old blocks
/// 4. Emit Copy chunks for matches, Insert chunks for non-matching regions
pub fn compute_diff(old: &[u8], new: &[u8]) -> Vec<DiffChunk> {
    if old.is_empty() {
        if new.is_empty() {
            return vec![];
        }
        return vec![DiffChunk::Insert {
            data: new.to_vec(),
        }];
    }

    let signatures = build_signatures(old);
    let hash_table = build_hash_table(&signatures);

    match_blocks(old, new, &hash_table, &signatures)
}

fn build_signatures(data: &[u8]) -> Vec<BlockSignature> {
    let num_blocks = (data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE;
    let mut sigs = Vec::with_capacity(num_blocks);

    for i in 0..num_blocks {
        let start = i * BLOCK_SIZE;
        let end = (start + BLOCK_SIZE).min(data.len());
        let block = &data[start..end];

        let mut rolling = RollingHash::new();
        rolling.init(block);

        sigs.push(BlockSignature {
            rolling_hash: rolling.digest(),
            strong_hash: blake3::hash(block),
            offset: start as u64,
        });
    }

    sigs
}

fn build_hash_table(signatures: &[BlockSignature]) -> HashMap<u32, Vec<usize>> {
    let mut table: HashMap<u32, Vec<usize>> = HashMap::with_capacity(signatures.len());
    for (idx, sig) in signatures.iter().enumerate() {
        table.entry(sig.rolling_hash).or_default().push(idx);
    }
    table
}

fn match_blocks(
    old: &[u8],
    new: &[u8],
    hash_table: &HashMap<u32, Vec<usize>>,
    signatures: &[BlockSignature],
) -> Vec<DiffChunk> {
    let mut chunks: Vec<DiffChunk> = Vec::new();
    let mut insert_buf: Vec<u8> = Vec::new();

    if new.len() < BLOCK_SIZE {
        return vec![DiffChunk::Insert {
            data: new.to_vec(),
        }];
    }

    let mut rolling = RollingHash::new();
    rolling.init(&new[..BLOCK_SIZE]);

    let mut pos: usize = 0;

    loop {
        let window_end = pos + BLOCK_SIZE;
        if window_end > new.len() {
            break;
        }

        let digest = rolling.digest();

        if let Some(match_result) = find_match(
            digest,
            &new[pos..window_end],
            old,
            hash_table,
            signatures,
        ) {
            if !insert_buf.is_empty() {
                chunks.push(DiffChunk::Insert {
                    data: std::mem::take(&mut insert_buf),
                });
            }

            chunks.push(DiffChunk::Copy {
                offset: match_result.0,
                length: match_result.1,
            });

            pos += match_result.1 as usize;

            if pos + BLOCK_SIZE <= new.len() {
                rolling = RollingHash::new();
                rolling.init(&new[pos..pos + BLOCK_SIZE]);
            }
        } else {
            insert_buf.push(new[pos]);
            pos += 1;

            if pos + BLOCK_SIZE <= new.len() {
                rolling.rotate(new[pos - 1], new[pos + BLOCK_SIZE - 1]);
            }
        }
    }

    // Remaining bytes that don't fill a complete block window
    if pos < new.len() {
        insert_buf.extend_from_slice(&new[pos..]);
    }

    if !insert_buf.is_empty() {
        chunks.push(DiffChunk::Insert { data: insert_buf });
    }

    chunks
}

/// Try to find a matching old block for the current new window.
/// Returns (old_offset, length) on match.
fn find_match(
    rolling_digest: u32,
    new_block: &[u8],
    old: &[u8],
    hash_table: &HashMap<u32, Vec<usize>>,
    signatures: &[BlockSignature],
) -> Option<(u64, u64)> {
    let candidates = hash_table.get(&rolling_digest)?;

    let new_strong = blake3::hash(new_block);

    for &sig_idx in candidates {
        let sig = &signatures[sig_idx];
        if sig.strong_hash == new_strong {
            let block_end = (sig.offset as usize + BLOCK_SIZE).min(old.len());
            let block_len = block_end - sig.offset as usize;
            return Some((sig.offset, block_len as u64));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_patch::apply_diff;

    #[test]
    fn test_identical_data() {
        let data = vec![42u8; BLOCK_SIZE * 3];
        let chunks = compute_diff(&data, &data);
        let result = apply_diff(&data, &chunks);
        assert_eq!(result, data);
    }

    #[test]
    fn test_completely_different() {
        let old = vec![0u8; BLOCK_SIZE * 2];
        let new = vec![1u8; BLOCK_SIZE * 2];
        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);
    }

    #[test]
    fn test_prefix_changed() {
        let old = vec![0u8; BLOCK_SIZE * 4];
        let mut new = old.clone();
        // Change only the first block
        for b in new[..BLOCK_SIZE].iter_mut() {
            *b = 0xFF;
        }

        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);

        // Should have Copy chunks for unchanged blocks
        let copy_count = chunks
            .iter()
            .filter(|c| matches!(c, DiffChunk::Copy { .. }))
            .count();
        assert!(copy_count >= 3, "Expected at least 3 Copy chunks for unchanged blocks");
    }

    #[test]
    fn test_empty_old() {
        let old = vec![];
        let new = vec![1u8; 100];
        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);
    }

    #[test]
    fn test_empty_new() {
        let old = vec![1u8; 100];
        let new = vec![];
        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);
    }

    #[test]
    fn test_small_files() {
        let old = b"Hello, World!".to_vec();
        let new = b"Hello, Rust!".to_vec();
        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);
    }

    #[test]
    fn test_insertion_in_middle() {
        let mut old = vec![0u8; BLOCK_SIZE * 4];
        for (i, b) in old.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let mut new = old.clone();
        // Insert some bytes in the middle (between block 1 and block 2)
        let insert_pos = BLOCK_SIZE * 2;
        let insertion = vec![0xAA; 100];
        new.splice(insert_pos..insert_pos, insertion);

        let chunks = compute_diff(&old, &new);
        let result = apply_diff(&old, &chunks);
        assert_eq!(result, new);
    }
}
