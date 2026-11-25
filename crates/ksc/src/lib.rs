//! ksc: the Kestrel compiler (`.kstl` -> `.kstb`).
//!
//! Phases: [`lexer`] -> [`parser`] -> [`modules`] (the files reached by `use`,
//! with names resolved by [`resolve`]) -> [`generics`] (one copy per use) ->
//! [`sema`] (types, HIR) -> [`throws`] (checked errors) -> [`linear`] (ownership) -> [`codegen`] (bytecode).

pub mod ast;
pub mod codegen;
pub mod diag;
pub mod docs;
pub mod generics;
pub mod hir;
pub mod lexer;
pub mod linear;
pub mod modules;
pub mod nav;
pub mod native;
pub mod object;
pub mod overload;
pub mod parser;
pub mod resolve;
pub mod sema;
pub mod throws;

use kestrel_core::Module;

/// A compiled program with what the compiler found along the way.
pub struct Output {
    pub module: Module,
    /// Warnings (`@deprecated` uses); they do not stop the compilation.
    pub warnings: Vec<diag::Diagnostic>,
    /// `@test` procs: name and function index, to run each as the entry.
    pub tests: Vec<(String, u32)>,
    /// The files compiled this time (with a [`Cache`], the others were up to date).
    pub compiled: Vec<String>,
}

/// Where [`build`] keeps each file's object (`.kstb`) between runs, like
/// `make`: a file is compiled again only if its text changed, or the
/// public part of a file it uses, or the compiler.
pub struct Cache {
    pub dir: std::path::PathBuf,
    /// Tells this `ksc` from others (objects of another one are not used).
    pub compiler: u64,
}

/// Runs every phase on a source file.
pub fn compile(src: &str) -> Result<Module, diag::Diagnostic> {
    compile_full(src, true).map(|o| o.module)
}

/// Like [`compile`], also giving the warnings and the tests. With
/// `need_main: false` (for `ksc --test`), a file without `main` is fine.
pub fn compile_full(src: &str, need_main: bool) -> Result<Output, diag::Diagnostic> {
    compile_program("main.kstl", src, &modules::NoFiles, need_main).1
}

/// Compiles the program whose main file is `root_path` (with text
/// `root_src`), reading the files it uses from `files`. Gives the files it
/// read, to show the messages in the right one.
pub fn compile_program(
    root_path: &str,
    root_src: &str,
    files: &dyn modules::Files,
    need_main: bool,
) -> (diag::SourceMap, Result<Output, diag::Diagnostic>) {
    build(root_path, root_src, files, need_main, None)
}

/// Like [`compile_program`], keeping the objects in `cache`.
pub fn build(
    root_path: &str,
    root_src: &str,
    files: &dyn modules::Files,
    need_main: bool,
    cache: Option<&Cache>,
) -> (diag::SourceMap, Result<Output, diag::Diagnostic>) {
    // The passes are recursive. Run them on a thread with a roomy stack, so the
    // parser's nesting limit (not the caller's stack size) is what stops deep code.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ksc".into())
            .stack_size(64 << 20)
            .spawn_scoped(scope, || {
                let mut map = diag::SourceMap::default();
                let out = compile_here(root_path, root_src, files, need_main, cache, &mut map);
                (map, out)
            })
            .expect("cannot start the compiler thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn compile_here(
    root_path: &str,
    root_src: &str,
    files: &dyn modules::Files,
    need_main: bool,
    cache: Option<&Cache>,
    map: &mut diag::SourceMap,
) -> Result<Output, diag::Diagnostic> {
    let units = modules::load(root_path, root_src, files, map)?;
    let headers: Vec<ast::Program> = units.iter().map(|u| modules::header(&u.program)).collect();
    let source = |u: &modules::Unit| &map.files.iter().find(|f| f.base == u.base).expect("every unit's file is in the map").src;
    let hashes: Vec<u64> = units.iter().map(|u| modules::interface_hash(source(u), u.base, &u.program)).collect();
    let mut objects = Vec::new();
    let mut warnings = Vec::new();
    let mut tests = Vec::new();
    let mut compiled = Vec::new();
    for k in 0..units.len() {
        // A library from `lib/` is already compiled.
        if let Some(object) = &units[k].object {
            objects.push(object.clone());
            continue;
        }
        let mine = matches!(units[k].origin, modules::Origin::Root | modules::Origin::Src);
        // What the object is made from: the compiler, the file's text and the
        // public part of every file it reaches.
        let src = source(&units[k]);
        let mut stamp = vec![("$source".to_string(), modules::fnv(src.as_bytes())), ("$main".to_string(), u64::from(k == 0))];
        for u in modules::closure(&units, k).into_iter().skip(1) {
            stamp.push((units[u].info.path.clone(), hashes[u]));
        }
        // (A `$`-name is a made-up main file, never kept.)
        let path = cache.filter(|_| !units[k].info.path.starts_with('$')).map(|c| object_path(&c.dir, &units[k].info.path));
        if let (Some(c), Some(path)) = (cache, &path) {
            stamp.push(("$compiler".to_string(), c.compiler));
            // For `--test`, the program's own files are compiled every time
            // (their tests are not kept in the objects).
            let fresh = std::fs::read(path)
                .ok()
                .and_then(|b| Module::from_bytes(&b).ok())
                .filter(|m| m.link.as_ref().is_some_and(|l| l.stamp == stamp) && (need_main || !mine));
            if let Some(object) = fresh {
                objects.push(object);
                continue;
            }
        }
        let mut unit = compile_unit(&units, &headers, k, need_main && k == 0)?;
        lines(&mut unit.object, map);
        let link = unit.object.link.as_mut().expect("an object");
        link.stamp = stamp;
        link.decls = modules::declarations(src, units[k].base, &units[k].program);
        if let Some(path) = &path {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(path, unit.object.to_bytes()));
            if let Err(e) = written {
                return Err(diag::Diagnostic::new(format!("cannot write {}: {e}", path.display()), diag::Span::default()));
            }
        }
        if !units[k].info.path.starts_with('$') {
            compiled.push(map.file_at(units[k].base).map(|f| f.path.clone()).unwrap_or_default());
        }
        objects.push(unit.object);
        warnings.extend(unit.warnings);
        // (`$all.kstl`, made up to check a whole project, uses every file.)
        if mine && !units[k].info.path.starts_with('$') {
            warnings.extend(unused_uses(src, units[k].base, &units[k].program));
            warnings.extend(docs::unused_private_methods(&units, map, k));
        }
        if mine {
            tests.extend(unit.tests);
        }
    }
    let module = link(&objects, need_main)?;
    let tests = tests.into_iter().map(|n| (n.clone(), function(&module, &n))).collect();
    Ok(Output { module, warnings, tests, compiled })
}

