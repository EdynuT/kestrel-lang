//! ksc command line: `ksc <program.ks> [--tokens]`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_tokens = args.iter().any(|a| a == "--tokens");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: ksc <program.ks> [--tokens]");
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
    } else {
        eprintln!("ksc: lexed {} tokens; the parser is not written yet (try --tokens)", tokens.len());
    }
    ExitCode::SUCCESS
}
