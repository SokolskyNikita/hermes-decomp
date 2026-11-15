// A single-use value computed before an `if` and read inside it after the
// arm's first statement (or in the `else` arm) lost its definition: the
// inliner dropped `v = o.load()` to substitute it, the substitution never
// reached the read, and the output read a temporary assigned nowhere.
// Fixture: tests/fixtures/nested_use.source.js, compiled with
// `hermesc -emit-binary -O -g0` from npm hermes-compiler@250829098.0.16 (HBC 98).

use std::path::Path;
use std::process::Command;

#[test]
fn a_value_read_deep_in_a_branch_keeps_its_definition() {
    let hbc = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nested_use_v98.hbc");
    let out = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
        .arg("decompile")
        .arg("--no-cache")
        .arg(&hbc)
        .output()
        .expect("run hermes-decomp");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let js = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        js.matches(".load()").count(),
        2,
        "each o.load() call must survive:\n{js}"
    );
}
