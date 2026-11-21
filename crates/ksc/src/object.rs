//! `Object`: every class has `toString`, `hashCode` and `equals`.
//!
//! Runs after [`crate::generics`], before the checker. A class that does not
//! write these gets them here, written in Kestrel and compared field by
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

/// Declares `Comparable` (unless the program has its own), before generics
/// are expanded so `T extends Comparable` finds it.
pub fn prelude(mut program: Program) -> Result<Program, Diagnostic> {
    if program.items.iter().any(|i| matches!(i, Item::Interface(d) if d.name.name == COMPARABLE)) {
        return Ok(program);
    }
    let text = "interface Comparable { proc compareTo(&self, other: &Comparable) -> int; }";
    let tokens = lexer::lex(text).map_err(|d| bug(d, text))?;
    let parsed = parser::parse(tokens).map_err(|d| bug(d, text))?;
    program.items.extend(parsed.items);
    Ok(program)
}

/// Stands for the class's name in the generated code (a generic class's
/// name, like `Box<int>`, is not a plain name), replaced after parsing.
const SELF: &str = "Self__";

/// Stands for the interface in `equals$I` (its name may be `Geometry.Forma`).
const IFACE: &str = "Iface__";

/// Adds the missing `Object` methods to every class.
pub fn add_defaults(mut program: Program) -> Result<Program, Diagnostic> {
    let interfaces: Vec<String> =
        program.items.iter().filter_map(|i| if let Item::Interface(d) = i { Some(d.name.name.clone()) } else { None }).collect();
    for item in &mut program.items {
        let Item::Class(c) = item else { continue };
        let has = |n: &str| c.methods.iter().any(|m| m.decl.name.name == n);
        let mut src = String::new();
        if !has("toString") {
            src += &to_string(c);
        }
        if !has("hashCode") {
            src += &hash_code(c);
        }
        if !has("equals") {
            src += &equals(c);
        }
        let ifaces: Vec<String> =
            c.annotations.iter().filter(|a| a.name.name == "impl").flat_map(|a| a.args.iter().map(|i| i.name.clone())).collect();
        let mut made = parse_methods(&src, &c.name.name, c.name.span, None)?;
        // Only real interfaces (the checker reports a wrong `@impl` name).
        for i in ifaces.iter().filter(|i| interfaces.contains(i)) {
            let mut src = String::new();
            // Through `Comparable`, `compareTo` compares with any `Comparable`:
            // only objects of the same class have an order.
            if i == COMPARABLE {
                src += &format!(
                    "@override public proc compareToAs__(&self, other: &{IFACE}) -> int {{ if other is {SELF} {{ return self.compareTo(&other); }} assert(false, \"compareTo: objects of different classes\"); return 0; }}\n"
                );
            }
            src += &format!(
                "@override public proc equalsAs__(&self, other: &{IFACE}) -> bool {{ if other is {SELF} {{ return self.equals(&other); }} return false; }}\n"
            );
            made.extend(parse_methods(&src, &c.name.name, c.name.span, Some(i))?);
        }
        c.methods.extend(made);
    }
    Ok(program)
}

/// The generated methods, parsed as the body of a class named [`SELF`] and
/// given the real class name.
fn parse_methods(src: &str, class: &str, span: Span, iface: Option<&str>) -> Result<Vec<MethodDecl>, Diagnostic> {
    if src.is_empty() {
        return Ok(Vec::new());
    }
    let text = format!("class {SELF} {{ {src} }}");
    let tokens = lexer::lex(&text).map_err(|d| bug(d, &text))?;
    let program = parser::parse(tokens).map_err(|d| bug(d, &text))?;
    let Some(Item::Class(c)) = program.items.into_iter().next() else { unreachable!("one class was written") };
    let mut methods = c.methods;
    for m in &mut methods {
        // The generated code points at the class, for messages.
        m.decl.name.span = span;
        if let (Some(base), Some(i)) = (m.decl.name.name.strip_suffix("As__"), iface) {
            m.decl.name.name = format!("{base}${i}");
        }
        for p in &mut m.decl.params {
            p.name.span = span;
            p.ty.span = span;
            if p.ty.ty == Type::LentNamed(SELF) {
                p.ty.ty = Type::LentNamed(intern(class));
            }
            if let (Type::LentNamed(IFACE), Some(i)) = (p.ty.ty, iface) {
                p.ty.ty = Type::LentNamed(intern(i));
            }
        }
        if let Some(r) = &mut m.decl.ret {
            r.span = span;
        }
        rename_block(&mut m.decl.body, class, span);
    }
    Ok(methods)
}

