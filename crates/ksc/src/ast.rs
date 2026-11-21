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
    Class(ClassDecl),
    Interface(InterfaceDecl),
}

/// `interface Name { proc m(lent self, ...) -> T; ... }`: methods without bodies.
#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceDecl {
    pub name: Ident,
    /// The methods; a default method (`bodies[i]`) has a body, the others none.
    pub methods: Vec<ProcDecl>,
    pub bodies: Vec<bool>,
}

/// `T` or `T extends Forma` in `class Box<T>` / `proc max<T>`.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeParam {
    pub name: Ident,
    /// The interface `T` must implement.
    pub bound: Option<Ident>,
}

/// Generic types written in the program (`Box<int>`, `Map<text, List<int>>`):
/// each one's text names it everywhere (types stay `Copy`), and this table
/// gives back the generic and its arguments.
type GenericTable = std::sync::Mutex<std::collections::HashMap<String, (&'static str, Vec<Type>)>>;

fn generic_table() -> &'static GenericTable {
    static TABLE: std::sync::OnceLock<GenericTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// The name of `base` with these type arguments: `Box<int>`.
pub fn generic_name(base: &str, args: &[Type]) -> &'static str {
    let text = format!("{base}<{}>", args.iter().map(Type::to_string).collect::<Vec<_>>().join(", "));
    let mut table = generic_table().lock().expect("not poisoned");
    if let Some((name, _)) = table.get_key_value(&text) {
        return intern_known(name);
    }
    let name = intern(&text);
    table.insert(text, (intern(base), args.to_vec()));
    name
}

/// `Box<int>` -> (`Box`, [int]); `None` for a name that is not generic.
pub fn generic_parts(name: &str) -> Option<(&'static str, Vec<Type>)> {
    generic_table().lock().expect("not poisoned").get(name).cloned()
}

/// Leaks `s` once per distinct text (generic names are looked up often).
fn intern_known(s: &str) -> &'static str {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> = std::sync::OnceLock::new();
    let mut seen = SEEN.get_or_init(Default::default).lock().expect("not poisoned");
    if let Some(k) = seen.get(s) {
        return k;
    }
    let k = intern(s);
    seen.insert(k);
    k
}

/// `@name` or `@name(A, B)` before a declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub name: Ident,
    pub args: Vec<Ident>,
    /// A string argument: `@deprecated("use x")`.
    pub text: Option<String>,
}

/// `class Name { fields; methods }`: data with methods and an owner.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub name: Ident,
    /// `class Box<T>`: its type parameters (empty for a plain class).
    pub generics: Vec<TypeParam>,
    /// Made from a generic class for one list of types (`Box<int>`).
    pub instance: bool,
    pub annotations: Vec<Annotation>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<MethodDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    pub decl: ProcDecl,
    pub public: bool,
    pub annotations: Vec<Annotation>,
}

/// How a method takes its object: `lent self` (borrows) or `own self` (consumes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfMode {
    Lent,
    Own,
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
    /// A method's `lent self` / `own self` (not in `params`); `None` for
    /// plain procs and static methods.
    pub this: Option<SelfMode>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    /// `proc max<T extends Comparable>`: its type parameters.
    pub generics: Vec<TypeParam>,
    /// Made from a generic proc or class for one list of types: a `give`,
    /// `lend` or `burn` of a value (a `T` that became `int`) just copies it.
    pub instance: bool,
    /// `@test`, `@deprecated(...)` before a top-level proc (a method keeps
    /// its own in `MethodDecl`).
    pub annotations: Vec<Annotation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
    /// `lent` / `own` as written (arrays and objects need one).
    pub mode: Option<SelfMode>,
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

