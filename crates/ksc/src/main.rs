//! ksc command line: `ksc <program.ks> [--tokens] [--ast]`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_tokens = args.iter().any(|a| a == "--tokens");
    let show_ast = args.iter().any(|a| a == "--ast");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: ksc <program.ks> [--tokens] [--ast]");
        return ExitCode::from(2);
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ksc: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let tokens = match ksc::lexer::lex(&src) {
        Ok(t) => t,
        Err(d) => {
            eprint!("{}", d.render(path, &src));
            return ExitCode::FAILURE;
        }
    };
    if show_tokens {
        for t in &tokens {
            println!("{:>5}..{:<5} {}", t.span.start, t.span.end, t.tok);
        }
    }
    let program = match ksc::parser::parse(tokens) {
        Ok(p) => p,
        Err(d) => {
            eprint!("{}", d.render(path, &src));
            return ExitCode::FAILURE;
        }
    };
    if show_ast {
        println!("{program:#?}");
    } else {
        eprintln!("ksc: parsed {} item(s); type checking is not written yet (try --ast)", program.items.len());
    }
    ExitCode::SUCCESS
}
