//! ksc command line.
//!
//! A project is a folder with `src/` (the code; `src/Main.kstl` has `main`),
//! optionally `lib/` (libraries: `.kstb` files) and, made by `ksc`, `build/`:
//!
//! ```text
//! ksc new Nome        create the project Nome/ (src/Main.kstl, .gitignore)
//! ksc build [--kstb]   build/obj/: one object per file; build/Nome: the executable
//! ksc run [args...]   build, then run it
//! ksc test            run every @test proc of src/
//! ```
//!
//! Without `src/Main.kstl` the project is a library: `ksc build` compiles
//! every file of `src/`, and the objects in `build/obj/` go into another
//! project's `lib/`.
//!
//! One file, gcc style (the files it uses are read from its folder):
//!
//! ```text
//! ksc File.kstl [-o out] [--kstb] [--disasm] [--tokens] [--ast] [--test]
//! ```
//!
//! Each file becomes an object (`.kstb`) in `build/obj/`, compiled again
//! only when it or the public part of a file it uses changed; then the
//! objects are linked into the executable, a copy of `ksvm` with the
//! program inside (it runs without installing anything). `--kstb`, or `-o`
//! with a name ending in `.kstb`, writes the program as a `.kstb` instead, to
//! run with `ksvm`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ksc::modules::Disk;

const USAGE: &str = "usage: ksc new <Name>          create a project
       ksc build [--kstb]         build the project in this folder
       ksc run [args...]         build and run it
       ksc test                  run its @test procs
       ksc check <File.kstl> [--stdin]  only report errors (for editors)
       ksc doc <File.kstl> --at <byte> [--stdin]  the documentation of the name there
       ksc std <collections/List.kstl>  print a file of the standard library
       ksc nav <File.kstl> (--at <byte> | --id <id>) --what <definition|implementations|supers> [--stdin]
       ksc <File.kstl> [-o out] [--kstb] [--disasm] [--tokens] [--ast] [--test]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("new") => new_project(&args[1..]),
        Some("build") => project(&args[1..], Action::Build),
        Some("run") => project(&args[1..], Action::Run),
        Some("test") => project(&args[1..], Action::Test),
        Some("check") => check(&args[1..]),
        Some("doc") => doc(&args[1..]),
        Some("std") => std_file(&args[1..]),
        Some("nav") => nav(&args[1..]),
        Some(_) => file(&args),
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Build,
    Run,
    Test,
}

/// `ksc new Nome`
fn new_project(args: &[String]) -> ExitCode {
    let Some(name) = args.first() else {
        eprintln!("usage: ksc new <Name>");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("ksc: {name} already exists");
        return ExitCode::from(2);
    }
    let made = std::fs::create_dir_all(root.join("src"))
        .and_then(|()| std::fs::write(root.join("src/Main.kstl"), "use std.io.Console;\n\nproc main() {\n    Console.printl(\"Hello, Kestrel!\");\n}\n"))
        .and_then(|()| std::fs::write(root.join(".gitignore"), "build/\n"));
    if let Err(e) = made {
        eprintln!("ksc: cannot create {name}: {e}");
        return ExitCode::from(2);
    }
    eprintln!("ksc: created {name}/ (src/Main.kstl); run it with `cd {name}` and `ksc run`");
    ExitCode::SUCCESS
}

