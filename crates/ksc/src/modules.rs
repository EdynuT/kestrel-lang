//! The files of a program: the main file and every file it reaches through
//! `use`, each parsed and resolved (see [`crate::resolve`]).
//!
//! `use jogo.Mapa;` is the file `jogo/Mapa.kstl`: folders are lowercase and
//! files PascalCase, like Java's packages and classes. It is looked up in
//! three places:
//!
//! - `std.*`: the standard library, inside `ksc` ([`STD`]);
//! - the source folder (`src/` of a project): `jogo/Mapa.kstl`;
//! - the library folder (`lib/`): `jogo/Mapa.kstb`, a file compiled
//!   elsewhere, which keeps its declarations so it works without the `.kstl`.
//!
//! Two files may use each other (a cycle): each one only needs the
//! other's declarations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use kestrel_core::Module;

use crate::ast::{Block, Item, Program};
use crate::diag::{Diagnostic, SourceMap, Span};
use crate::resolve::{ModuleInfo, Scope};
use crate::{lexer, parser, resolve};

type R<T> = Result<T, Diagnostic>;

// The standard library: `STD`, a list of (`collections.HashMap`, text),
// made by `build.rs` from the `.kstl` files under the repository's `std/`.
include!(concat!(env!("OUT_DIR"), "/std.rs"));

/// The text of a file of the standard library: `collections.List`.
pub fn std_source(key: &str) -> Option<&'static str> {
    STD.iter().find(|(k, _)| *k == key).map(|(_, t)| *t)
}

/// The file of the standard library named `alias` (`Console` is
/// `io.Console`), if there is one.
pub fn std_file_named(alias: &str) -> Option<&'static str> {
    STD.iter().map(|(k, _)| *k).find(|k| k.rsplit('.').next() == Some(alias))
}

/// A file found for a `use`.
pub struct Found {
    /// The path shown in messages.
    pub shown: String,
    /// Kestrel text: the file, or the declarations a library keeps.
    pub src: String,
    /// A library (from `lib/`): its compiled code. `None` for a `.kstl`.
    pub object: Option<Module>,
}

/// Where the files of a program come from (besides `std`).
pub trait Files: Sync {
    /// The file for `use jogo.Mapa;` (`["jogo", "Mapa"]`); `Ok(None)` if
    /// there is no such file, `Err` if it cannot be used.
    fn read(&self, path: &[String]) -> Result<Option<Found>, String>;

    /// A file of the standard library as an editor has it (being edited),
    /// instead of the one built into `ksc`.
    fn std_file(&self, _path: &[String]) -> Option<Found> {
        None
    }
}

/// Files on disk: `.kstl` under `src`, libraries (`.kstb`) under `lib`.
pub struct Disk {
    pub src: PathBuf,
    pub lib: Option<PathBuf>,
}

impl Disk {
    /// A project's folders: `root/src` and `root/lib`.
    pub fn project(root: &std::path::Path) -> Disk {
        Disk { src: root.join("src"), lib: Some(root.join("lib")) }
    }
}

fn file_of(dir: &std::path::Path, path: &[String], ext: &str) -> PathBuf {
    let mut file = dir.to_path_buf();
    for p in path {
        file.push(p);
    }
    file.set_extension(ext);
    file
}

impl Files for Disk {
    fn read(&self, path: &[String]) -> Result<Option<Found>, String> {
        let ks = file_of(&self.src, path, "kstl");
        let ksb = self.lib.as_ref().map(|lib| file_of(lib, path, "kstb")).filter(|f| f.is_file());
        if ks.is_file() {
            if let Some(ksb) = ksb {
                return Err(format!("both {} and {} exist; keep one", ks.display(), ksb.display()));
            }
            let src = std::fs::read_to_string(&ks).map_err(|e| format!("cannot read {}: {e}", ks.display()))?;
            return Ok(Some(Found { shown: ks.display().to_string(), src, object: None }));
        }
        let Some(ksb) = ksb else { return Ok(None) };
        let bytes = std::fs::read(&ksb).map_err(|e| format!("cannot read {}: {e}", ksb.display()))?;
        let object = Module::from_bytes(&bytes).map_err(|e| format!("{}: {e}", ksb.display()))?;
        let Some(decls) = object.link.as_ref().map(|l| l.decls.clone()).filter(|d| !d.is_empty()) else {
            return Err(format!("{} is not a library (a file compiled by `ksc build`, from build/obj/)", ksb.display()));
        };
        Ok(Some(Found { shown: format!("{} (declarations)", ksb.display()), src: decls, object: Some(object) }))
    }
}

/// No files besides the main one and `std` (a program given as text).
pub struct NoFiles;

impl Files for NoFiles {
    fn read(&self, _: &[String]) -> Result<Option<Found>, String> {
        Ok(None)
    }
}

