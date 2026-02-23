use std::fs;
use std::path::Path;
use std::process::Command;

fn patcher_exe() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove 'deps'
    path.push("patcher.exe");
    path
}

fn create_dir_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (rel_path, content) in files {
        let full = root.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
    }
}

fn collect_dir_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    collect_recursive(root, root, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn collect_recursive(root: &Path, current: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
    let mut dir_entries: Vec<_> = fs::read_dir(current).unwrap().collect::<Result<_, _>>().unwrap();
    dir_entries.sort_by_key(|e| e.file_name());

    for entry in dir_entries {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().to_str().unwrap().replace('\\', "/");

        if path.is_dir() {
            collect_recursive(root, &path, entries);
        } else {
            let content = fs::read(&path).unwrap();
            entries.push((rel, content));
        }
    }
}

#[test]
fn test_end_to_end_full_patch_cycle() {
    let temp = std::env::temp_dir().join("patcher_e2e_test");
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).unwrap();

    let old_dir = temp.join("old");
    let new_dir = temp.join("new");
    let target_dir = temp.join("target");
    let patch_file = temp.join("test.patch");

    // OLD version: some files and directories
    create_dir_tree(
        &old_dir,
        &[
            ("readme.txt", b"Hello, World! This is version 1."),
            ("config/settings.json", b"{\"version\": 1, \"debug\": false}"),
            ("data/records.bin", &vec![0xAA; 8192]),
            ("data/old_file.txt", b"This file will be deleted"),
            ("obsolete/remove_me.txt", b"Going away"),
        ],
    );

    // NEW version: modifications, additions, deletions
    let mut modified_bin = vec![0xAA; 4096];
    modified_bin.extend_from_slice(&vec![0xBB; 4096]);

    create_dir_tree(
        &new_dir,
        &[
            ("readme.txt", b"Hello, World! This is version 2 with new features."),
            ("config/settings.json", b"{\"version\": 2, \"debug\": true, \"newField\": 42}"),
            ("data/records.bin", &modified_bin),
            ("data/new_file.txt", b"Brand new file in version 2"),
            ("extras/bonus.dat", &vec![0xFF; 1024]),
        ],
    );

    // Create a copy of old as our target (simulates installed app)
    copy_dir_recursive(&old_dir, &target_dir);

    let exe = patcher_exe();

    // Step 1: Create patch
    let output = Command::new(&exe)
        .args(["create", "--old", old_dir.to_str().unwrap(), "--new", new_dir.to_str().unwrap(), "--output", patch_file.to_str().unwrap()])
        .output()
        .expect("Failed to run patcher create");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "patcher create failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    assert!(patch_file.exists(), "Patch file should have been created");
    assert!(
        fs::metadata(&patch_file).unwrap().len() > 8,
        "Patch file should be larger than just the magic header"
    );

    println!("Create output:\n{}", stdout);
    println!(
        "Patch file size: {} bytes",
        fs::metadata(&patch_file).unwrap().len()
    );

    // Step 2: Apply patch to target
    let output = Command::new(&exe)
        .args(["apply", "--target", target_dir.to_str().unwrap(), "--patch", patch_file.to_str().unwrap()])
        .output()
        .expect("Failed to run patcher apply");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "patcher apply failed:\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    println!("Apply output:\n{}", stdout);

    // Step 3: Verify target matches new_dir
    let expected = collect_dir_tree(&new_dir);
    let actual = collect_dir_tree(&target_dir);

    assert_eq!(
        expected.len(),
        actual.len(),
        "File count mismatch.\nExpected files: {:?}\nActual files: {:?}",
        expected.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        actual.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );

    for ((exp_path, exp_data), (act_path, act_data)) in expected.iter().zip(actual.iter()) {
        assert_eq!(exp_path, act_path, "Path mismatch");
        assert_eq!(
            exp_data, act_data,
            "Content mismatch for file: {}",
            exp_path
        );
    }

    // Verify deleted files are actually gone
    assert!(!target_dir.join("data/old_file.txt").exists());
    assert!(!target_dir.join("obsolete/remove_me.txt").exists());
    assert!(!target_dir.join("obsolete").exists());

    // Verify new files exist
    assert!(target_dir.join("data/new_file.txt").exists());
    assert!(target_dir.join("extras/bonus.dat").exists());

    // Cleanup
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_empty_to_full() {
    let temp = std::env::temp_dir().join("patcher_e2e_empty_to_full");
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).unwrap();

    let old_dir = temp.join("old");
    let new_dir = temp.join("new");
    let target_dir = temp.join("target");
    let patch_file = temp.join("test.patch");

    fs::create_dir_all(&old_dir).unwrap();
    fs::create_dir_all(&target_dir).unwrap();

    create_dir_tree(
        &new_dir,
        &[
            ("file1.txt", b"Content of file 1"),
            ("sub/file2.txt", b"Content of file 2"),
        ],
    );

    let exe = patcher_exe();

    let output = Command::new(&exe)
        .args(["create", "--old", old_dir.to_str().unwrap(), "--new", new_dir.to_str().unwrap(), "--output", patch_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "create failed: {}", String::from_utf8_lossy(&output.stderr));

    let output = Command::new(&exe)
        .args(["apply", "--target", target_dir.to_str().unwrap(), "--patch", patch_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "apply failed: {}", String::from_utf8_lossy(&output.stderr));

    let expected = collect_dir_tree(&new_dir);
    let actual = collect_dir_tree(&target_dir);
    assert_eq!(expected, actual);

    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_no_changes() {
    let temp = std::env::temp_dir().join("patcher_e2e_no_changes");
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).unwrap();

    let old_dir = temp.join("old");
    let new_dir = temp.join("new");
    let target_dir = temp.join("target");
    let patch_file = temp.join("test.patch");

    let files: &[(&str, &[u8])] = &[
        ("a.txt", b"Same content"),
        ("sub/b.txt", b"Also same"),
    ];
    create_dir_tree(&old_dir, files);
    create_dir_tree(&new_dir, files);
    copy_dir_recursive(&old_dir, &target_dir);

    let exe = patcher_exe();

    let output = Command::new(&exe)
        .args(["create", "--old", old_dir.to_str().unwrap(), "--new", new_dir.to_str().unwrap(), "--output", patch_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = Command::new(&exe)
        .args(["apply", "--target", target_dir.to_str().unwrap(), "--patch", patch_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    let expected = collect_dir_tree(&new_dir);
    let actual = collect_dir_tree(&target_dir);
    assert_eq!(expected, actual);

    let _ = fs::remove_dir_all(&temp);
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}
