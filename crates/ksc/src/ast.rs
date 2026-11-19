//! Abstract syntax tree produced by the parser.

use kestrel_core::{Real, Word};

use crate::diag::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Const(ConstDecl),
    Proc(ProcDecl),
    Enum(EnumDecl),
    Struct(StructDecl),
}

/// `enum Name { A, B = 5, C }`: values count up from 0, or from the last explicit one.
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<(Ident, Option<Expr>)>,
}

/// `struct Name { a: int; b: text; }`: plain data, copied like a number.
#[derive(Clone, Debug, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<(Ident, TypeExpr)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Trit,
    Tryte,
    Int,
    Real,
}

/// Cell type of a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem {
    Tryte,
    Int,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Scalar(Scalar),
    Text,
    Own(Buf),
    Lent(Buf),
    /// A declared enum, by name (interned: types stay `Copy`).
    Enum(&'static str),
    /// A declared struct, by name.
    Struct(&'static str),
    /// A type name as written; the checker turns it into `Enum` or `Struct`.
    /// Only produced by the parser.
    Named(&'static str),
    /// No value: the result of `print(...)` or of a proc without `->`.
    /// Only produced by the checker, never by the parser.
    Void,
}

/// Leaks `s` so it can live in a `Copy` type. The compiler runs once per
/// process, so the few enum names it leaks do not matter.
pub fn intern(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl Scalar {
    pub fn is_int(self) -> bool {
        self != Scalar::Real
    }

    /// Integer width rank: trit < tryte < int.
    pub fn rank(self) -> u8 {
        match self {
            Scalar::Trit => 0,
            Scalar::Tryte => 1,
            Scalar::Int | Scalar::Real => 2,
        }
    }
}

/// An array type: `int[]` (indices `0..n-1`) or `sym int[]` (`-k..+k`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buf {
    pub elem: Elem,
    pub sym: bool,
}

impl Buf {
    pub fn scalar(self) -> Scalar {
        self.elem.scalar()
    }
}

impl std::fmt::Display for Buf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}[]", if self.sym { "sym " } else { "" }, self.elem.scalar())
    }
}

impl Elem {
    pub fn scalar(self) -> Scalar {
        match self {
            Elem::Tryte => Scalar::Tryte,
            Elem::Int => Scalar::Int,
        }
    }
}

impl Type {
    pub fn is_int(self) -> bool {
        matches!(self, Type::Scalar(s) if s.is_int())
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Scalar(_))
    }
}

impl std::fmt::Display for Scalar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Scalar::Trit => "trit",
            Scalar::Tryte => "tryte",
            Scalar::Int => "int",
            Scalar::Real => "real",
        })
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Scalar(s) => write!(f, "{s}"),
            Type::Text => f.write_str("text"),
            Type::Own(b) => write!(f, "{b}"),
            Type::Lent(b) => write!(f, "lent {b}"),
            Type::Enum(name) | Type::Struct(name) | Type::Named(name) => f.write_str(name),
            Type::Void => f.write_str("no value"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeExpr {
    pub ty: Type,
    pub span: Span,
}

/// One `match` arm; no patterns means `else`.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pats: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// `x: T @R0 = e on NaT { ... };` or `x := e;`, optionally after `final`.
    Let {
        is_final: bool,
        name: Ident,
        ty: Option<TypeExpr>,
        value: Expr,
        on_nat: Option<Block>,
    },
    /// `x = e;` or `b[i] = e;`
    Assign { place: Place, value: Expr },
    /// `add d, s;`, `add d, a, b;`, `neg d;`, `mov d, s;`
    Burn(Ident),
    Yield(Option<Expr>),
    Break,
    Continue,
    /// Three-way branch. `head` evaluates to a trit.
    Tri { head: Expr, neg: Block, zero: Block, pos: Block },
    /// `if cond { } else { }`; `else if` is an `else` block holding one `If`.
    If { cond: Expr, then: Block, otherwise: Option<Block> },
    Walk { var: Ident, range: Range, body: Block },
    /// `match e { A | B => ..., else => ... }`
    Match { scrut: Expr, arms: Vec<MatchArm> },
    /// `while cond { }`
    While { cond: Expr, body: Block },
    /// `for init; cond; step { }`, like Java's `for` without parentheses.
    For { init: Box<Stmt>, cond: Expr, step: Box<Stmt>, body: Block },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: Ident,
    pub index: Option<Expr>,
    /// `p.a.b = ...`: the fields after the name (and index), outermost first.
    pub fields: Vec<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Range {
    /// `a..b`, inclusive.
    Between(Expr, Expr),
    /// `span k`, the same as `-k..k`.
    Span(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    /// Integer literal; its type (trit, tryte, word) comes from context.
    Num(Word),
    Real(Real),
    /// Character literal, as its tryte value.
    Char(i64),
    Str(String),
    NaT,
    Var(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, Scalar),
    Index(Box<Expr>, Box<Expr>),
    /// `{1, 2, 3}`: a new array; the declared type says which kind.
    ArrayLit(Vec<Expr>),
    /// `base.name`: an enum value (`Cor.Verde`) or a struct field (`p.x`).
    Member(Box<Expr>, Ident),
    /// `Ponto { x: 1, y: 2 }`
    StructLit { name: Ident, fields: Vec<(Ident, Expr)> },
    Call { name: Ident, args: Vec<Expr> },
    Claim(Box<Expr>),
    Give(Ident),
    Lend(Ident),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-x`
    Neg,
    /// `~x` (same value as `-x` in balanced ternary)
    /// `!x`: flips a trit (Kleene not).
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Cmp3,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    Pow,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Cmp3 => "<=>",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Pow => "**",
        }
    }
}
