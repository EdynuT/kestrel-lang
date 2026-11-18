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
    Own(Elem),
    Lent(Elem),
    /// No value: the result of `print(...)` or of a proc without `->`.
    /// Only produced by the checker, never by the parser.
    Void,
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
            Type::Own(e) => write!(f, "own [{}]", e.scalar()),
            Type::Lent(e) => write!(f, "lent [{}]", e.scalar()),
            Type::Void => f.write_str("no value"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeExpr {
    pub ty: Type,
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
    /// `x: T @R0 = e on NaT { ... };` or `x := e;`
    Let {
        name: Ident,
        ty: Option<TypeExpr>,
        reg: Option<(i8, Span)>,
        value: Expr,
        on_nat: Option<Block>,
    },
    /// `x = e;` or `b[i] = e;`
    Assign { place: Place, value: Expr },
    /// `add d, s;`, `add d, a, b;`, `neg d;`, `mov d, s;`
    Instr { op: InstrOp, op_span: Span, dest: Place, args: Vec<Expr> },
    Burn(Ident),
    Yield(Option<Expr>),
    Break,
    Continue,
    /// Three-way branch. `head` evaluates to a trit.
    Tri { head: Expr, neg: Block, zero: Block, pos: Block },
    Walk { var: Ident, range: Range, body: Block },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: Ident,
    pub index: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Shl3,
    Shr3,
    Neg,
    Mov,
}

impl InstrOp {
    pub fn from_name(name: &str) -> Option<InstrOp> {
        Some(match name {
            "add" => InstrOp::Add,
            "sub" => InstrOp::Sub,
            "mul" => InstrOp::Mul,
            "div" => InstrOp::Div,
            "mod" => InstrOp::Mod,
            "pow" => InstrOp::Pow,
            "shl3" => InstrOp::Shl3,
            "shr3" => InstrOp::Shr3,
            "neg" => InstrOp::Neg,
            "mov" => InstrOp::Mov,
            _ => return None,
        })
    }

    /// The binary operator an arithmetic instruction stands for.
    pub fn bin_op(self) -> Option<BinOp> {
        Some(match self {
            InstrOp::Add => BinOp::Add,
            InstrOp::Sub => BinOp::Sub,
            InstrOp::Mul => BinOp::Mul,
            InstrOp::Div => BinOp::Div,
            InstrOp::Mod => BinOp::Mod,
            InstrOp::Pow => BinOp::Pow,
            InstrOp::Shl3 => BinOp::Shl,
            InstrOp::Shr3 => BinOp::Shr,
            InstrOp::Neg | InstrOp::Mov => return None,
        })
    }
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
    Tilde,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Cmp3,
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
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Cmp3 => "<=>",
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