/// `ksc build`, `ksc run`, `ksc test` in the project folder.
fn project(args: &[String], action: Action) -> ExitCode {
    let here = std::env::current_dir().unwrap_or_default();
    // Paths are relative to the project folder (the current one), as shown.
    let root = PathBuf::new();
    let src = root.join("src");
    if !src.is_dir() {
        eprintln!("ksc: no src/ folder here\nksc: create a project with `ksc new Name`, or compile one file with `ksc File.kstl`");
        return ExitCode::from(2);
    }
    let name = here.file_name().and_then(|n| n.to_str()).unwrap_or("program").to_string();
    let disk = Disk::project(&root);
    let cache = cache(&root);
    let main = src.join("Main.kstl");
    let flags = Flags::new(args);
    if action == Action::Test || !main.is_file() {
        if action == Action::Run {
            eprintln!("ksc: no src/Main.kstl: this project is a library, there is nothing to run");
            return ExitCode::from(2);
        }
        // Every file of src/: a made-up main file uses them all.
        let mut files = Vec::new();
        sources(&src, &src, &mut files);
        files.sort();
        let text: String = files.iter().map(|f| format!("use {f};\n")).collect();
        let Some(output) = compile("$all.kstl", &text, &disk, false, &cache) else {
            return ExitCode::FAILURE;
        };
        if action == Action::Test {
            return run_tests(&output);
        }
        eprintln!("ksc: library: {} compiled; objects in build/obj/ (copy them into another project's lib/)", count(&output));
        return ExitCode::SUCCESS;
    }
    let text = match std::fs::read_to_string(&main) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ksc: {}: {e}", main.display());
            return ExitCode::from(2);
        }
    };
    let shown = main.strip_prefix(&root).unwrap_or(&main).display().to_string();
    let Some(output) = compile(&shown, &text, &disk, true, &cache) else {
        return ExitCode::FAILURE;
    };
    let ksb = flags.ksb;
    let out = PathBuf::from("build").join(if ksb { format!("{name}.kstb") } else { format!("{name}{}", std::env::consts::EXE_SUFFIX) });
    if !write_program(&out, &output, ksb) {
        return ExitCode::from(2);
    }
    if action == Action::Build {
        return ExitCode::SUCCESS;
    }
    run(&output.module, flags.rest)
}

/// The `.kstl` files under `dir`, as `use` paths (`jogo.Mapa`).
fn sources(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(base, &p, out);
        } else if p.extension().is_some_and(|x| x == "kstl") {
            let rel = p.strip_prefix(base).expect("under src/").with_extension("");
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            out.push(parts.join("."));
        }
    }
}

