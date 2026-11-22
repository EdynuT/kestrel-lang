//! ksvm: runs a `.ksb` file on the BET virtual machine. An executable made
//! by `ksc` is a copy of ksvm with the program at its end ([`kestrel_core::ksb::bundle`]).
//!
//! Exit codes: the value `main` returns (mod 256), 101 on a Trap,
//! 102 if `main` returns NaT, 2 if the file cannot be loaded.

use std::io::{self, BufWriter};
use std::process::ExitCode;

use kestrel_core::Module;
use ksvm::Vm;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // An executable made by `ksc build`: native code (see `ksvm::native`).
    if let Some(code) = ksvm::native::run_if_present(args[1..].to_vec()) {
        return ExitCode::from(code);
    }
    // An executable made by `ksc` is this program with a `.ksb` at its end:
    // then it runs that, and every argument is the program's.
    let me = std::env::current_exe().and_then(std::fs::read).unwrap_or_default();
    let (path, bytes, program_args) = match kestrel_core::ksb::bundled(&me) {
        Some(program) => (args[0].clone(), Ok(program.to_vec()), args[1..].to_vec()),
        None => {
            let Some(path) = args.get(1) else {
                eprintln!("usage: ksvm <program.ksb> [arguments for the program...]");
                return ExitCode::from(2);
            };
            (path.clone(), std::fs::read(path).map_err(|e| e.to_string()), args[2..].to_vec())
        }
    };

    let module = match bytes.and_then(|b| Module::from_bytes(&b).map_err(|e| e.to_string())) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ksvm: {path}: {e}");
            return ExitCode::from(2);
        }
    };

    let stdin = io::stdin().lock();
    let stdout = BufWriter::new(io::stdout().lock());
    ExitCode::from(ksvm::exit_code(Vm::new(&module, stdin, stdout).with_args(program_args).run()))
}