/// Replaces [`SELF`] in `x is Self__` and moves every span to the class.
fn rename_block(b: &mut Block, class: &str, span: Span) {
    b.span = span;
    for s in &mut b.stmts {
        s.span = span;
        match &mut s.kind {
            StmtKind::If { cond, then, otherwise } => {
                rename_expr(cond, class, span);
                rename_block(then, class, span);
                if let Some(o) = otherwise {
                    rename_block(o, class, span);
                }
            }
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => rename_expr(e, class, span),
            StmtKind::Let { value: Some(e), ty, .. } => {
                if let Some(t) = ty {
                    t.span = span;
                }
                rename_expr(e, class, span);
            }
            StmtKind::Assign { target, value } => {
                rename_expr(target, class, span);
                rename_expr(value, class, span);
            }
            StmtKind::For { init, cond, step, body } => {
                rename_block(&mut Block { stmts: vec![(**init).clone()], span }, class, span);
                init.span = span;
                step.span = span;
                rename_expr(cond, class, span);
                rename_block(body, class, span);
            }
            _ => {}
        }
    }
}

fn rename_expr(e: &mut Expr, class: &str, span: Span) {
    e.span = span;
    match &mut e.kind {
        ExprKind::Is(x, c) => {
            rename_expr(x, class, span);
            if c.name == SELF {
                c.name = class.to_string();
            }
            c.span = span;
        }
        ExprKind::Unary(_, a) | ExprKind::Lend(a) => rename_expr(a, class, span),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => {
            rename_expr(a, class, span);
            rename_expr(b, class, span);
        }
        ExprKind::Member(a, f) => {
            rename_expr(a, class, span);
            f.span = span;
        }
        ExprKind::MethodCall { recv, name, args } => {
            rename_expr(recv, class, span);
            name.span = span;
            args.iter_mut().for_each(|a| rename_expr(a, class, span));
        }
        ExprKind::Call { name, args } => {
            name.span = span;
            args.iter_mut().for_each(|a| rename_expr(a, class, span));
        }
        _ => {}
    }
}

/// An error in the generated code is a bug in the compiler.
fn bug(d: Diagnostic, text: &str) -> Diagnostic {
    Diagnostic::new(format!("internal error in the generated Object methods: {}\n{text}", d.message), Span::default())
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

fn to_string(c: &ClassDecl) -> String {
    // `Box { ... }` for `Box<int>`, `Circle { ... }` for `Shapes.Circle`.
    let base = c.name.name.split('<').next().unwrap_or_default();
    let short = base.rsplit('.').next().unwrap_or(base).to_string();
    let mut body = format!("s: text = \"{short} {{ \";\n");
    for (i, f) in c.fields.iter().enumerate() {
        let n = &f.name.name;
        let sep = if i > 0 { ", " } else { "" };
        body += &format!("s = s + \"{sep}{n}: \";\n");
        let (k, opt) = kind(f.ty.ty);
        let shown = match k {
            Kind::Text => format!("s = s + \"\\\"\" + self.{n} + \"\\\"\";"),
            Kind::Value => format!("s = s + self.{n}.toString();"),
            Kind::Array { text } => {
                let elem = if text { format!("\"\\\"\" + self.{n}[i] + \"\\\"\"") } else { format!("self.{n}[i].toString()") };
                format!(
                    "s = s + \"[\"; for i: int = 0; i < len(self.{n}); i++ {{ if i > 0 {{ s = s + \", \"; }} s = s + {elem}; }} s = s + \"]\";"
                )
            }
        };
        body += &if opt { format!("if self.{n} == null {{ s = s + \"null\"; }} else {{ {shown} }}\n") } else { format!("{shown}\n") };
    }
    body += "return s + \" }\";\n";
    format!("@override public proc toString(&self) -> text {{\n{body}}}\n")
}

fn hash_code(c: &ClassDecl) -> String {
    let mut body = String::from("h: int = 17;\n");
    for f in &c.fields {
        let n = &f.name.name;
        let (k, opt) = kind(f.ty.ty);
        let mix = match k {
            Kind::Text | Kind::Value => format!("h = hashMix(h, self.{n}.hashCode());"),
            Kind::Array { .. } => format!("for i: int = 0; i < len(self.{n}); i++ {{ h = hashMix(h, self.{n}[i].hashCode()); }}"),
        };
        body += &if opt { format!("if self.{n} != null {{ {mix} }}\n") } else { format!("{mix}\n") };
    }
    body += "return h;\n";
    format!("@override public proc hashCode(&self) -> int {{\n{body}}}\n")
}

fn equals(c: &ClassDecl) -> String {
    let mut body = String::new();
    for f in &c.fields {
        let n = &f.name.name;
        let (k, opt) = kind(f.ty.ty);
        let test = match k {
            Kind::Text | Kind::Value => format!("if !self.{n}.equals(&other.{n}) {{ return false; }}"),
            Kind::Array { .. } => format!(
                "if len(self.{n}) != len(other.{n}) {{ return false; }} for i: int = 0; i < len(self.{n}); i++ {{ if !self.{n}[i].equals(&other.{n}[i]) {{ return false; }} }}"
            ),
        };
        body += &if opt {
            format!(
                "if self.{n} == null {{ if other.{n} != null {{ return false; }} }} else {{ if other.{n} == null {{ return false; }} {test} }}\n"
            )
        } else {
            format!("{test}\n")
        };
    }
    body += "return true;\n";
    format!("@override public proc equals(&self, other: &{SELF}) -> bool {{\n{body}}}\n")
}
