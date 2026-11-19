//! ksvm: runs a `.ksb` file on the BET virtual machine.
//!
//! Exit codes: the value `main` yields (mod 256), 101 on a Trap,
//! 102 if `main` yields NaT, 2 if the file cannot be loaded.

use std::io::{self, BufWriter};
use std::process::ExitCode;

use kestrel_core::Module;
use ksvm::{Value, Vm};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: ksvm <program.ksb> [arguments for the program...]");
        return ExitCode::from(2);
    };
    let program_args = args[2..].to_vec();

    let module = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| {
        Module::from_bytes(&b).map_err(|e| e.to_string())
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ksvm: {path}: {e}");
            return ExitCode::from(2);
        }
    };

    let stdin = io::stdin().lock();
    let stdout = BufWriter::new(io::stdout().lock());
    match Vm::new(&module, stdin, stdout).with_args(program_args).run() {
        Ok(Value::Word(w)) => match w.get() {
            Some(v) => ExitCode::from(v.rem_euclid(256) as u8),
            None => {
                eprintln!("ksvm: main yielded NaT");
                ExitCode::from(102)
            }
        },
        Ok(_) => ExitCode::SUCCESS,
        Err(trap) => {
            eprintln!("\nksvm: {trap}\nksvm: heap and registers were zeroed");
            ExitCode::from(101)
        }
    }
}