/// `ksc File.kstl ...`
fn file(args: &[String]) -> ExitCode {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let out_arg = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    let Some(path) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "-o")).map(|(_, a)| a)
    else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ksc: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    if flag("--tokens") || flag("--ast") {
        let tokens = match ksc::lexer::lex(&src) {
            Ok(t) => t,
            Err(d) => {
                eprint!("{}", d.render(path, &src));
                return ExitCode::FAILURE;
            }
        };
        if flag("--tokens") {
            for t in &tokens {
                println!("{:>5}..{:<5} {}", t.span.start, t.span.end, t.tok);
            }
        }
        if flag("--ast") {
            match ksc::parser::parse(tokens) {
                Ok(p) => println!("{p:#?}"),
                Err(d) => {
                    eprint!("{}", d.render(path, &src));
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    let testing = flag("--test");
    // The files it uses are read from its folder (and `lib/` there).
    let dir = Path::new(path).parent().map(PathBuf::from).unwrap_or_default();
    let disk = Disk { src: dir.clone(), lib: Some(dir.join("lib")) };
    let Some(output) = compile(path, &src, &disk, !testing, &cache(&dir)) else {
        return ExitCode::FAILURE;
    };
    if testing {
        return run_tests(&output);
    }
    if flag("--disasm") {
        print!("{}", output.module.disassemble());
    }
    // `File.kstl` -> the executable `File`; `--kstb` / `-o x.kstb` -> a `.kstb`.
    let ksb = flag("--kstb") || out_arg.is_some_and(|o| o.ends_with(".kstb"));
    let out = match out_arg {
        Some(o) => PathBuf::from(o),
        None if ksb => PathBuf::from(path).with_extension("kstb"),
        None => PathBuf::from(path).with_extension(std::env::consts::EXE_EXTENSION),
    };
    if write_program(&out, &output, ksb) { ExitCode::SUCCESS } else { ExitCode::from(2) }
}

/// `ksc check File.kstl [--stdin]`: the errors of a file, for an editor.
/// Nothing is written and `main` is not needed. With `--stdin` the file's
/// text (maybe not saved) comes from standard input; its `use`s are read
/// from where the file is. A file under a project's `src/` is checked with
/// the whole project, so files that use each other work.
fn check(args: &[String]) -> ExitCode {
    let Some(target) = editor_target(args, "usage: ksc check <File.kstl> [--stdin]") else {
        return ExitCode::from(2);
    };
    let (map, result) = ksc::build(&target.root_path, &target.root_src, target.files.as_ref(), false, None);
    match result {
        Ok(o) => {
            for w in &o.warnings {
                eprint!("{}", map.render(w));
            }
            ExitCode::SUCCESS
        }
        Err(d) => {
            eprint!("{}", map.render(&d));
            ExitCode::FAILURE
        }
    }
}

/// `ksc doc File.kstl --at N [--stdin]`: the documentation (Markdown) of the
/// name at byte `N` of the file, for an editor's hover. Nothing if none.
fn doc(args: &[String]) -> ExitCode {
    let Some(target) = editor_target(args, "usage: ksc doc <File.kstl> --at <byte> [--stdin]") else {
        return ExitCode::from(2);
    };
    let at = args.iter().position(|a| a == "--at").and_then(|i| args.get(i + 1)).and_then(|n| n.parse::<usize>().ok());
    let Some(at) = at else {
        eprintln!("usage: ksc doc <File.kstl> --at <byte> [--stdin]");
        return ExitCode::from(2);
    };
    let mut map = ksc::diag::SourceMap::default();
    let Ok(units) = ksc::modules::load(&target.root_path, &target.root_src, target.files.as_ref(), &mut map) else {
        return ExitCode::FAILURE;
    };
    let me = match &target.module {
        Some(m) => units.iter().position(|u| u.info.path == m.join(".")),
        None => Some(0),
    };
    match me.and_then(|me| ksc::docs::hover(&units, &map, me, at)) {
        Some(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// `ksc nav File.kstl --at N --what definition [--stdin]`: places for an
/// editor, one per line: `kind<TAB>name<TAB>file<TAB>line<TAB>column<TAB>id`.
/// `--what` is `definition`, `implementations` (of an interface or its
/// method) or `supers` (the interfaces of a class, or the interface method
/// a method fills in). `--id` (from an earlier answer) names the item
/// instead of a position.
fn nav(args: &[String]) -> ExitCode {
    const USAGE: &str = "usage: ksc nav <File.kstl> (--at <byte> | --id <id>) --what <definition|implementations|supers> [--stdin]";
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let rest: Vec<String> = {
        let mut out = Vec::new();
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if matches!(a.as_str(), "--what" | "--id") {
                skip = true;
            } else {
                out.push(a.clone());
            }
        }
        out
    };
    let Some(target) = editor_target(&rest, USAGE) else {
        return ExitCode::from(2);
    };
    let mut map = ksc::diag::SourceMap::default();
    let Ok(units) = ksc::modules::load(&target.root_path, &target.root_src, target.files.as_ref(), &mut map) else {
        return ExitCode::FAILURE;
    };
    let found: Vec<ksc::docs::Target> = match (value("--id"), value("--at").and_then(|n| n.parse::<usize>().ok())) {
        (Some(id), _) => ksc::nav::by_id(&units, &id).into_iter().collect(),
        (None, Some(at)) => {
            let me = match &target.module {
                Some(m) => units.iter().position(|u| u.info.path == m.join(".")),
                None => Some(0),
            };
            me.map(|me| ksc::docs::targets(&units, &map, me, at)).unwrap_or_default()
        }
        (None, None) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let places: Vec<ksc::docs::Target> = match value("--what").as_deref() {
        Some("definition") => found,
        Some("implementations") => found.iter().flat_map(|t| ksc::nav::implementations(&units, t)).collect(),
        Some("supers") => found.iter().flat_map(|t| ksc::nav::supers(&units, t)).collect(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut seen = std::collections::HashSet::new();
    for p in places.iter().filter_map(|t| ksc::nav::place(&units, &map, t)).filter(|p| seen.insert(p.id.clone())) {
        println!("{}\t{}\t{}\t{}\t{}\t{}", p.kind.word(), p.name, p.file, p.line, p.column, p.id);
    }
    ExitCode::SUCCESS
}

/// `ksc std collections/List.kstl`: a file of the standard library (built
/// into `ksc`), for an editor to show.
fn std_file(args: &[String]) -> ExitCode {
    let key = args.first().map(|a| a.trim_start_matches("std/").trim_end_matches(".kstl").replace('/', "."));
    match key.as_deref().and_then(ksc::modules::std_source) {
        Some(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("usage: ksc std <collections/List.kstl> (a file of the standard library)");
            ExitCode::from(2)
        }
    }
}

/// What an editor command works on: the program to load (a whole project
/// when the file is under `src/`), and which file of it is the edited one.
struct EditorTarget {
    root_path: String,
    root_src: String,
    files: Box<dyn ksc::modules::Files>,
    /// The edited file's `use` path, in a project (`None`: it is the root).
    module: Option<Vec<String>>,
}

fn editor_target(args: &[String], usage: &str) -> Option<EditorTarget> {
    let Some(path) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "--at")).map(|(_, a)| a)
    else {
        eprintln!("{usage}");
        return None;
    };
    let text = if args.iter().any(|a| a == "--stdin") {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map(|_| s)
    } else {
        std::fs::read_to_string(path)
    };
    let text = match text {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ksc: {path}: {e}");
            return None;
        }
    };
    let file = PathBuf::from(path);
    // A file of the standard library (`.../std/io/Console.kstl`): checked as
    // `use std.io.Console;` with this text, so its native procs are known.
    let canonical = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
    let in_std = canonical.ancestors().skip(1).find(|d| d.file_name().is_some_and(|n| n == "std")).map(|std| {
        let rel = canonical.strip_prefix(std).expect("under std/").with_extension("");
        std::iter::once("std".to_string()).chain(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned())).collect::<Vec<_>>()
    });
    if let Some(module) = in_std.filter(|m| ksc::modules::std_source(&m[1..].join(".")).is_some() || m.len() > 2) {
        let files = Edited { disk: Disk { src: PathBuf::new(), lib: None }, module: module.clone(), text, shown: path.clone() };
        let root_src = format!("use {};\n", module.join("."));
        return Some(EditorTarget { root_path: "$all.kstl".into(), root_src, files: Box::new(files), module: Some(module) });
    }
    // In a project: the folder holding the `src/` the file is under.
    let project = file.ancestors().skip(1).find(|d| d.file_name().is_some_and(|n| n == "src")).map(|src| {
        let rel = file.strip_prefix(src).expect("under src/").with_extension("");
        let module: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        (src.parent().map(Path::to_path_buf).unwrap_or_default(), module)
    });
    Some(match project {
        Some((root, module)) => {
            let files = Edited { disk: Disk::project(&root), module: module.clone(), text, shown: path.clone() };
            let mut all = Vec::new();
            sources(&files.disk.src, &files.disk.src, &mut all);
            all.sort();
            let uses: String = all.iter().map(|f| format!("use {f};\n")).collect();
            EditorTarget { root_path: "$all.kstl".into(), root_src: uses, files: Box::new(files), module: Some(module) }
        }
        None => {
            let dir = file.parent().map(PathBuf::from).unwrap_or_default();
            let files = Disk { src: dir.clone(), lib: Some(dir.join("lib")) };
            EditorTarget { root_path: path.clone(), root_src: text, files: Box::new(files), module: None }
        }
    })
}

/// A project's files, with one of them (`module`) as the editor has it.
struct Edited {
    disk: Disk,
    module: Vec<String>,
    text: String,
    /// How the edited file is shown in messages (its path).
    shown: String,
}

impl ksc::modules::Files for Edited {
    fn std_file(&self, path: &[String]) -> Option<ksc::modules::Found> {
        (path == self.module.as_slice()).then(|| ksc::modules::Found { shown: self.shown.clone(), src: self.text.clone(), object: None })
    }

    fn read(&self, path: &[String]) -> Result<Option<ksc::modules::Found>, String> {
        let found = self.disk.read(path)?;
        if path != self.module.as_slice() {
            return Ok(found);
        }
        let shown = found.map_or_else(|| self.disk.src.join(self.module.join("/")).with_extension("kstl").display().to_string(), |f| f.shown);
        Ok(Some(ksc::modules::Found { shown, src: self.text.clone(), object: None }))
    }
}

/// `build/obj/` under `dir`.
fn cache(dir: &Path) -> ksc::Cache {
    ksc::Cache { dir: dir.join("build").join("obj"), compiler: compiler_id() }
}

struct Flags {
    ksb: bool,
    /// The program's arguments (`ksc run a b`).
    rest: Vec<String>,
}

impl Flags {
    fn new(args: &[String]) -> Flags {
        let ksb = args.first().is_some_and(|a| a == "--kstb");
        let rest = args.iter().skip_while(|a| *a == "--kstb" || *a == "--").cloned().collect();
        Flags { ksb, rest }
    }
}

/// Compiles the program whose main file is `path`, printing the errors and warnings.
fn compile(path: &str, src: &str, disk: &Disk, need_main: bool, cache: &ksc::Cache) -> Option<ksc::Output> {
    let (map, output) = ksc::build(path, src, disk, need_main, Some(cache));
    match output {
        Ok(o) => {
            for w in &o.warnings {
                eprint!("{}", map.render(w));
            }
            Some(o)
        }
        Err(d) => {
            eprint!("{}", map.render(&d));
            None
        }
    }
}

fn count(output: &ksc::Output) -> String {
    match output.compiled.len() {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

/// Writes the program: an executable (a copy of `ksvm` with it inside) or a `.kstb`.
fn write_program(out: &Path, output: &ksc::Output, ksb: bool) -> bool {
    let program = output.module.to_bytes();
    let bytes = if ksb {
        program
    } else {
        // Machine code, in a copy of the runtime (see `ksc::native`).
        match runtime().and_then(|vm| ksc::native::build(&output.module, &vm)) {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("ksc: {e}\nksc: to write a `.kstb` for ksvm instead, use --kstb");
                return false;
            }
        }
    };
    let written = out.parent().filter(|p| !p.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
        std::fs::write(out, &bytes)?;
        #[cfg(unix)]
        if !ksb {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    });
    if let Err(e) = written {
        eprintln!("ksc: cannot write {}: {e}", out.display());
        return false;
    }
    eprintln!("ksc: wrote {} ({} bytes, {} compiled)", out.display(), bytes.len(), count(output));
    true
}

/// Runs the program here, as `ksvm` would (same exit codes).
fn run(module: &kestrel_core::Module, args: Vec<String>) -> ExitCode {
    use std::io::Write;
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    let result = ksvm::Vm::new(module, stdin, &mut stdout).with_args(args).run();
    let _ = stdout.flush();
    drop(stdout);
    match result {
        Ok(ksvm::Value::Word(w)) => match w.get() {
            Some(v) => ExitCode::from(v.rem_euclid(256) as u8),
            None => {
                eprintln!("ksc: main yielded NaT");
                ExitCode::from(102)
            }
        },
        Ok(_) => ExitCode::SUCCESS,
        Err(trap) => {
            eprintln!("\nksc: {trap}");
            ExitCode::from(101)
        }
    }
}

/// The `ksvm` next to this `ksc`: an executable is a copy of it.
fn runtime() -> Result<Vec<u8>, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let vm = me.with_file_name(format!("ksvm{}", std::env::consts::EXE_SUFFIX));
    std::fs::read(&vm).map_err(|e| format!("cannot read {} (the runtime of every executable): {e}", vm.display()))
}

/// Tells this build of `ksc` from others, so `build/` objects made by
/// another one are compiled again.
fn compiler_id() -> u64 {
    let meta = std::env::current_exe().and_then(std::fs::metadata);
    let text = match meta {
        Ok(m) => format!("{} {:?} {}", m.len(), m.modified().ok(), env!("CARGO_PKG_VERSION")),
        Err(_) => env!("CARGO_PKG_VERSION").to_string(),
    };
    ksc::modules::fnv(text.as_bytes())
}

/// Runs each `@test` proc as the entry of its own VM. A test passes if it
/// ends normally; a Trap (a failed `assert`, for one) or `exit(n)` with
/// `n != 0` fails it. What a test prints is shown only when it fails.
fn run_tests(output: &ksc::Output) -> ExitCode {
    if output.tests.is_empty() {
        println!("no tests: mark procs with `@test`");
        return ExitCode::SUCCESS;
    }
    let s = if output.tests.len() == 1 { "" } else { "s" };
    println!("running {} test{s}", output.tests.len());
    let mut failed = Vec::new();
    for (name, id) in &output.tests {
        let mut module = output.module.clone();
        module.entry = *id;
        let mut printed = Vec::new();
        let result = ksvm::Vm::new(&module, std::io::empty(), &mut printed).run();
        let why = match result {
            Ok(ksvm::Value::Word(w)) if w.get().is_some_and(|c| c != 0) => Some(format!("exit({})", w.get().unwrap_or(0))),
            Ok(_) => None,
            Err(trap) => Some(trap.to_string()),
        };
        match why {
            None => println!("test {name} ... ok"),
            Some(why) => {
                println!("test {name} ... FAILED");
                failed.push((name, why, String::from_utf8_lossy(&printed).into_owned()));
            }
        }
    }
    for (name, why, printed) in &failed {
        println!("\n---- {name} ----");
        if !printed.is_empty() {
            print!("{printed}");
            if !printed.ends_with('\n') {
                println!();
            }
        }
        println!("{why}");
    }
    let passed = output.tests.len() - failed.len();
    println!("\ntest result: {passed} passed, {} failed", failed.len());
    if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
