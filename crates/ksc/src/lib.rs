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
    let program = parser::parse(lexer::lex(src)?)?;
    let hir = sema::check(&program)?;
    linear::check(&hir)?;
    codegen::generate(&hir)
}