/// Element type of an array: any type that is not itself an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem {
    Scalar(Scalar),
    Bool,
    Text,
    Enum(&'static str),
    Struct(&'static str),
    /// An object: the array owns it, each element is a pointer.
    Class(&'static str),
    /// An object seen through an interface: pointer and class number.
    Iface(&'static str),
    /// A type name as written; the checker resolves it.
    Named(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Scalar(Scalar),
    /// `true` or `false` (stored as +1 / -1, so it fits in a trit).
    Bool,
    Text,
    Own(Buf),
    Lent(Buf),
    /// A declared enum, by name (interned: types stay `Copy`).
    Enum(&'static str),
    /// A declared struct, by name.
    Struct(&'static str),
    /// An object of a class; it has an owner, like an array.
    Class(&'static str),
    /// A borrowed object (`lent c: Conta`, `lent self`).
    LentClass(&'static str),
    /// A type name as written; the checker turns it into `Enum`, `Struct`
    /// or `Class`. Only produced by the parser.
    Named(&'static str),
    /// `lent x: Name` as written; must name a class or an interface.
    LentNamed(&'static str),
    /// A borrowed object seen through an interface (`lent d: Desenhavel`):
    /// its pointer and the number of its class.
    LentIface(&'static str),
    /// An owned object seen through an interface (`f: Forma = Circulo.novo();`).
    Iface(&'static str),
    /// `T?` as written (`proximo: No?`); the checker makes it `OptClass` or
    /// `OptIface`. Only produced by the parser.
    NamedOpt(&'static str),
    /// `lent x: T?` as written. Only produced by the parser.
    LentNamedOpt(&'static str),
    /// An object that may be empty (NaT): `proximo: No?`. It must be checked
    /// (`if !isNaT(x) { ... }`) before it is used.
    OptClass(&'static str),
    /// An interface value that may be empty.
    OptIface(&'static str),
    /// A value that may be null: `int?`, `bool?`, `text?`, `Cor?`. Same
    /// slot as the plain type; null is NaT inside the VM.
    OptScalar(Scalar),
    OptBool,
    OptText,
    OptEnum(&'static str),
    /// A borrowed `T?` (`lent n: No?`).
    LentOptClass(&'static str),
    LentOptIface(&'static str),
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
    /// The type of one element.
    pub fn ty(self) -> Type {
        self.elem.ty()
    }
}

impl std::fmt::Display for Buf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}[]", if self.sym { "sym " } else { "" }, self.elem.ty())
    }
}

impl Elem {
    pub fn ty(self) -> Type {
        match self {
            Elem::Scalar(s) => Type::Scalar(s),
            Elem::Bool => Type::Bool,
            Elem::Text => Type::Text,
            Elem::Enum(n) => Type::Enum(n),
            Elem::Struct(n) => Type::Struct(n),
            Elem::Class(n) => Type::Class(n),
            Elem::Iface(n) => Type::Iface(n),
            Elem::Named(n) => Type::Named(n),
        }
    }

    /// The element type for `ty`; `None` for arrays and `Void`.
    pub fn of(ty: Type) -> Option<Elem> {
        Some(match ty {
            Type::Scalar(s) => Elem::Scalar(s),
            Type::Bool => Elem::Bool,
            Type::Text => Elem::Text,
            Type::Enum(n) => Elem::Enum(n),
            Type::Struct(n) => Elem::Struct(n),
            Type::Class(n) => Elem::Class(n),
            Type::Iface(n) => Elem::Iface(n),
            Type::Named(n) => Elem::Named(n),
            Type::NamedOpt(_)
            | Type::OptScalar(_)
            | Type::OptBool
            | Type::OptText
            | Type::OptEnum(_)
            | Type::LentNamedOpt(_)
            | Type::OptClass(_)
            | Type::OptIface(_)
            | Type::LentOptClass(_)
            | Type::LentOptIface(_)
            | Type::Own(_)
            | Type::Lent(_)
            | Type::Void
            | Type::LentClass(_) | Type::LentNamed(_) | Type::LentIface(_) => {
                return None;
            }
        })
    }
}

impl Type {
    /// What a `T?` holds when it is not empty: `No?` -> `No`, `lent No?` -> `lent No`.
    pub fn unopt(self) -> Type {
        match self {
            Type::OptClass(c) => Type::Class(c),
            Type::OptIface(i) => Type::Iface(i),
            Type::LentOptClass(c) => Type::LentClass(c),
            Type::LentOptIface(i) => Type::LentIface(i),
            Type::OptScalar(s) => Type::Scalar(s),
            Type::OptBool => Type::Bool,
            Type::OptText => Type::Text,
            Type::OptEnum(n) => Type::Enum(n),
            t => t,
        }
    }

    /// `T?` (may be null), owned or lent.
    pub fn is_opt(self) -> bool {
        matches!(
            self,
            Type::OptClass(_)
                | Type::OptIface(_)
                | Type::LentOptClass(_)
                | Type::LentOptIface(_)
                | Type::OptScalar(_)
                | Type::OptBool
                | Type::OptText
                | Type::OptEnum(_)
        )
    }

    /// `T?` for `T`: `int` -> `int?`, `lent No` -> `lent No?`. `None` for
    /// types that cannot be null (arrays, structs, nothing).
    pub fn opt(self) -> Option<Type> {
        Some(match self {
            Type::Scalar(s) => Type::OptScalar(s),
            Type::Bool => Type::OptBool,
            Type::Text => Type::OptText,
            Type::Enum(n) => Type::OptEnum(n),
            Type::Class(c) => Type::OptClass(c),
            Type::Iface(i) => Type::OptIface(i),
            Type::LentClass(c) => Type::LentOptClass(c),
            Type::LentIface(i) => Type::LentOptIface(i),
            t if t.is_opt() => t,
            _ => return None,
        })
    }

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
            Type::Bool => f.write_str("bool"),
            Type::Text => f.write_str("text"),
            Type::Own(b) => write!(f, "{b}"),
            Type::Lent(b) => write!(f, "&{b}"),
            Type::Enum(name) | Type::Struct(name) | Type::Named(name) | Type::Class(name) | Type::Iface(name) => f.write_str(name),
            Type::NamedOpt(name) | Type::OptClass(name) | Type::OptIface(name) | Type::OptEnum(name) => write!(f, "{name}?"),
            Type::OptScalar(s) => write!(f, "{s}?"),
            Type::OptBool => f.write_str("bool?"),
            Type::OptText => f.write_str("text?"),
            Type::LentNamedOpt(name) | Type::LentOptClass(name) | Type::LentOptIface(name) => write!(f, "&{name}?"),
            Type::LentClass(name) | Type::LentNamed(name) | Type::LentIface(name) => write!(f, "&{name}"),
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
    /// `assert(cond);` or `assert(cond, "message");`. `msg` is what a failure
    /// shows: the message, or the condition as written, with its line.
    Assert { cond: Expr, msg: String },
    /// `x: T = e on NaT { ... };` or `x: T;` (no value), optionally after `final`.
    Let {
        is_final: bool,
        name: Ident,
        ty: Option<TypeExpr>,
        /// `None` for `x: int;`: declared without a value (it holds NaT).
        value: Option<Expr>,
        on_nat: Option<Block>,
    },
    /// `target = e;`: target is a variable, `v[i]`, `p.x`, or a chain of them.
    Assign { target: Expr, value: Expr },
    /// `add d, s;`, `add d, a, b;`, `neg d;`, `mov d, s;`
    Burn(Ident),
    Return(Option<Expr>),
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
    /// `(T) x`: T is a number type or `bool`.
    Cast(Box<Expr>, Type),
    /// `true` / `false`
    Bool(bool),
    Index(Box<Expr>, Box<Expr>),
    /// `{1, 2, 3}`: a new array; the declared type says which kind.
    ArrayLit(Vec<Expr>),
    /// `base.name`: an enum value (`Cor.Verde`) or a struct field (`p.x`).
    Member(Box<Expr>, Ident),
    /// `Ponto { x: 1, y: 2 }`
    StructLit { name: Ident, fields: Vec<(Ident, Expr)> },
    /// `a..b` as a call argument (inclusive), as in `replace(v, 0..1, 4..5)`.
    Range(Box<Expr>, Box<Expr>),
    Call { name: Ident, args: Vec<Expr> },
    Claim(Box<Expr>),
    Give(Ident),
    /// `give a.b`: takes the object out of an optional field, leaving it empty.
    GiveField(Box<Expr>),
    /// `a?.rest`: `rest` is a member or method call on the hidden variable
    /// `var`, which holds `a` when it is not null; null otherwise.
    Safe { recv: Box<Expr>, var: String, rest: Box<Expr> },
    /// `f is Circulo`: whether an interface value holds an object of that class.
    Is(Box<Expr>, Ident),
    /// `lend x`, or `lend self.items` (a path to an array or object).
    Lend(Box<Expr>),
    /// `obj.method(args)`, or `Class.method(args)` for a static method.
    MethodCall { recv: Box<Expr>, name: Ident, args: Vec<Expr> },
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
