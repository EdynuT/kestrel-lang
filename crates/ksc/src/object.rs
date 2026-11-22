//! `Object`: every class has `toString`, `hashCode` and `equals`.
//!
//! Runs after [`crate::generics`], before the checker. A class that does not
//! write these gets them here, built as Kestrel code (AST) and compared field by
//! field: `toString` gives `Conta { dono: "Ana", saldo: 10.0 }`, `equals`
//! is true when every field is equal, and `hashCode` mixes the fields'
//! hashes. For each interface in `@impl(...)`, the class also gets
//! `equals$I(&self, other: &I)`, which is what `a.equals(&b)` calls when `a`
//! is seen through the interface `I`: it is true only for an object of the
//! same class with equal fields.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::{lexer, parser};

/// The methods every type has.
pub const METHODS: [&str; 3] = ["toString", "hashCode", "equals"];

/// The interface of things in an order: `compareTo` gives a negative
/// number, 0 or a positive number. Numbers, texts and enums have it; a class
/// gets it with `@impl(Comparable)` and its own `compareTo(&self, other: &C) -> int`.
pub const COMPARABLE: &str = "Comparable";

/// What every thrown error has: `throw` takes an object of a class with
/// `@impl(Error)`, and `catch e: Error` catches any of them.
pub const ERROR: &str = "Error";

/// Declares `Comparable`, `Error` and `Trap` (unless the program has its
/// own `Comparable`), before generics are expanded so `T extends
/// Comparable` finds it. `Trap` is the error a failure of the VM becomes
/// (overflow, division by zero, a bad index...); only the VM makes one.
pub fn prelude(mut program: Program) -> Result<Program, Diagnostic> {
    let mut text = String::new();
    if !program.items.iter().any(|i| matches!(i, Item::Interface(d) if d.name.name == COMPARABLE)) {
        text += "interface Comparable { proc compareTo(&self, other: &Comparable) -> int; }\n";
    }
    text += "interface Error { proc message(&self) -> text; }\n";
    text += "@impl(Error) class Trap { reason: text; @override public proc message(&self) -> text { return self.reason; } }\n";
    let tokens = lexer::lex(&text).map_err(|d| bug(d, &text))?;
    let parsed = parser::parse(tokens).map_err(|d| bug(d, &text))?;
    for mut item in parsed.items {
        // Every file has it: the linker keeps one copy of its code.
        if let Item::Class(c) = &mut item {
            c.instance = true;
        }
        program.items.push(item);
    }
    Ok(program)
}

/// Adds the missing `Object` methods to every class.
pub fn add_defaults(mut program: Program) -> Result<Program, Diagnostic> {
    let interfaces: Vec<String> =
        program.items.iter().filter_map(|i| if let Item::Interface(d) = i { Some(d.name.name.clone()) } else { None }).collect();
    for item in &mut program.items {
        let Item::Class(c) = item else { continue };
        let b = B { sp: c.name.span };
        let class = intern(&c.name.name);
        let has = |n: &str| c.methods.iter().any(|m| m.decl.name.name == n);
        let mut made = Vec::new();
        if !has("toString") {
            made.push(to_string(&b, c));
        }
        if !has("hashCode") {
            made.push(hash_code(&b, c));
        }
        if !has("equals") {
            made.push(equals(&b, c, class));
        }
        let ifaces: Vec<&'static str> =
            c.annotations.iter().filter(|a| a.name.name == "impl").flat_map(|a| a.args.iter().map(|i| intern(&i.name))).collect();
        // Only real interfaces (the checker reports a wrong `@impl` name).
        for i in ifaces.into_iter().filter(|i| interfaces.iter().any(|n| n == i)) {
            // Through `I`, `equals` (and `compareTo` for `Comparable`)
            // takes any `I`: only an object of the same class can match.
            //   if other is C { return self.m(&other); } <otherwise>
            let through = |m: &str, ret: Type, otherwise: Vec<Stmt>| {
                let call = b.method(b.var("self"), m, vec![b.lend(b.var("other"))]);
                let mut body = vec![b.if_(b.is(b.var("other"), class), vec![b.ret(call)], None)];
                body.extend(otherwise);
                b.method_decl(&format!("{m}${i}"), vec![("other", i)], ret, body)
            };
            if i == COMPARABLE {
                let fail = Stmt {
                    kind: StmtKind::Assert { cond: b.bool(false), msg: "assert failed: compareTo: objects of different classes".into() },
                    span: b.sp,
                };
                made.push(through("compareTo", Type::Scalar(Scalar::Int), vec![fail, b.ret(b.num(0))]));
            }
            made.push(through("equals", Type::Bool, vec![b.ret(b.bool(false))]));
        }
        c.methods.extend(made);
    }
    Ok(program)
}

/// An error in the generated code is a bug in the compiler.
fn bug(d: Diagnostic, text: &str) -> Diagnostic {
    Diagnostic::new(format!("internal error in the built-in declarations: {}\n{text}", d.message), Span::default())
}

/// Builds the pieces of the generated methods, every one at the class's
/// name (for messages).
struct B {
    sp: Span,
}

