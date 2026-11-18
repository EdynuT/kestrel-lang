//! ksc command line.
//!
//! ```text
//! ksc program.ks [-o out.ksb] [--disasm] [--tokens] [--ast]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let out_arg = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    let Some(path) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[i - 1] != "-o")).map(|(_, a)| a)
    else {
        eprintln!("usage: ksc <program.ks> [-o out.ksb] [--disasm] [--tokens] [--ast]");
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

    let module = match ksc::compile(&src) {
        Ok(m) => m,
        Err(d) => return fail(d),
    };
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
