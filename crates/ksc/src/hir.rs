//! Typed intermediate tree (HIR) produced by the checker.
//!
//! Every expression carries its type, every variable is a numbered local,
//! built-ins are resolved, and assembly instructions are lowered to plain
//! assignments. The ownership checker and the code generator both read this.

use std::collections::HashMap;

use kestrel_core::{Real, Word};

pub use crate::ast::{BinOp, Elem, Scalar, Type};
use crate::diag::Span;

pub type LocalId = usize;
pub type ProcId = usize;

#[derive(Clone, Debug, PartialEq)]
pub struct Hir {
    pub procs: Vec<HProc>,
    pub entry: ProcId,
    /// Fields of each struct, in declared order.
    pub structs: Structs,
    /// Fields of each class, in declared order (`burn` frees what they own).
    pub classes: Structs,
    /// Every class: its number, where its code is, and the interfaces it
    /// implements (for the linker's interface calls and frees).
    pub class_info: Vec<ClassLink>,
    /// Every interface's name.
    pub iface_names: Vec<&'static str>,
    /// How the classes of this file answer interface calls: the dispatch
    /// proc's name (`$dispatch.I.m`), the class number, the proc that runs,
    /// and whether it takes the receiver whole (a default method).
    pub impls: Vec<(String, i64, ProcId, bool)>,
    /// `@test` procs, in the order they are written.
    pub tests: Vec<(String, ProcId)>,
    /// Uses of `@deprecated` things.
    pub warnings: Vec<crate::diag::Diagnostic>,
    /// For each proc that returns a borrow (`-> &T`): which of its
    /// parameters the result borrows from (the first `&` one).
    pub borrows: Vec<Option<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassLink {
    pub name: &'static str,
    pub tag: i64,
    pub link: Link,
    pub ifaces: Vec<&'static str>,
}

/// Where a proc's code comes from (see `kestrel_core::ksb::Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Written in the file being compiled.
    Own,
    /// A copy of a generic: every file that uses it compiles it, the
    /// linker keeps one.
    Shared,
    /// Written in another file: only its signature is here.
    Import,
    /// `$dispatch.I.m`, an interface call; the linker writes it.
    Dispatch,
}

/// Struct layouts. A struct value is its fields side by side, nested
/// structs flattened: `struct A { x: int; b: B; }` with `B { y; z }` takes
/// 3 slots (`x`, `b.y`, `b.z`).
pub type Structs = HashMap<&'static str, Vec<(String, Type)>>;