impl B {
    fn ex(&self, kind: ExprKind) -> Expr {
        Expr { kind, span: self.sp }
    }
    fn id(&self, n: &str) -> Ident {
        Ident { name: n.to_string(), span: self.sp }
    }
    fn var(&self, n: &str) -> Expr {
        self.ex(ExprKind::Var(n.to_string()))
    }
    fn num(&self, n: i64) -> Expr {
        self.ex(ExprKind::Num(kestrel_core::Word::new(n)))
    }
    fn text(&self, s: &str) -> Expr {
        self.ex(ExprKind::Str(s.to_string()))
    }
    fn bool(&self, v: bool) -> Expr {
        self.ex(ExprKind::Bool(v))
    }
    fn null(&self) -> Expr {
        self.ex(ExprKind::NaT)
    }
    fn bin(&self, op: BinOp, a: Expr, c: Expr) -> Expr {
        self.ex(ExprKind::Binary(op, Box::new(a), Box::new(c)))
    }
    fn not(&self, a: Expr) -> Expr {
        self.ex(ExprKind::Unary(UnOp::Not, Box::new(a)))
    }
    fn lend(&self, a: Expr) -> Expr {
        self.ex(ExprKind::Lend(Box::new(a)))
    }
    fn is(&self, a: Expr, class: &str) -> Expr {
        self.ex(ExprKind::Is(Box::new(a), self.id(class)))
    }
    /// `obj.f`
    fn field(&self, obj: &str, f: &str) -> Expr {
        self.ex(ExprKind::Member(Box::new(self.var(obj)), self.id(f)))
    }
    fn index(&self, a: Expr, i: Expr) -> Expr {
        self.ex(ExprKind::Index(Box::new(a), Box::new(i)))
    }
    fn call(&self, f: &str, args: Vec<Expr>) -> Expr {
        self.ex(ExprKind::Call { name: self.id(f), args })
    }
    fn method(&self, recv: Expr, m: &str, args: Vec<Expr>) -> Expr {
        self.ex(ExprKind::MethodCall { recv: Box::new(recv), name: self.id(m), args })
    }
    fn st(&self, kind: StmtKind) -> Stmt {
        Stmt { kind, span: self.sp }
    }
    fn block(&self, stmts: Vec<Stmt>) -> Block {
        Block { stmts, span: self.sp }
    }
    fn let_(&self, n: &str, ty: Type, value: Expr) -> Stmt {
        let ty = Some(TypeExpr { ty, span: self.sp });
        self.st(StmtKind::Let { is_final: false, name: self.id(n), ty, value: Some(value), on_nat: None })
    }
    fn set(&self, n: &str, value: Expr) -> Stmt {
        self.st(StmtKind::Assign { target: self.var(n), value })
    }
    /// `s = s + value;`
    fn add_to(&self, s: &str, value: Expr) -> Stmt {
        self.set(s, self.bin(BinOp::Add, self.var(s), value))
    }
    fn ret(&self, e: Expr) -> Stmt {
        self.st(StmtKind::Return(Some(e)))
    }
    fn if_(&self, cond: Expr, then: Vec<Stmt>, otherwise: Option<Vec<Stmt>>) -> Stmt {
        self.st(StmtKind::If { cond, then: self.block(then), otherwise: otherwise.map(|o| self.block(o)) })
    }
    /// `for i: int = 0; i < len(self.f); i++ { body }`
    fn each(&self, f: &str, body: Vec<Stmt>) -> Stmt {
        let init = Box::new(self.let_("i", Type::Scalar(Scalar::Int), self.num(0)));
        let cond = self.bin(BinOp::Lt, self.var("i"), self.call("len", vec![self.field("self", f)]));
        let step = Box::new(self.set("i", self.bin(BinOp::Add, self.var("i"), self.num(1))));
        self.st(StmtKind::For { init, cond, step, body: self.block(body) })
    }
    /// `@override public proc name(&self, p: &T, ...) -> ret { body }`
    fn method_decl(&self, name: &str, params: Vec<(&str, &'static str)>, ret: Type, body: Vec<Stmt>) -> MethodDecl {
        let params = params
            .into_iter()
            .map(|(n, t)| Param { name: self.id(n), ty: TypeExpr { ty: Type::LentNamed(t), span: self.sp }, mode: Some(SelfMode::Lent) })
            .collect();
        let decl = ProcDecl {
            name: self.id(name),
            this: Some(SelfMode::Lent),
            params,
            ret: Some(TypeExpr { ty: ret, span: self.sp }),
            throws: Vec::new(),
            body: self.block(body),
            generics: Vec::new(),
            instance: false,
            annotations: Vec::new(),
            native: false,
        };
        let annotations = vec![Annotation { name: self.id("override"), args: Vec::new(), text: None }];
        MethodDecl { decl, public: true, annotations }
    }
}

/// How a field is shown, compared and hashed.
enum Kind {
    /// A text (shown between quotes).
    Text,
    /// Anything else with the `Object` methods: numbers, bools, enums,
    /// structs, objects, interface values.
    Value,
    /// An array: element by element (`text` elements between quotes).
    Array { text: bool },
}

fn kind(t: Type) -> (Kind, bool) {
    let opt = t.is_opt() || matches!(t, Type::NamedOpt(_));
    let k = match t {
        Type::Text | Type::OptText => Kind::Text,
        Type::Own(b) => Kind::Array { text: b.elem == Elem::Text },
        _ => Kind::Value,
    };
    (k, opt)
}

/// `"\"" + x + "\""`
fn quoted(b: &B, x: Expr) -> Expr {
    b.bin(BinOp::Add, b.bin(BinOp::Add, b.text("\""), x), b.text("\""))
}

/// `Conta { dono: "Ana", saldo: 10.0 }`, field by field.
fn to_string(b: &B, c: &ClassDecl) -> MethodDecl {
    // `Box { ... }` for `Box<int>`, `Circle { ... }` for `shapes.Circle`.
    let short = short_name(&c.name.name);
    let mut body = vec![b.let_("s", Type::Text, b.text(&format!("{short} {{ ")))];
    for (k, f) in c.fields.iter().enumerate() {
        let n = &f.name.name;
        let sep = if k > 0 { ", " } else { "" };
        body.push(b.add_to("s", b.text(&format!("{sep}{n}: "))));
        let (kind, opt) = kind(f.ty.ty);
        let shown = match kind {
            Kind::Text => vec![b.add_to("s", quoted(b, b.field("self", n)))],
            Kind::Value => vec![b.add_to("s", b.method(b.field("self", n), "toString", vec![]))],
            Kind::Array { text } => {
                let item = b.index(b.field("self", n), b.var("i"));
                let elem = if text { quoted(b, item) } else { b.method(item, "toString", vec![]) };
                let comma = b.if_(b.bin(BinOp::Gt, b.var("i"), b.num(0)), vec![b.add_to("s", b.text(", "))], None);
                vec![b.add_to("s", b.text("[")), b.each(n, vec![comma, b.add_to("s", elem)]), b.add_to("s", b.text("]"))]
            }
        };
        if opt {
            let empty = b.bin(BinOp::Eq, b.field("self", n), b.null());
            body.push(b.if_(empty, vec![b.add_to("s", b.text("null"))], Some(shown)));
        } else {
            body.extend(shown);
        }
    }
    body.push(b.ret(b.bin(BinOp::Add, b.var("s"), b.text(" }"))));
    b.method_decl("toString", vec![], Type::Text, body)
}

/// The fields' hashes mixed: `h = hashMix(h, self.f.hashCode())`.
fn hash_code(b: &B, c: &ClassDecl) -> MethodDecl {
    let mut body = vec![b.let_("h", Type::Scalar(Scalar::Int), b.num(17))];
    let mix = |x: Expr| b.set("h", b.call("hashMix", vec![b.var("h"), b.method(x, "hashCode", vec![])]));
    for f in &c.fields {
        let n = &f.name.name;
        let (kind, opt) = kind(f.ty.ty);
        let step = match kind {
            Kind::Text | Kind::Value => mix(b.field("self", n)),
            Kind::Array { .. } => b.each(n, vec![mix(b.index(b.field("self", n), b.var("i")))]),
        };
        body.push(if opt { b.if_(b.bin(BinOp::Ne, b.field("self", n), b.null()), vec![step], None) } else { step });
    }
    body.push(b.ret(b.var("h")));
    b.method_decl("hashCode", vec![], Type::Scalar(Scalar::Int), body)
}

/// True when every field is equal: `if !self.f.equals(&other.f) { return false; }`.
fn equals(b: &B, c: &ClassDecl, class: &'static str) -> MethodDecl {
    let no = || b.ret(b.bool(false));
    let differ = |x: Expr, y: Expr| b.if_(b.not(b.method(x, "equals", vec![b.lend(y)])), vec![no()], None);
    let mut body = Vec::new();
    for f in &c.fields {
        let n = &f.name.name;
        let (kind, opt) = kind(f.ty.ty);
        let test = match kind {
            Kind::Text | Kind::Value => vec![differ(b.field("self", n), b.field("other", n))],
            Kind::Array { .. } => {
                let size = |o: &str| b.call("len", vec![b.field(o, n)]);
                let item = |o: &str| b.index(b.field(o, n), b.var("i"));
                vec![b.if_(b.bin(BinOp::Ne, size("self"), size("other")), vec![no()], None), b.each(n, vec![differ(item("self"), item("other"))])]
            }
        };
        if opt {
            let empty = |o: &str| b.bin(BinOp::Eq, b.field(o, n), b.null());
            let not_empty = b.bin(BinOp::Ne, b.field("other", n), b.null());
            let mut full = vec![b.if_(empty("other"), vec![no()], None)];
            full.extend(test);
            body.push(b.if_(empty("self"), vec![b.if_(not_empty, vec![no()], None)], Some(full)));
        } else {
            body.extend(test);
        }
    }
    body.push(b.ret(b.bool(true)));
    b.method_decl("equals", vec![("other", class)], Type::Bool, body)
}
