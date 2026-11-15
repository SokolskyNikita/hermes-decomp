// HERMES_DECOMP_CACHE_DIR: the analysis cache goes to that directory, keyed by
// the input's content, and nothing is written next to the input.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jsx_cycle_v98.hbc")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hd-cache-dir-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn decompile(input: &Path, cache_dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
        .arg("decompile")
        .arg(input)
        .env("HERMES_DECOMP_CACHE_DIR", cache_dir)
        .output()
        .expect("run hermes-decomp");
    assert!(
        out.status.success(),
        "decompile failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn cache_dir_keeps_the_input_folder_clean_and_is_shared_by_copies() {
    let a = scratch("a");
    let b = scratch("b");
    let cache = scratch("cache");
    let in_a = a.join("bundle.hbc");
    let in_b = b.join("copy.hbc");
    std::fs::copy(fixture(), &in_a).unwrap();
    std::fs::copy(fixture(), &in_b).unwrap();

    let first = decompile(&in_a, &cache);
    let second = decompile(&in_b, &cache);

    assert_eq!(
        files_in(&a),
        vec!["bundle.hbc"],
        "nothing may be written next to the input"
    );
    assert_eq!(
        files_in(&b),
        vec!["copy.hbc"],
        "nothing may be written next to the input"
    );
    let cached = files_in(&cache);
    assert_eq!(
        cached.len(),
        1,
        "two copies of one input share one entry: {cached:?}"
    );
    assert!(cached[0].ends_with(".hdcache"));
    assert_eq!(first, second, "the cached run prints the same output");

    for d in [a, b, cache] {
        let _ = std::fs::remove_dir_all(d);
    }
}