/// How many slots (or stack values) a value of type `ty` takes.
pub fn width(structs: &Structs, ty: Type) -> usize {
    match ty {
        Type::Struct(n) => structs[n].iter().map(|(_, t)| width(structs, *t)).sum(),
        Type::Void => 0,
        // An interface value: the object's pointer and its class number.
        Type::LentIface(_) | Type::Iface(_) | Type::OptIface(_) | Type::LentOptIface(_) => 2,
        _ => 1,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HProc {
    pub name: String,
    /// Parameters are locals `0..params`.
    pub params: usize,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub body: HBlock,
    /// For each parameter: whether it is `x: &int` (a value the caller
    /// passed with `&n`; its final value is written back to `n`).
    pub refs: Vec<bool>,
    pub link: Link,
    /// The errors it declares (`throws`): class names, or `Error` for any.
    pub throws: Vec<&'static str>,
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
    pub kind: LocalKind,
    pub span: Span,
    /// Declared with `final`: it can never be assigned again.
    pub is_final: bool,
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
    /// `x: T;`: the local holds NaT (no value) until it is assigned.
    Declare(LocalId),
    /// `x = e`, `b[i] = e`, or a field: `p.x = e` writes at slot offset `field`.
    Assign { local: LocalId, index: Option<HExpr>, field: usize, value: HExpr },
    /// A cell of an array that is not in a local (one held in a field):
    /// `arr` gives the pointer.
    SetElem { arr: HExpr, index: HExpr, field: usize, value: HExpr },
    Burn(LocalId),
    Return(Option<HExpr>),
    Break,
    Continue,
    /// Arms in order `-`, `0`, `+`.
    /// `origin` says which statement this came from, for error messages:
    /// `if`/`else` and `match` lower to a `tri` (arms: else, trap, then).
    Tri { head: HExpr, arms: Box<[HBlock; 3]>, origin: Branch },
    /// A nested block (used by `match`, which stores its value first).
    Block(HBlock),
    /// `while cond { body }`: `-` leaves, `0` traps, `+` runs the body again.
    /// `step` runs after the body and on `continue` (from a `for`).
    While { cond: HExpr, body: HBlock, step: Option<HBlock> },
    /// Stop the program with a trap carrying this reason.
    Trap(String),
    /// `exit(code);`: stop the program normally with this exit code.
    Exit(HExpr),
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
    /// `throw e;`: `value` is the error seen as an `Error` (pointer and class
    /// number); `class` is its class, `None` when it is only known as an `Error`.
    Throw { value: HExpr, class: Option<&'static str> },
    /// `try { body } catch ...`: an error inside `body` frees what `body`
    /// owns or changed (see [`changed_in`]), then goes to the first catch
    /// that takes it, or on to whoever handles it outside.
    Try { body: HBlock, catches: Vec<HCatch> },
}

/// `catch e: C { body }`: `class` is `None` for `catch e: Error` (any error).
#[derive(Clone, Debug, PartialEq)]
pub struct HCatch {
    pub local: LocalId,
    pub class: Option<(&'static str, i64)>,
    pub body: HBlock,
    pub span: Span,
}

/// The locals a block declares, or gives away, burns or assigns: what may
/// own something (or nothing any more) when an error leaves it midway.
pub fn changed_in(b: &HBlock, out: &mut Vec<LocalId>) {
    fn add(out: &mut Vec<LocalId>, id: LocalId) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    fn expr(e: &HExpr, out: &mut Vec<LocalId>) {
        match &e.kind {
            HExprKind::Give(id) => add(out, *id),
            HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_) | HExprKind::Local(_) | HExprKind::Lend(_) | HExprKind::Field(..) => {}
            HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) | HExprKind::ToIface(a, _) | HExprKind::TakeField(a, _) | HExprKind::IsClass(a, ..) => {
                expr(a, out)
            }
            HExprKind::Binary(_, a, c) | HExprKind::Index(a, c) | HExprKind::ElemField(a, c, _) | HExprKind::Bind(_, a, c) => {
                expr(a, out);
                expr(c, out);
            }
            HExprKind::Call(_, xs) | HExprKind::Builtin(_, xs) | HExprKind::Pack(xs) | HExprKind::StructLit(xs) | HExprKind::NewObj(xs) => {
                xs.iter().for_each(|x| expr(x, out))
            }
            HExprKind::Print(ps) => ps.iter().for_each(|(_, x)| expr(x, out)),
            HExprKind::IfNull { value, then, null, .. } => {
                expr(value, out);
                expr(then, out);
                if let Some(n) = null {
                    expr(n, out);
                }
            }
        }
    }
    for id in &b.scope {
        add(out, *id);
    }
    for s in &b.stmts {
        match &s.kind {
            HStmtKind::Let { local, value, on_nat } => {
                add(out, *local);
                expr(value, out);
                if let Some(x) = on_nat {
                    changed_in(x, out);
                }
            }
            HStmtKind::Declare(id) | HStmtKind::Burn(id) => add(out, *id),
            HStmtKind::Assign { local, index, value, .. } => {
                if index.is_none() {
                    add(out, *local);
                }
                if let Some(i) = index {
                    expr(i, out);
                }
                expr(value, out);
            }
            HStmtKind::SetElem { arr, index, value, .. } => {
                expr(arr, out);
                expr(index, out);
                expr(value, out);
            }
            HStmtKind::Return(v) => v.iter().for_each(|x| expr(x, out)),
            HStmtKind::Break | HStmtKind::Continue | HStmtKind::Trap(_) | HStmtKind::Dump(_) => {}
            HStmtKind::Tri { head, arms, .. } => {
                expr(head, out);
                arms.iter().for_each(|x| changed_in(x, out));
            }
            HStmtKind::Block(x) => changed_in(x, out),
            HStmtKind::While { cond, body, step } => {
                expr(cond, out);
                changed_in(body, out);
                if let Some(x) = step {
                    changed_in(x, out);
                }
            }
            HStmtKind::Exit(x) | HStmtKind::Expr(x) => expr(x, out),
            HStmtKind::Walk { span_tmp, start, end, body, .. } => {
                if let Some((_, k)) = span_tmp {
                    expr(k, out);
                }
                expr(start, out);
                expr(end, out);
                changed_in(body, out);
            }
            HStmtKind::Throw { value, .. } => expr(value, out),
            HStmtKind::Try { body, catches } => {
                changed_in(body, out);
                for c in catches {
                    add(out, c.local);
                    changed_in(&c.body, out);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HExpr {
    pub kind: HExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Emit,
    Input,
    Len,
    /// `toText(x)`, `toText(x, 't')`, `toText(x, 'v')`
    ToText(Fmt),
    /// `sub(s, start, end)`: characters `start..end-1`, like Java's `substring`.
    Sub,
    ToInt,
    ToReal,
    /// `isDigit(c)` etc.: index into `charset::Class::ALL`.
    CharIs(u8),
    ReadFile,
    FileExists,
    WriteFile,
    WriteBytes,
    ReadLine,
    Eof,
    ArgCount,
    Arg,
    /// `isNaT(x)`: whether `x` has no value.
    IsNaT,
    /// `x.hashCode()` of a number, bool, enum or text.
    Hash,
    /// Order of two texts: -1, 0 or 1.
    TCmp,
    /// `hashMix(h, x)`: `h * 31 + x`, wrapped into the `int` range.
    HashMix,
    /// Arrays that change size; the first argument is the (lent) array.
    Append,
    Pop,
    Insert,
    Remove,
    Resize,
    Clear,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    Tri,
    If,
    Match,
}

/// How one `print`/`printf` piece is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    /// Text as is, numbers in decimal (`{x}`, `%d`, `%f`, `%s`).
    Plain,
    /// `{x:t}` / `%t`
    Trits,
    /// `{x:v}` / `%v`
    Hepta,
    /// `{x:c}` / `%c`
    Char,
    /// A text field inside a printed struct or object: `"Ana"` in quotes,
    /// or `NaT` (no quotes) when it has no value.
    Quoted,
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
    /// `print` / `printf`, already split into pieces printed in order.
    Print(Vec<(Fmt, HExpr)>),
    /// `claim n`: the node's type says symmetric or from 0.
    Claim(Box<HExpr>),
    /// `{a, b, c}`: a new array holding these values.
    Pack(Vec<HExpr>),
    /// `Ponto { x: 1, y: 2 }`: field values in declared order.
    StructLit(Vec<HExpr>),
    /// A new object (`Conta { ... }` inside its class): field values in
    /// declared order, stored in a heap block of one element.
    NewObj(Vec<HExpr>),
    /// A field of a struct local: the slots from this offset on (as many as
    /// the node's type takes).
    Field(LocalId, usize),
    /// Stores the value in a hidden local, then evaluates the body (used to
    /// read fields of a struct that is not in a variable, like `f().x`).
    Bind(LocalId, Box<HExpr>, Box<HExpr>),
    /// An object seen through an interface: its pointer (the inner
    /// expression) and its class number.
    ToIface(Box<HExpr>, i64),
    /// A field of an array element, `pts[i].x`: the array (a pointer),
    /// index, offset of the field inside the element.
    ElemField(Box<HExpr>, Box<HExpr>, usize),
    /// `give n.campo`: the value of a `T?` field of the object (a pointer),
    /// at that offset; the field is left empty (NaT).
    TakeField(Box<HExpr>, usize),
    /// `f is Circulo`: an interface value (pointer and class number), the
    /// class's number and its name. False when it is empty.
    IsClass(Box<HExpr>, i64, &'static str),
    /// `value` stored in the hidden local `tmp`; if it is null the result is
    /// `null` (or NaT of the node's type when `None`), otherwise `then`
    /// (which reads `tmp`, now known not null). For `a?.b` and printing `T?`.
    IfNull { value: Box<HExpr>, tmp: LocalId, then: Box<HExpr>, null: Option<Box<HExpr>> },
}

impl HBlock {
    /// True if control never reaches the end of the block.
    pub fn diverges(&self) -> bool {
        self.stmts.last().is_some_and(|s| match &s.kind {
            HStmtKind::Return(_) | HStmtKind::Break | HStmtKind::Continue | HStmtKind::Trap(_) | HStmtKind::Exit(_) => true,
            HStmtKind::Throw { .. } => true,
            HStmtKind::Tri { arms, .. } => arms.iter().all(HBlock::diverges),
            HStmtKind::Try { body, catches } => body.diverges() && catches.iter().all(|c| c.body.diverges()),
            HStmtKind::Block(b) => b.diverges(),
            _ => false,
        })
    }
}
