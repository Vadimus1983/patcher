use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 8] = b"PATCHV01";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct PatchManifest {
    pub version: u32,
    pub operations: Vec<PatchOp>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PatchOp {
    CreateDir {
        path: String,
    },
    AddFile {
        path: String,
        data: Vec<u8>,
        blake3_hash: [u8; 32],
    },
    ModifyFile {
        path: String,
        diff_chunks: Vec<DiffChunk>,
        new_blake3_hash: [u8; 32],
    },
    DeleteFile {
        path: String,
    },
    DeleteDir {
        path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiffChunk {
    Copy { offset: u64, length: u64 },
    Insert { data: Vec<u8> },
}

pub struct ApplySummary {
    pub dirs_created: usize,
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub dirs_deleted: usize,
}

