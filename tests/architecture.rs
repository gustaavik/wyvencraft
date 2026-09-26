//! The layer rule, checked.
//!
//! The engine/game line is enforced by cargo: no `wyven-*` crate lists the
//! game, so a violation stops compiling. The layers *inside* the game crate
//! share one crate and so cannot be policed that way — this test is the
//! substitute. It reads the source tree (nothing else) and fails on any
//! import that points the wrong way:
//!
//! ```text
//! presentation ─┐
//!               ├─→ application ─→ domain
//! infrastructure┘
//! ```
//!
//! - `domain` is the rules. No file, socket, GPU or egui; no outer layer.
//! - `application` is the use cases and their ports. It may use the network
//!   *vocabulary* (`wyven_net::PlayerId`) but no adapter, and nothing that draws.
//!
//! Comments are skipped: a doc link to an outer layer explains a boundary, it
//! does not cross one.

use std::fs;
use std::path::{Path, PathBuf};

/// What a file under a layer may not name, and why.
struct Rule {
    layer: &'static str,
    forbidden: &'static [(&'static str, &'static str)],
}

const OUTER_FROM_DOMAIN: &str = "the domain depends on no outer layer";
const OUTER_FROM_APPLICATION: &str = "application depends only on domain";

const RULES: &[Rule] = &[
    Rule {
        layer: "domain",
        forbidden: &[
            ("crate::application", OUTER_FROM_DOMAIN),
            ("crate::infrastructure", OUTER_FROM_DOMAIN),
            ("crate::presentation", OUTER_FROM_DOMAIN),
            ("crate::boot", OUTER_FROM_DOMAIN),
            ("crate::app::", OUTER_FROM_DOMAIN),
            ("egui", "the domain draws nothing"),
            ("wyven_render", "the domain draws nothing"),
            ("vulkano", "the domain draws nothing"),
            ("wyven_app", "the domain draws nothing"),
            ("wyven_net", "the domain knows no transport"),
            ("renet", "the domain knows no transport"),
            ("std::fs", "the domain does no I/O"),
            ("fs::read", "the domain does no I/O"),
            ("fs::write", "the domain does no I/O"),
        ],
    },
    Rule {
        layer: "application",
        forbidden: &[
            ("crate::infrastructure", OUTER_FROM_APPLICATION),
            ("crate::presentation", OUTER_FROM_APPLICATION),
            ("crate::boot", OUTER_FROM_APPLICATION),
            ("crate::app::", OUTER_FROM_APPLICATION),
            ("egui", "application draws nothing"),
            ("wyven_render", "application draws nothing"),
            ("vulkano", "application draws nothing"),
            ("wyven_app", "application draws nothing"),
            ("std::fs", "application reaches files through a port"),
        ],
    },
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The code on a line, with any `//` comment removed. Good enough for import
/// paths: a `//` inside a string literal would only hide code from the check,
/// never invent a violation.
fn code(line: &str) -> &str {
    line.find("//").map_or(line, |i| &line[..i])
}

fn violations(rule: &Rule) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(rule.layer);
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty(), "no sources under {}", root.display());

    let mut found = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("readable source");
        for (n, line) in text.lines().enumerate() {
            let code = code(line);
            for (needle, why) in rule.forbidden {
                if code.contains(needle) {
                    let rel = file
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&file);
                    found.push(format!("{}:{}: `{needle}` — {why}", rel.display(), n + 1));
                }
            }
        }
    }
    found
}

#[test]
fn the_domain_layer_depends_on_nothing_outside_it() {
    let found = violations(&RULES[0]);
    assert!(found.is_empty(), "layer violations:\n{}", found.join("\n"));
}

#[test]
fn the_application_layer_depends_only_on_the_domain() {
    let found = violations(&RULES[1]);
    assert!(found.is_empty(), "layer violations:\n{}", found.join("\n"));
}

#[test]
fn the_comment_filter_keeps_code_and_drops_comments() {
    assert_eq!(
        code("use crate::domain::core; // crate::presentation"),
        "use crate::domain::core; "
    );
    assert_eq!(code("/// [`crate::presentation::ui`]"), "");
    assert_eq!(code("let x = 1;"), "let x = 1;");
}
