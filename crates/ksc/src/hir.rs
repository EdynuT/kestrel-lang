//! Typed intermediate tree (HIR) produced by the checker.
//!
//! Every expression carries its type, every variable is a numbered local,
//! built-ins are resolved, and assembly instructions are lowered to plain
//! assignments. The ownership checker and the code generator both read this.

use kestrel_core::{Real, Word};

pub use crate::ast::{BinOp, Elem, Scalar, Type};
use crate::diag::Span;

pub type LocalId = usize;
pub type ProcId = usize;

#[derive(Clone, Debug, PartialEq)]
pub struct Hir {
    pub procs: Vec<HProc>,
    pub entry: ProcId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HProc {
    pub name: String,
    /// Parameters are locals `0..params`.
    pub params: usize,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub body: HBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    Param,
    Let,
    /// The variable of a `walk`; read-only.
    WalkVar,
    /// Compiler temporary (walk bounds).
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub name: String,
    pub ty: Type,
    /// Pinned register `@R-4..@R+4`, if any.
    pub reg: Option<i8>,
    pub kind: LocalKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HBlock {
    pub stmts: Vec<HStmt>,
    /// Locals declared directly in this block (they die at its `}`).
    pub scope: Vec<LocalId>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HStmt {
    pub kind: HStmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HStmtKind {
    Let { local: LocalId, value: HExpr, on_nat: Option<HBlock> },
    /// `x = e` or `b[i] = e`.
    Assign { local: LocalId, index: Option<HExpr>, value: HExpr },
    Burn(LocalId),
    Yield(Option<HExpr>),
    Break,
    Continue,
    /// Arms in order `-`, `0`, `+`.
    Tri { head: HExpr, arms: Box<[HBlock; 3]> },
    /// `walk var in start..end`. Bounds are evaluated once into hidden locals;
    /// for `span k`, `k` is first stored in `span_tmp`.
    Walk {
        var: LocalId,
        span_tmp: Option<(LocalId, HExpr)>,
        start: HExpr,
        end: HExpr,
        end_tmp: LocalId,
        body: HBlock,
    },
    Expr(HExpr),
    Dump(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HExpr {
    pub kind: HExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Show,
    Emit,
    Input,
    Len,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HExprKind {
    Word(Word),
    Real(Real),
    Text(String),
    Local(LocalId),
    Give(LocalId),
    Lend(LocalId),
    /// `-x` / `~x` on an integer or a real.
    Neg(Box<HExpr>),
    /// Operand types pick integer or real ops; an integer result narrower
    /// than a word is narrowed (overflow becomes NaT).
    Binary(BinOp, Box<HExpr>, Box<HExpr>),
    /// Conversion from the inner expression's type to this node's type.
    Convert(Box<HExpr>),
    /// Buffer cell (base is a buffer local) or text character (base is text).
    Index(Box<HExpr>, Box<HExpr>),
    Call(ProcId, Vec<HExpr>),
    Builtin(Builtin, Vec<HExpr>),
    Claim(Box<HExpr>),
}

impl HBlock {
    /// True if control never reaches the end of the block.
    pub fn diverges(&self) -> bool {
        self.stmts.last().is_some_and(|s| match &s.kind {
            HStmtKind::Yield(_) | HStmtKind::Break | HStmtKind::Continue => true,
            HStmtKind::Tri { arms, .. } => arms.iter().all(HBlock::diverges),
            _ => false,
        })
    }
}
