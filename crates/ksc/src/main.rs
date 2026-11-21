//! ksc command line.
//!
//! A project is a folder with `src/` (the code; `src/Main.ks` has `main`),
//! optionally `lib/` (libraries: `.ksb` files) and, made by `ksc`, `build/`:
//!
//! ```text
//! ksc new Nome        create the project Nome/ (src/Main.ks, .gitignore)
//! ksc build [--ksb]   build/obj/: one object per file; build/Nome: the executable
//! ksc run [args...]   build, then run it
//! ksc test            run every @test proc of src/
//! ```
//!
//! Without `src/Main.ks` the project is a library: `ksc build` compiles
//! every file of `src/`, and the objects in `build/obj/` go into another
//! project's `lib/`.
//!
//! One file, gcc style (the files it uses are read from its folder):
//!
//! ```text
//! ksc File.ks [-o out] [--ksb] [--disasm] [--tokens] [--ast] [--test]
//! ```
//!
//! Each file becomes an object (`.ksb`) in `build/obj/`, compiled again
//! only when it or the public part of a file it uses changed; then the
//! objects are linked into the executable, a copy of `ksvm` with the
//! program inside (it runs without installing anything). `--ksb`, or `-o`
//! with a name ending in `.ksb`, writes the program as a `.ksb` instead, to
//! run with `ksvm`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ksc::modules::Disk;

const USAGE: &str = "usage: ksc new <Name>          create a project
       ksc build [--ksb]         build the project in this folder
       ksc run [args...]         build and run it
       ksc test                  run its @test procs
       ksc <File.ks> [-o out] [--ksb] [--disasm] [--tokens] [--ast] [--test]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("new") => new_project(&args[1..]),
        Some("build") => project(&args[1..], Action::Build),
        Some("run") => project(&args[1..], Action::Run),
        Some("test") => project(&args[1..], Action::Test),
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
        .and_then(|()| std::fs::write(root.join("src/Main.ks"), "proc main() {\n    printl(\"Hello, Kestrel!\");\n}\n"))
        .and_then(|()| std::fs::write(root.join(".gitignore"), "build/\n"));
    if let Err(e) = made {
        eprintln!("ksc: cannot create {name}: {e}");
        return ExitCode::from(2);
    }
    eprintln!("ksc: created {name}/ (src/Main.ks); run it with `cd {name}` and `ksc run`");
    ExitCode::SUCCESS
}

/// `ksc build`, `ksc run`, `ksc test` in the project folder.
fn project(args: &[String], action: Action) -> ExitCode {
    let here = std::env::current_dir().unwrap_or_default();
    // Paths are relative to the project folder (the current one), as shown.
    let root = PathBuf::new();
    let src = root.join("src");
    if !src.is_dir() {
        eprintln!("ksc: no src/ folder here\nksc: create a project with `ksc new Name`, or compile one file with `ksc File.ks`");
        return ExitCode::from(2);
    }
    let name = here.file_name().and_then(|n| n.to_str()).unwrap_or("program").to_string();
    let disk = Disk::project(&root);
    let cache = cache(&root);
    let main = src.join("Main.ks");
    let flags = Flags::new(args);
    if action == Action::Test || !main.is_file() {
        if action == Action::Run {
            eprintln!("ksc: no src/Main.ks: this project is a library, there is nothing to run");
            return ExitCode::from(2);
        }
        // Every file of src/: a made-up main file uses them all.
        let mut files = Vec::new();
        sources(&src, &src, &mut files);
        files.sort();
        let text: String = files.iter().map(|f| format!("use {f};\n")).collect();
        let Some(output) = compile("$all.ks", &text, &disk, false, &cache) else {
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
    let out = PathBuf::from("build").join(if ksb { format!("{name}.ksb") } else { format!("{name}{}", std::env::consts::EXE_SUFFIX) });
    if !write_program(&out, &output, ksb) {
        return ExitCode::from(2);
    }
    if action == Action::Build {
        return ExitCode::SUCCESS;
    }
    run(&output.module, flags.rest)
}

/// The `.ks` files under `dir`, as `use` paths (`jogo.Mapa`).
fn sources(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(base, &p, out);
        } else if p.extension().is_some_and(|x| x == "ks") {
            let rel = p.strip_prefix(base).expect("under src/").with_extension("");
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            out.push(parts.join("."));
        }
    }
}

/// `ksc File.ks ...`
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
    // `File.ks` -> the executable `File`; `--ksb` / `-o x.ksb` -> a `.ksb`.
    let ksb = flag("--ksb") || out_arg.is_some_and(|o| o.ends_with(".ksb"));
    let out = match out_arg {
        Some(o) => PathBuf::from(o),
        None if ksb => PathBuf::from(path).with_extension("ksb"),
        None => PathBuf::from(path).with_extension(std::env::consts::EXE_EXTENSION),
    };
    if write_program(&out, &output, ksb) { ExitCode::SUCCESS } else { ExitCode::from(2) }
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
        let ksb = args.first().is_some_and(|a| a == "--ksb");
        let rest = args.iter().skip_while(|a| *a == "--ksb" || *a == "--").cloned().collect();
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

/// Writes the program: an executable (a copy of `ksvm` with it inside) or a `.ksb`.
fn write_program(out: &Path, output: &ksc::Output, ksb: bool) -> bool {
    let program = output.module.to_bytes();
    let bytes = if ksb {
        program
    } else {
        match runtime() {
            Ok(vm) => kestrel_core::ksb::bundle(&vm, &program),
            Err(e) => {
                eprintln!("ksc: {e}\nksc: to write a `.ksb` for ksvm instead, use --ksb");
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
    std::fs::read(&vm).map_err(|e| format!("cannot read {} (an executable is a copy of it): {e}", vm.display()))
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
            Err(trap) => Some(trap.reason),
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
