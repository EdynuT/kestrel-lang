//! ksc command line.
//!
//! ```text
//! ksc program.ks [-o out.ksb] [--disasm] [--tokens] [--ast]
//! ksc --test program.ks
//! ```
//!
//! `--test` compiles the file and runs each `@test` proc on the VM, without
//! writing a `.ksb`. Exit code 1 if a test fails.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let out_arg = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    let Some(path) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "-o")).map(|(_, a)| a)
    else {
        eprintln!("usage: ksc <program.ks> [-o out.ksb] [--disasm] [--tokens] [--ast]\n       ksc --test <program.ks>");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ksc: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let fail = |d: ksc::diag::Diagnostic| {
        eprint!("{}", d.render(path, &src));
        ExitCode::FAILURE
    };

    if flag("--tokens") || flag("--ast") {
        let tokens = match ksc::lexer::lex(&src) {
            Ok(t) => t,
            Err(d) => return fail(d),
        };
        if flag("--tokens") {
            for t in &tokens {
                println!("{:>5}..{:<5} {}", t.span.start, t.span.end, t.tok);
            }
        }
        if flag("--ast") {
            match ksc::parser::parse(tokens) {
                Ok(p) => println!("{p:#?}"),
                Err(d) => return fail(d),
            }
        }
    }

    let testing = flag("--test");
    let output = match ksc::compile_full(&src, !testing) {
        Ok(o) => o,
        Err(d) => return fail(d),
    };
    for w in &output.warnings {
        eprint!("{}", w.render(path, &src));
    }
    if testing {
        return run_tests(&output);
    }
    let module = output.module;
    if flag("--disasm") {
        print!("{}", module.disassemble());
    }
    let out = out_arg.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(path).with_extension("ksb"));
    let bytes = module.to_bytes();
    if let Err(e) = std::fs::write(&out, &bytes) {
        eprintln!("ksc: cannot write {}: {e}", out.display());
        return ExitCode::from(2);
    }
    eprintln!("ksc: wrote {} ({} bytes)", out.display(), bytes.len());
    ExitCode::SUCCESS
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
