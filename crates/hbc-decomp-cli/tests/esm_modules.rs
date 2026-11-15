// End-to-end: a Metro bundle decompiles to one ES module per `// === Module N`
// section, and those modules, linked to each other, run like the bytecode.
//
// The whole-bundle output is one file holding every module, so it is never one
// valid program: two modules may bind the same top-level name (module 1's
// `export const useTheme` and module 0's local `useTheme` here). Each section is
// written to its own file, specifiers are pointed at those files, and the entry
// module runs under node. Its stdout must equal the bytecode's, recorded next to
// the fixture as `<name>.expected.txt`.
//
// `import X from` is how the decompiler spells both `require(N)` and
// `importDefault(N)`, so each module is reached through a small interface file
// that adds Metro's interop default: the module's own default export when it has
// one, its exports object otherwise.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// `// === Module 3: name ===` -> 3
fn module_header(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("// === Module ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// The module sections, in order, and the entry: the last `__r(N)` of the
// trailing top-level program, which is not itself a module.
fn split_modules(text: &str) -> (Vec<(u32, Vec<String>)>, Option<u32>) {
    let mut modules: Vec<(u32, Vec<String>)> = Vec::new();
    let mut orphan = String::new();
    let mut in_orphan = false;
    for line in text.lines() {
        if let Some(id) = module_header(line) {
            modules.push((id, Vec::new()));
            in_orphan = false;
        } else if line.starts_with("// === Orphan Functions") {
            in_orphan = true;
        } else if in_orphan {
            orphan.push_str(line);
            orphan.push('\n');
        } else if let Some((_, lines)) = modules.last_mut() {
            lines.push(line.to_string());
        }
    }
    let entry = orphan.rsplit("__r(").find_map(|s| {
        let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    });
    (modules, entry)
}

// `from "name" /* 2 */` or `from "module_2"` -> `from "./m2.if.mjs"`.
fn link_line(line: &str) -> String {
    let Some(at) = line.find(" from \"") else {
        return line.to_string();
    };
    let spec_start = at + " from \"".len();
    let Some(spec_len) = line[spec_start..].find('"') else {
        return line.to_string();
    };
    let spec = &line[spec_start..spec_start + spec_len];
    let after = &line[spec_start + spec_len + 1..];
    let from_comment = after
        .trim_start()
        .strip_prefix("/*")
        .and_then(|c| c.trim_start().split(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|d| d.parse::<u32>().ok());
    let id = from_comment.or_else(|| spec.strip_prefix("module_")?.parse().ok());
    match id {
        Some(id) => format!("{} from \"./m{id}.if.mjs\";", &line[..at]),
        None => line.to_string(),
    }
}

fn run_bundle(hbc: &Path, dir: &Path) -> Result<String, String> {
    let out = dir.join("bundle.js");
    let status = Command::new(env!("CARGO_BIN_EXE_hermes-decomp"))
        .arg("decompile")
        .arg(hbc)
        .arg("--no-cache")
        .arg("-o")
        .arg(&out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("spawn: {e}"))?;
    if !status.success() {
        return Err(format!("decompile: {status}"));
    }
    let text = std::fs::read_to_string(&out).map_err(|e| e.to_string())?;
    let (modules, entry) = split_modules(&text);
    let entry = entry
        .or_else(|| modules.first().map(|m| m.0))
        .ok_or("no module sections in the output")?;
    for (id, lines) in &modules {
        let body: Vec<String> = lines.iter().map(|l| link_line(l)).collect();
        std::fs::write(dir.join(format!("m{id}.mjs")), body.join("\n") + "\n")
            .map_err(|e| e.to_string())?;
        std::fs::write(
            dir.join(format!("m{id}.if.mjs")),
            format!(
                "export * from \"./m{id}.mjs\";\nimport * as ns from \"./m{id}.mjs\";\n\
                 export default (\"default\" in ns ? ns.default : ns);\n"
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    std::fs::write(
        dir.join("shim.mjs"),
        "globalThis.print = (...a) => console.log(a.join(\" \"));\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("main.mjs"),
        format!("import \"./shim.mjs\";\nawait import(\"./m{entry}.mjs\");\n"),
    )
    .map_err(|e| e.to_string())?;
    let run = Command::new("node")
        .arg("main.mjs")
        .current_dir(dir)
        .output()
        .map_err(|e| format!("node: {e}"))?;
    let stderr = String::from_utf8_lossy(&run.stderr);
    if !run.status.success() {
        return Err(stderr.lines().take(6).collect::<Vec<_>>().join(" | "));
    }
    Ok(String::from_utf8_lossy(&run.stdout).into_owned())
}

#[test]
fn metro_modules_link_and_run_as_es_modules() {
    if !node_available() {
        eprintln!("skipped: node not found");
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/metro");
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/fixtures/metro")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "hbc"))
        .collect();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures under {}", dir.display());
    let tmp = std::env::temp_dir().join(format!("hermes-esm-{}", std::process::id()));
    let mut failures: Vec<String> = Vec::new();
    for hbc in &fixtures {
        let stem = hbc.file_stem().unwrap().to_string_lossy().into_owned();
        let name = stem.rsplit_once("_v").map_or(stem.as_str(), |(n, _)| n);
        let expected = std::fs::read_to_string(dir.join(format!("{name}.expected.txt")))
            .unwrap_or_else(|_| panic!("{name}.expected.txt next to {stem}.hbc"));
        let work = tmp.join(&stem);
        std::fs::create_dir_all(&work).expect("temp dir");
        match run_bundle(hbc, &work) {
            Ok(got) if got == expected => {}
            Ok(got) => failures.push(format!("{stem}: printed {got:?}, expected {expected:?}")),
            Err(e) => failures.push(format!("{stem}: {e}")),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "fixtures failed:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn a_specifier_resolves_by_its_id_comment_or_its_module_name() {
    assert_eq!(
        link_line("import Greeter from \"Greeter\" /* 2 */;"),
        "import Greeter from \"./m2.if.mjs\";"
    );
    assert_eq!(
        link_line("import _mod1 from \"module_1\" /* 1 */;"),
        "import _mod1 from \"./m1.if.mjs\";"
    );
    assert_eq!(
        link_line("import x from \"module_7\";"),
        "import x from \"./m7.if.mjs\";"
    );
    assert_eq!(link_line("const a = 1;"), "const a = 1;");
}