/// Files held in memory, by dotted path (`"jogo.Mapa"`); for tests.
impl Files for HashMap<String, String> {
    fn read(&self, path: &[String]) -> Result<Option<Found>, String> {
        let found = self.get(&path.join(".")).map(|src| Found { shown: format!("{}.kstl", path.join("/")), src: src.clone(), object: None });
        Ok(found)
    }
}

/// Where a file of the program came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The file given to `ksc`.
    Root,
    /// Another file of the program's source.
    Src,
    Std,
    /// A library from `lib/`: already compiled.
    Lib,
}

/// One file of the program, parsed and resolved.
pub struct Unit {
    pub info: Rc<ModuleInfo>,
    pub program: Program,
    /// The files it uses, by index into the list of units.
    pub uses: Vec<usize>,
    /// Where its text starts in the [`SourceMap`].
    pub base: usize,
    pub origin: Origin,
    /// A library's compiled code.
    pub object: Option<Module>,
}

/// Reads the main file and every file it reaches; the main file is unit 0.
/// `map` gets every file read, so an error can be shown in its file.
pub fn load(root_path: &str, root_src: &str, files: &dyn Files, map: &mut SourceMap) -> R<Vec<Unit>> {
    let stem = std::path::Path::new(root_path).file_stem().and_then(|s| s.to_str()).unwrap_or("main").to_string();
    let base = map.add(root_path, root_src);
    struct Loaded {
        path: String,
        program: Program,
        base: usize,
        origin: Origin,
        object: Option<Module>,
    }
    let mut loaded = vec![Loaded { path: stem.clone(), program: parse(root_src, base)?, base, origin: Origin::Root, object: None }];
    let mut index: HashMap<String, usize> = HashMap::from([(stem.clone(), 0)]);
    let mut uses: Vec<Vec<usize>> = Vec::new();
    let mut next = 0;
    while next < loaded.len() {
        let mut mine = Vec::new();
        let decls = loaded[next].program.uses.clone();
        for u in &decls {
            check_path(u)?;
            let dotted = u.dotted();
            if dotted == stem {
                return Err(Diagnostic::new(format!("`{dotted}` is the program's main file; no file can use it"), u.span)
                    .with_note("move what the other files need into a file of its own, and use that one from both"));
            }
            if dotted == loaded[next].path {
                return Err(Diagnostic::new("a file cannot use itself", u.span));
            }
            if mine.iter().any(|&k: &usize| loaded[k].path.rsplit('.').next() == Some(u.alias().name.as_str())) {
                return Err(Diagnostic::new(format!("two used files are named `{}`", u.alias().name), u.alias().span)
                    .with_note("their items would be written the same way"));
            }
            let k = match index.get(&dotted) {
                Some(&k) => k,
                None => {
                    let path: Vec<String> = u.path.iter().map(|p| p.name.clone()).collect();
                    let (found, origin) = if path[0] == "std" {
                        let key = path[1..].join(".");
                        let text = STD.iter().find(|(k, _)| *k == key).map(|(_, t)| t.to_string());
                        let found = text.map(|src| Found { shown: format!("<{}.kstl>", path.join("/")), src, object: None });
                        (files.std_file(&path).or(found), Origin::Std)
                    } else {
                        let found = files.read(&path).map_err(|e| Diagnostic::new(e, u.span))?;
                        let origin = if found.as_ref().is_some_and(|f| f.object.is_some()) { Origin::Lib } else { Origin::Src };
                        (found, origin)
                    };
                    let Some(found) = found else {
                        return Err(missing(&path, u.span));
                    };
                    let base = map.add(found.shown, found.src.clone());
                    loaded.push(Loaded { path: dotted.clone(), program: parse(&found.src, base)?, base, origin, object: found.object });
                    index.insert(dotted, loaded.len() - 1);
                    loaded.len() - 1
                }
            };
            mine.push(k);
        }
        uses.push(mine);
        next += 1;
    }
    let infos: Vec<Rc<ModuleInfo>> = loaded
        .iter()
        .enumerate()
        .map(|(k, l)| ModuleInfo::new(l.path.clone(), k == 0, &l.program).map(Rc::new))
        .collect::<R<_>>()?;
    let mut units = Vec::new();
    for (k, l) in loaded.into_iter().enumerate() {
        let mut program = l.program;
        let scope = scope_of(&infos, k, &program, &uses[k]);
        resolve::resolve(&mut program, &scope)?;
        units.push(Unit { info: infos[k].clone(), program, uses: uses[k].clone(), base: l.base, origin: l.origin, object: l.object });
    }
    Ok(units)
}

