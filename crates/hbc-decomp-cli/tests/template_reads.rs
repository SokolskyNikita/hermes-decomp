// Metro propagation renames a module's import binding (`_module` -> `clamp`,
// the name of the module it requires) with `rename_variables_in_stmts`. That
// walk skipped template literals, spreads, await and yield, so a read inside
// `${...}` kept the old name and the module threw a ReferenceError.
// Fixture: tests/fixtures/metro/template_import.source.js, compiled with npm
// hermes-compiler@250829098.0.16 (HBC 98) -O -g0.

use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn an_import_read_inside_a_template_literal_is_renamed() {
    let hbc =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/metro/template_import_v98.hbc");
    let out = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
        .arg("decompile")
        .arg("--no-cache")
        .arg(&hbc)
        .stderr(Stdio::null())
        .output()
        .expect("run hermes-decomp");
    assert!(out.status.success(), "hermes-decomp failed");
    let js = String::from_utf8_lossy(&out.stdout);
    let module = js
        .split("// === Module 0")
        .nth(1)
        .and_then(|m| m.split("// === ").next())
        .expect("module 0 section");
    assert!(
        module.contains("import clamp from"),
        "module 0 imports `clamp`:\n{module}"
    );
    assert!(
        module.contains("${clamp.clamp(-5)}") && module.contains("${clamp.isMissing(null)}"),
        "template holes must read the import binding:\n{module}"
    );
    assert!(
        !module.contains("_module"),
        "a template hole kept the old name:\n{module}"
    );
}
