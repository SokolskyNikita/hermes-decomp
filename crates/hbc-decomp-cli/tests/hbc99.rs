// HBC 99 regressions, against tracked bytecode built by the released
// compiler (npm `hermes-compiler@260318099.0.4`). The source of each fixture
// sits next to it as `<name>.source.js`.
//
// `hbc99_ops_v99.hbc` covers the opcode table (`==`, `!=`, `===`, `!==`
// jumps), the large function header (a try/catch only survives when the
// exception table is found) and the `undefined` literal buffer tag.
// `metro_no_deps_v*.hbc` covers a Metro module registered without a
// dependency map.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run(args: &[&std::ffi::OsStr]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
        .args(args)
        .stderr(Stdio::null())
        .output()
        .expect("run hermes-decomp");
    assert!(out.status.success(), "hermes-decomp {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// Tests run in parallel and may decompile the same fixture, so each call
// writes its own output file.
fn decompile(name: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp =
        std::env::temp_dir().join(format!("hermes-hbc99-{}-{n}-{name}.js", std::process::id()));
    run(&[
        "decompile".as_ref(),
        fixture(name).as_os_str(),
        "--no-cache".as_ref(),
        "-o".as_ref(),
        tmp.as_os_str(),
    ]);
    let js = std::fs::read_to_string(&tmp).expect("decompiled output");
    let _ = std::fs::remove_file(&tmp);
    js
}

#[test]
fn v99_comparisons_try_catch_and_literals_decode() {
    let js = decompile("hbc99_ops_v99.hbc");
    for needle in [
        "arg0 === arg1",
        "arg0 !== arg1",
        "arg0 == arg1",
        "arg0 != arg1",
        "try {",
        "catch (",
        "b: undefined",
        "c: \"x\"",
    ] {
        assert!(js.contains(needle), "missing `{needle}` in:\n{js}");
    }
}

// The decompiled program prints what the source prints. Skipped without node.
#[test]
fn v99_ops_fixture_runs_like_its_source() {
    let node_ok = Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !node_ok {
        eprintln!("skipped: node not found");
        return;
    }
    let shim = "globalThis.print=(...a)=>console.log(a.join(' '));\n";
    let program = format!(
        "{shim}{}\nglobal(globalThis);\n",
        decompile("hbc99_ops_v99.hbc")
    );
    let out = Command::new("node")
        .arg("-e")
        .arg(program)
        .output()
        .expect("run node");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        r#"seq,eq sne,eq sne,ne ok boom {"a":1,"c":"x"} true"#
    );
}

#[test]
fn metro_module_without_dependency_map_is_detected() {
    for name in [
        "metro_no_deps_v96.hbc",
        "metro_no_deps_v98.hbc",
        "metro_no_deps_v99.hbc",
    ] {
        // `modules` keeps an analysis cache next to its input, so list a copy.
        let copy = std::env::temp_dir().join(format!("hermes-hbc99-{}-{name}", std::process::id()));
        std::fs::copy(fixture(name), &copy).expect("copy fixture");
        let listing = run(&["modules".as_ref(), copy.as_os_str()]);
        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_file(copy.with_extension("hbc.hdcache"));
        assert!(
            listing.contains("Total modules: 2"),
            "{name}: expected both modules in:\n{listing}"
        );
    }
}