/// Folders lowercase, the file PascalCase: `use jogo.Mapa;`.
fn check_path(u: &crate::ast::UseDecl) -> R<()> {
    let (file, folders) = u.path.split_last().expect("a use has a name");
    if let Some(bad) = folders.iter().find(|p| !p.name.starts_with(|c: char| c.is_ascii_lowercase())) {
        return Err(Diagnostic::new("folder names are lowercase", bad.span)
            .with_note(format!("name the folder `{}` and write it so here", bad.name.to_ascii_lowercase())));
    }
    if !file.name.starts_with(|c: char| c.is_ascii_uppercase()) {
        let fixed = capitalize(&file.name);
        return Err(Diagnostic::new("file names are PascalCase, like types", file.span)
            .with_note(format!("name the file `{fixed}.kstl` and write `{fixed}` here")));
    }
    Ok(())
}

fn missing(path: &[String], span: Span) -> Diagnostic {
    let file = path.join("/");
    if path[0] == "std" {
        let mut have: Vec<String> = STD.iter().map(|(k, _)| format!("std.{k}")).collect();
        have.sort();
        let note = if have.is_empty() { "the standard library is still empty".to_string() } else { format!("it has {}", have.join(", ")) };
        return Diagnostic::new(format!("the standard library has no `{}`", path.join(".")), span).with_note(note);
    }
    Diagnostic::new(format!("cannot find `{file}.kstl` (or `lib/{file}.kstb`)"), span)
        .with_note("`use jogo.Mapa;` reads `jogo/Mapa.kstl` from the source folder, or the library `lib/jogo/Mapa.kstb`")
}

/// What file `k` sees.
pub fn scope_of(infos: &[Rc<ModuleInfo>], k: usize, program: &Program, uses: &[usize]) -> Scope {
    let by_alias = program.uses.iter().zip(uses).map(|(u, &m)| (u.alias().name.clone(), infos[m].clone())).collect();
    Scope { me: infos[k].clone(), uses: by_alias }
}

fn parse(src: &str, base: usize) -> R<Program> {
    parser::parse_source_at(lexer::lex_at(src, base)?, src, base)
}

/// The text other files need of a file (kept in its object, for a library):
/// its source with the bodies of plain procs and methods emptied. Generics
/// keep theirs (each user compiles its own copy), and so does everything
/// else (constants, enums, private items a generic may call).
pub fn declarations(src: &str, base: usize, program: &Program) -> String {
    let mut cut: Vec<Span> = Vec::new();
    for item in &program.items {
        match item {
            Item::Proc(p) if p.generics.is_empty() && !p.native => cut.push(p.body.span),
            Item::Class(c) if c.generics.is_empty() => cut.extend(c.methods.iter().map(|m| m.decl.body.span)),
            Item::Interface(d) => cut.extend(d.methods.iter().zip(&d.bodies).filter(|(_, b)| **b).map(|(m, _)| m.body.span)),
            _ => {}
        }
    }
    cut.sort_by_key(|s| s.start);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for s in cut {
        let (start, end) = (s.start - base, s.end - base);
        out.push_str(&src[at..start]);
        out.push_str("{ }");
        at = end;
    }
    out.push_str(&src[at..]);
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
}

/// The files `k` reaches through `use` (itself first): what compiling it reads.
pub fn closure(units: &[Unit], k: usize) -> Vec<usize> {
    let mut out = vec![k];
    let mut next = 0;
    while next < out.len() {
        for &u in &units[out[next]].uses {
            if !out.contains(&u) {
                out.push(u);
            }
        }
        next += 1;
    }
    out
}

/// What other files need of a file: its items without the bodies of
/// plain procs and methods (as a C header). Generics keep theirs (each user
/// compiles its own copy), constants and enums their values.
pub fn header(program: &Program) -> Program {
    let empty = |b: &mut Block| b.stmts.clear();
    let mut out = program.clone();
    for item in &mut out.items {
        match item {
            Item::Proc(p) if p.generics.is_empty() => empty(&mut p.body),
            Item::Class(c) if c.generics.is_empty() => c.methods.iter_mut().for_each(|m| empty(&mut m.decl.body)),
            Item::Interface(d) => d.methods.iter_mut().for_each(|m| empty(&mut m.body)),
            _ => {}
        }
    }
    out
}

/// FNV-1a: a stable hash (the same in every run and every build of ksc).
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A hash of what other files see of this one: the tokens of its
/// [`declarations`]. Editing a plain body, a comment or the layout leaves it
/// the same.
pub fn interface_hash(src: &str, base: usize, program: &Program) -> u64 {
    let decls = declarations(src, base, program);
    let mut bytes = Vec::with_capacity(decls.len());
    // The file was lexed already, so its declarations lex too.
    for t in crate::lexer::lex(&decls).unwrap_or_default() {
        bytes.extend_from_slice(&decls.as_bytes()[t.span.start..t.span.end]);
        bytes.push(0);
    }
    fnv(&bytes)
}
