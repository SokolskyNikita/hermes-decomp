// Metro registers a module without dependencies as `__d(factory, id)`, a
// 3-argument call. Fixtures: tests/fixtures/metro_no_deps.source.js compiled
// as HBC 96 and HBC 98, one module with a dependency map and one without.

use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn metro_module_without_dependency_map_is_detected() {
    for name in ["metro_no_deps_v96.hbc", "metro_no_deps_v98.hbc"] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        // `modules` keeps an analysis cache next to its input, so list a copy.
        let copy =
            std::env::temp_dir().join(format!("hermes-nodeps-{}-{name}", std::process::id()));
        std::fs::copy(&fixture, &copy).expect("copy fixture");
        let out = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
            .arg("modules")
            .arg(&copy)
            .stderr(Stdio::null())
            .output()
            .expect("run hermes-decomp");
        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_file(copy.with_extension("hbc.hdcache"));
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(
            listing.contains("Total modules: 2"),
            "{name}: expected both modules in:\n{listing}"
        );
    }
}
