use crate::patch_format::DiffChunk;

/// Reconstruct the new file from the old file data and a sequence of diff chunks.
pub fn apply_diff(old: &[u8], chunks: &[DiffChunk]) -> Vec<u8> {
    let estimated_size: u64 = chunks
        .iter()
        .map(|c| match c {
            DiffChunk::Copy { length, .. } => *length,
            DiffChunk::Insert { data } => data.len() as u64,
        })
        .sum();

    let mut result = Vec::with_capacity(estimated_size as usize);

    for chunk in chunks {
        match chunk {
            DiffChunk::Copy { offset, length } => {
                let start = *offset as usize;
                let end = start + *length as usize;
                result.extend_from_slice(&old[start..end]);
            }
            DiffChunk::Insert { data } => {
                result.extend_from_slice(data);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_copy_only() {
        let old = b"Hello, World!";
        let chunks = vec![DiffChunk::Copy {
            offset: 0,
            length: old.len() as u64,
        }];
        let result = apply_diff(old, &chunks);
        assert_eq!(result, old);
    }

    #[test]
    fn test_apply_insert_only() {
        let old = b"";
        let new_data = b"Brand new content";
        let chunks = vec![DiffChunk::Insert {
            data: new_data.to_vec(),
        }];
        let result = apply_diff(old, &chunks);
        assert_eq!(result, new_data);
    }

    #[test]
    fn test_apply_mixed() {
        let old = b"AAAA_BBBB_CCCC";
        let chunks = vec![
            DiffChunk::Copy {
                offset: 0,
                length: 5,
            },
            DiffChunk::Insert {
                data: b"XXXX_".to_vec(),
            },
            DiffChunk::Copy {
                offset: 10,
                length: 4,
            },
        ];
        let result = apply_diff(old, &chunks);
        assert_eq!(result, b"AAAA_XXXX_CCCC");
    }

    #[test]
    fn test_apply_empty_chunks() {
        let old = b"some data";
        let chunks: Vec<DiffChunk> = vec![];
        let result = apply_diff(old, &chunks);
        assert!(result.is_empty());
    }
}
