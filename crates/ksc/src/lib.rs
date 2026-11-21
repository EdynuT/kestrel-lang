//! ksc: the Kestrel compiler (`.ks` -> `.ksb`).
//!
//! Phases: [`lexer`] -> [`parser`] -> [`generics`] (one copy per use) -> [`sema`] (types, HIR) ->
//! [`linear`] (ownership) -> [`codegen`] (bytecode).

pub mod ast;
pub mod codegen;
pub mod diag;
pub mod generics;
pub mod hir;
pub mod lexer;
pub mod linear;
pub mod parser;
pub mod sema;

use kestrel_core::Module;

/// A compiled program with what the compiler found along the way.
pub struct Output {
    pub module: Module,
    /// Warnings (`@deprecated` uses); they do not stop the compilation.
    pub warnings: Vec<diag::Diagnostic>,
    /// `@test` procs: name and function index, to run each as the entry.
    pub tests: Vec<(String, u32)>,
}

/// Runs every phase on a source file.
pub fn compile(src: &str) -> Result<Module, diag::Diagnostic> {
    compile_full(src, true).map(|o| o.module)
}

/// Like [`compile`], also giving the warnings and the tests. With
/// `need_main: false` (for `ksc --test`), a file without `main` is fine.
pub fn compile_full(src: &str, need_main: bool) -> Result<Output, diag::Diagnostic> {
    // The passes are recursive. Run them on a thread with a roomy stack, so the
    // parser's nesting limit (not the caller's stack size) is what stops deep code.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ksc".into())
            .stack_size(64 << 20)
            .spawn_scoped(scope, || compile_here(src, need_main))
            .expect("cannot start the compiler thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn compile_here(src: &str, need_main: bool) -> Result<Output, diag::Diagnostic> {
    let program = parser::parse_source(lexer::lex(src)?, src)?;
    let program = generics::expand(program)?;
    let hir = sema::check_with(&program, need_main)?;
    if hir.procs.is_empty() {
        return Err(diag::Diagnostic::new("the file has no procs", diag::Span::default()));
    }
    linear::check(&hir)?;
    let module = codegen::generate(&hir)?;
    let tests = hir.tests.iter().map(|(n, id)| (n.clone(), *id as u32)).collect();
    Ok(Output { module, warnings: hir.warnings, tests })
}

