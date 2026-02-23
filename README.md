# Patcher

A **binary patching tool** for directory trees. It computes the difference between two directory snapshots and produces a compact, compressed patch file that can be applied to transform an old version into a new version.

Use it to:

- **Create patches** — Compare an old and a new directory; output a single patch file (adds, modifications, deletes).
- **Apply patches** — Take a target directory and a patch file; update the target to match the "new" snapshot.

Patches use rsync-style binary diffs for changed files (copy + insert chunks), so only actual changes are stored. The patch file is serialized with bincode and compressed with zstd.

---

## Requirements

- [Rust](https://www.rust-lang.org/) (edition 2021; install via [rustup](https://rustup.rs/)).

---

## Quick Start

1. **Build** the tool:
   ```bash
   cargo build --release
   ```

2. **Create a patch** from an old and a new directory:
   ```bash
   ./target/release/patcher create --old ./old_version --new ./new_version --output update.patch
   ```

3. **Apply the patch** to a target directory (e.g. a copy of the old version):
   ```bash
   ./target/release/patcher apply --target ./my_install --patch update.patch
   ```

The target directory will then match the "new" snapshot. All written files are verified with BLAKE3 before the patch is considered applied.

---

## Commands

### Build

```bash
# Debug build
cargo build

# Release build (LTO + max optimization)
cargo build --release
```

### Run the tool

**Create a patch** (compare two directories, write a patch file):

```bash
cargo run -- create --old <path_to_old_dir> --new <path_to_new_dir> --output <patch_file>
```

Example:

```bash
cargo run -- create --old ./v1 --new ./v2 --output patch.bin
```

**Apply a patch** (update a directory using a patch file):

```bash
cargo run -- apply --target <path_to_target_dir> --patch <patch_file>
```

Example:

```bash
cargo run -- apply --target ./my_app --patch patch.bin
```

You can use the release binary for real use:

```bash
cargo build --release
./target/release/patcher create --old ./v1 --new ./v2 --output patch.bin
./target/release/patcher apply --target ./my_app --patch patch.bin
```

### Tests

```bash
# All tests (unit + integration)
cargo test

# Unit tests only (faster; no binary needed)
cargo test --lib

# Integration tests only (uses compiled binary)
cargo test --test integration_test

# Single test by name
cargo test test_end_to_end_full_patch_cycle
```

Integration tests run the compiled `patcher` binary; `cargo test` (without `--lib`) will build it if needed.

### Lint and format

```bash
cargo clippy
cargo fmt
```

---

## Examples

### App or game update

You have `game_v1/` and `game_v2/`. Create a patch and apply it to an installed copy:

```bash
# Create the update patch
patcher create --old game_v1 --new game_v2 --output game_update.patch

# User applies it to their install (e.g. C:\Games\MyGame)
patcher apply --target C:\Games\MyGame --patch game_update.patch
```

Only changed files are stored in the patch, so updates stay small.

### Documentation or static site

You maintain `docs/` and publish a "built" tree `site/`. Generate a patch for deploy:

```bash
patcher create --old ./site_previous --new ./site --output site_patch.patch
# On the server (or CDN): apply site_patch.patch to the live site directory
patcher apply --target /var/www/site --patch site_patch.patch
```

### Sync a folder to match another (one-way)

Make `backup/` match `source/` by treating `backup/` as the "old" tree and `source/` as the "new" one:

```bash
patcher create --old ./backup --new ./source --output sync.patch
patcher apply --target ./backup --patch sync.patch
```

After applying, `backup/` will have the same structure and file contents as `source/` (new and modified files updated, removed files and dirs deleted).

### Try it locally with dummy dirs

```bash
mkdir old new
echo "hello" > old/file.txt
echo "hello world" > new/file.txt
echo "new only" > new/extra.txt

patcher create --old old --new new --output demo.patch
mkdir target
cp old/file.txt target/
patcher apply --target target --patch demo.patch
# target/ now has file.txt ("hello world") and extra.txt ("new only")
```

---

## Dependencies

| Dependency   | Version  | Purpose |
|-------------|----------|--------|
| **clap**    | 4.5.x    | CLI parsing (subcommands and flags for `create` / `apply`). |
| **serde**   | 1.0.x    | Serialization traits for patch structures. |
| **bincode** | 1.3.x    | Binary serialization of the patch manifest. |
| **zstd**    | 0.13.x   | Compressing the serialized patch before writing to disk. |
| **blake3**  | 1.8.x    | Content hashing: verify file identity and integrity when creating/applying patches. |
| **walkdir** | 2.5.x    | Recursive directory traversal for old/new trees. |
| **tokio**   | 1.49.x   | Async runtime; overlaps I/O (e.g. walking dirs, reading files) with other work. |
| **rayon**   | 1.11.x   | Parallel CPU work: hashing, binary diffing, and apply-phase file writes/deletes. |
| **anyhow**  | 1.0.x    | Error handling and propagation. |
| **memmap2** | 0.9.x    | Memory-mapped file I/O for large files during diff/apply. |

---

## Patch format (summary)

- **File layout:** 8-byte magic `PATCHV01` + zstd-compressed bincode payload.
- **Payload:** A `PatchManifest` with an ordered list of operations:
  - **CreateDir** — create directories (parent-first).
  - **AddFile** — write new files (content + BLAKE3 hash).
  - **ModifyFile** — apply binary deltas (copy/insert chunks) and verify new BLAKE3.
  - **DeleteFile** — remove files.
  - **DeleteDir** — remove directories (deepest-first).

Paths in the manifest use forward slashes for cross-platform consistency. Modified files are represented as rsync-like diffs (fixed-size block matching with a rolling hash, then BLAKE3 confirmation).