/// `build/Shapes/Circle.kstb` for `Shapes.Circle`.
fn object_path(dir: &std::path::Path, module: &str) -> std::path::PathBuf {
    let mut p = dir.to_path_buf();
    for part in module.split('.') {
        p.push(part);
    }
    p.set_extension("kstb");
    p
}

/// Turns the source positions codegen left in each function's `lines` into
/// its file and line numbers (a function's code all comes from one file).
fn lines(object: &mut Module, map: &diag::SourceMap) {
    // Where each line starts, by file (found once).
    let mut starts: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for f in &mut object.functions {
        let Some(file) = f.lines.first().and_then(|&(_, pos)| map.file_at(pos as usize)) else { continue };
        f.file = file.path.clone();
        let newlines = starts.entry(file.base).or_insert_with(|| file.src.bytes().enumerate().filter(|&(_, b)| b == b'\n').map(|(i, _)| i).collect());
        let mut out: Vec<(u32, u32)> = Vec::new();
        for &(at, pos) in &f.lines {
            let local = (pos as usize).saturating_sub(file.base);
            let line = newlines.partition_point(|&n| n < local) as u32 + 1;
            if out.last().is_none_or(|&(_, l)| l != line) {
                out.push((at, line));
            }
        }
        f.lines = out;
    }
}

/// A warning for each `use a.b.Name;` whose `Name` the file never writes
/// again: its items are always written `Name.item` (or `Name` itself, for a
/// type), so a file that never names it does not need it.
fn unused_uses(src: &str, base: usize, program: &ast::Program) -> Vec<diag::Diagnostic> {
    let Some(tokens) = docs::lex_with_formats(src) else { return Vec::new() };
    let in_use = |at: usize| program.uses.iter().any(|u| u.span.start <= at && at < u.span.end);
    let unused = program
        .uses
        .iter()
        .filter(|u| {
            let alias = &u.alias().name;
            !tokens.iter().any(|t| matches!(&t.tok, lexer::Tok::Ident(n) if n == alias) && !in_use(base + t.span.start))
        })
        .map(|u| {
            diag::Diagnostic::warning(format!("`use {}` is never used", u.dotted()), u.span)
                .with_note(format!("nothing in this file is written `{}`; remove the `use`", u.alias().name))
        });
    unused.collect()
}

/// One file compiled on its own: its object for the linker.
pub struct Compiled {
    pub object: Module,
    pub warnings: Vec<diag::Diagnostic>,
    /// Its `@test` procs.
    pub tests: Vec<String>,
}

/// Compiles file `k` of the program: the files it reaches are read only as
/// declarations (their [`modules::header`]s), as C reads `.h` files.
pub fn compile_unit(units: &[modules::Unit], headers: &[ast::Program], k: usize, need_main: bool) -> Result<Compiled, diag::Diagnostic> {
    let mut program = ast::Program::default();
    // A native proc is only a name for a built-in: calls to it were
    // turned into the built-in already (see `resolve::NATIVES`).
    let compiled = |item: &&ast::Item| !matches!(item, ast::Item::Proc(p) if p.native);
    for u in modules::closure(units, k) {
        if u == k {
            program.items.extend(units[u].program.items.iter().filter(compiled).cloned());
        } else {
            for item in headers[u].items.iter().filter(compiled) {
                program.external.insert(parser::item_name(item).name.clone());
                program.items.push(item.clone());
            }
        }
    }
    let program = object::prelude(program)?;
    let program = generics::expand(program)?;
    let program = object::add_defaults(program)?;
    let program = overload::rename(program)?;
    let hir = sema::check_with(&program, need_main)?;
    throws::check(&hir)?;
    linear::check(&hir)?;
    let object = codegen::generate(&hir)?;
    Ok(Compiled { object, warnings: hir.warnings, tests: hir.tests.into_iter().map(|(n, _)| n).collect() })
}

/// Links objects into a program (the first one has `main`).
fn link(objects: &[Module], need_main: bool) -> Result<Module, diag::Diagnostic> {
    kestrel_core::link::link(objects, need_main.then_some("main"))
        .map_err(|e| diag::Diagnostic::new(e.to_string(), diag::Span::default()))
}

/// The number of the function named `name` in a linked program.
fn function(m: &Module, name: &str) -> u32 {
    m.functions.iter().position(|f| f.name == name).expect("a test is in the program") as u32
}
