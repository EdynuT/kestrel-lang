//! Builds the standard library into `ksc`: every `.ks` file under the
//! repository's `std/` becomes an entry of `STD` (see `modules.rs`).

use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    if root.is_dir() {
        walk(&root, &mut files);
    }
    files.sort();
    let mut out = String::from("/// The standard library: (`collections.HashMap`, its text).\npub const STD: &[(&str, &str)] = &[\n");
    for f in &files {
        let rel = f.strip_prefix(&root).expect("under std/").with_extension("");
        let key: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let abs = f.canonicalize().expect("a file that exists");
        out += &format!("    ({:?}, include_str!({:?})),\n", key.join("."), abs.display().to_string());
    }
    out += "];\n";
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("std.rs");
    std::fs::write(dest, out).expect("cannot write std.rs");
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "ks") {
            out.push(p);
        }
    }
}
