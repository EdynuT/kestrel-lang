//! ksc: the Kestrel compiler (`.ks` -> `.ksb`).
//!
//! Phases: [`lexer`] -> [`parser`] -> [`sema`] (types, HIR) ->
//! [`linear`] (ownership) -> [`codegen`] (bytecode).

pub mod ast;
pub mod codegen;
pub mod diag;
pub mod hir;
pub mod lexer;
pub mod linear;
pub mod parser;
pub mod sema;

use kestrel_core::Module;

/// Runs every phase on a source file.
pub fn compile(src: &str) -> Result<Module, diag::Diagnostic> {
    // The passes are recursive. Run them on a thread with a roomy stack, so the
    // parser's nesting limit (not the caller's stack size) is what stops deep code.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ksc".into())
            .stack_size(64 << 20)
            .spawn_scoped(scope, || compile_here(src))
            .expect("cannot start the compiler thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn compile_here(src: &str) -> Result<Module, diag::Diagnostic> {
    let program = parser::parse(lexer::lex(src)?)?;
    let hir = sema::check(&program)?;
    linear::check(&hir)?;
    codegen::generate(&hir)
}
