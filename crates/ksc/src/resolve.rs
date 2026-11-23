//! Names across files: `use Geometry;` and `Geometry.area(2.0)`.
//!
//! Runs on each file right after it is parsed. Every top-level item of a
//! file that is not the program's main file gets its full name: `area` in
//! `Geometry.kstl` becomes `Geometry.area`, in its own file and in every file
//! that uses it, so names from different files never clash and the later
//! passes see one flat program. The item named like its file (`List` in
//! `List.kstl`) is the file's main item and is just `List`. Items of the main
//! file keep their names (no file can use it, as with Go's `package main`).
//!
//! An item another file reads must be `public`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::diag::{Diagnostic, Span};

type R<T> = Result<T, Diagnostic>;

/// What kind of thing a top-level name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Proc,
    /// A native proc: the built-in that does its work.
    Native(&'static str),
    Const,
    /// An enum, struct, class or interface.
    Type,
}

/// The native procs of the standard library: file, proc, and the built-in
/// (an instruction of the runtime) that does its work.
pub const NATIVES: &[(&str, &str, &str)] = &[
    ("std.io.Console", "print", "print"),
    ("std.io.Console", "printl", "printl"),
    ("std.io.Console", "printf", "printf"),
    ("std.io.Console", "readLine", "readLine"),
    ("std.io.Console", "eof", "eof"),
    ("std.io.Console", "input", "input"),
    ("std.io.Console", "emit", "emit"),
    ("std.io.Files", "read", "readFile"),
    ("std.io.Files", "write", "writeFile"),
    ("std.io.Files", "writeBytes", "writeBytes"),
    ("std.io.Files", "exists", "fileExists"),
    ("std.os.Process", "argCount", "argCount"),
    ("std.os.Process", "arg", "arg"),
    ("std.os.Process", "exit", "exit"),
    ("std.text.Texts", "sub", "sub"),
    ("std.text.Texts", "toInt", "toInt"),
    ("std.text.Texts", "toReal", "toReal"),
    ("std.text.Texts", "toText", "toText"),
    ("std.text.Chars", "isDigit", "isDigit"),
    ("std.text.Chars", "isLetter", "isLetter"),
    ("std.text.Chars", "isUpper", "isUpper"),
    ("std.text.Chars", "isLower", "isLower"),
    ("std.text.Chars", "isSpace", "isSpace"),
    ("std.debug.Debug", "dump", "dump"),
    ("std.debug.Debug", "hashMix", "hashMix"),
];

/// The top-level names of one file.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// `Shapes.Circle` (for `Shapes/Circle.kstl`); the file name for the main file.
    pub path: String,
    /// The program's main file: its names stay as written.
    pub root: bool,
    pub items: HashMap<String, Kind>,
    pub public: HashSet<String>,
}

impl ModuleInfo {
    pub fn new(path: String, root: bool, program: &Program) -> R<ModuleInfo> {
        let mut items = HashMap::new();
        let mut generic_procs = HashSet::new();
        for item in &program.items {
            let kind = match item {
                Item::Proc(p) if p.native => {
                    let found = NATIVES.iter().find(|(file, name, _)| *file == path && *name == p.name.name);
                    match found {
                        Some((_, _, builtin)) => Kind::Native(builtin),
                        None if !path.starts_with("std.") => {
                            return Err(Diagnostic::new("only the standard library has native procs", p.name.span)
                                .with_note("write the proc's body in Kestrel"));
                        }
                        None => {
                            let known: Vec<String> = NATIVES.iter().filter(|(file, _, _)| *file == path).map(|(_, n, _)| format!("`{n}`")).collect();
                            let note = if known.is_empty() {
                                format!("`{path}` has no native procs: write the proc's body in Kestrel")
                            } else {
                                format!("the native procs of `{path}` are {}; anything else is written in Kestrel", known.join(", "))
                            };
                            return Err(Diagnostic::new(format!("the runtime does nothing called `{}` in `{path}`", p.name.name), p.name.span)
                                .with_note(note));
                        }
                    }
                }
                Item::Proc(_) => Kind::Proc,
                Item::Const(_) => Kind::Const,
                _ => Kind::Type,
            };
            let name = crate::parser::item_name(item);
            // Procs may share a name (overloads); anything else may not, and
            // neither may a generic proc (`max<int>(...)` names one already).
            let generic = matches!(item, Item::Proc(p) if !p.generics.is_empty());
            let twice = items.get(&name.name).is_some_and(|&before| before == Kind::Proc && kind == Kind::Proc);
            if twice && (generic || generic_procs.contains(&name.name)) {
                return Err(Diagnostic::new(format!("generic proc `{}` cannot be overloaded", name.name), name.span)
                    .with_note("give the other procs another name"));
            }
            if generic {
                generic_procs.insert(name.name.clone());
            }
            if matches!(kind, Kind::Native(_)) && items.contains_key(&name.name) {
                return Err(Diagnostic::new(format!("native proc `{}` cannot be overloaded", name.name), name.span)
                    .with_note("each native proc is one thing the runtime does; one that takes any type ends with `values: ...`"));
            }
            if items.insert(name.name.clone(), kind).is_some_and(|before| !(before == Kind::Proc && kind == Kind::Proc)) {
                return Err(Diagnostic::new(format!("`{}` is defined twice in this file", name.name), name.span));
            }
        }
        let info = ModuleInfo { path, root, items, public: program.public.clone() };
        info.check_main_item(program)?;
        Ok(info)
    }

    /// `Circle` for `Shapes.Circle`: how other files write this file's items.
    pub fn alias(&self) -> &str {
        self.path.rsplit('.').next().unwrap_or(&self.path)
    }

    /// The full name of one of this file's items.
    pub fn mangle(&self, name: &str) -> String {
        if self.root {
            name.to_string()
        } else if name == self.alias() {
            self.path.clone()
        } else {
            format!("{}.{name}", self.path)
        }
    }

    /// The item named like the file, if any (`class List` in `List.kstl`).
    fn main_item(&self) -> Option<&str> {
        (!self.root && self.items.contains_key(self.alias())).then(|| self.alias())
    }

    /// `List.empty()` must mean one thing: with a main item `List`, no
    /// other item of the file may share a name with its methods or values.
    fn check_main_item(&self, program: &Program) -> R<()> {
        let Some(main) = self.main_item() else { return Ok(()) };
        let mut members: Vec<&Ident> = Vec::new();
        for item in &program.items {
            match item {
                Item::Class(c) if c.name.name == main => members.extend(c.methods.iter().map(|m| &m.decl.name)),
                Item::Enum(e) if e.name.name == main => members.extend(e.variants.iter().map(|(v, _)| v)),
                Item::Interface(d) if d.name.name == main => members.extend(d.methods.iter().map(|m| &m.name)),
                _ => {}
            }
        }
        for item in &program.items {
            let name = crate::parser::item_name(item);
            // The constructor has the class's name.
            if name.name != main && members.iter().any(|m| m.name == name.name) {
                return Err(Diagnostic::new(
                    format!("`{}` is both an item of this file and a member of `{main}`", name.name),
                    name.span,
                )
                .with_note(format!("other files would write `{main}.{}` for both; rename one of them", name.name)));
            }
        }
        Ok(())
    }
}

/// What one file can see: its own items and the files it uses.
#[derive(Clone, Debug)]
pub struct Scope {
    pub me: Rc<ModuleInfo>,
    /// `Circle` -> the file `Shapes/Circle.kstl`.
    pub uses: HashMap<String, Rc<ModuleInfo>>,
}

/// Gives every name in `program` its full form (see the module docs).
pub fn resolve(program: &mut Program, scope: &Scope) -> R<()> {
    for u in &program.uses {
        let alias = u.alias();
        if scope.me.items.contains_key(&alias.name) {
            return Err(Diagnostic::new(format!("`{}` is both a used file and an item of this file", alias.name), alias.span)
                .with_note("rename the item"));
        }
    }
    let mut r = Resolver { scope, type_params: Vec::new(), locals: HashSet::new() };
    for item in &mut program.items {
        r.item(item)?;
    }
    let me = &scope.me;
    program.public = program.public.iter().map(|n| me.mangle(n)).collect();
    Ok(())
}

struct Resolver<'a> {
    scope: &'a Scope,
    /// Type parameters of the generic being resolved (never renamed).
    type_params: Vec<String>,
    /// Every variable name of the proc being resolved (never renamed).
    locals: HashSet<String>,
}

impl Resolver<'_> {
    fn used(&self, alias: &str) -> Option<&Rc<ModuleInfo>> {
        if self.locals.contains(alias) || self.scope.me.items.contains_key(alias) {
            return None;
        }
        self.scope.uses.get(alias)
    }

    /// A built-in that lives in the standard library now (`printl` is
    /// `Console.printl`), called by its old name.
    fn moved(&self, name: &Ident) -> R<()> {
        if self.locals.contains(&name.name) || self.scope.me.items.contains_key(&name.name) {
            return Ok(());
        }
        let Some((file, item, _)) = NATIVES.iter().find(|(_, _, builtin)| *builtin == name.name) else { return Ok(()) };
        let alias = file.rsplit('.').next().unwrap_or(file);
        Err(Diagnostic::new(format!("`{}` is in `{file}`", name.name), name.span)
            .with_note(format!("add `use {file};` at the top of the file and write `{alias}.{item}(...)`")))
    }

    /// `item` of the used file `m`, which must be public.
    fn qualified(&self, m: &ModuleInfo, item: &str, span: Span) -> R<String> {
        if !m.items.contains_key(item) {
            return Err(Diagnostic::new(format!("`{}` has no item `{item}`", m.path), span));
        }
        if !m.public.contains(item) {
            return Err(Diagnostic::new(format!("`{item}` is private to `{}`", m.path), span)
                .with_note(format!("mark it `public` in {}.kstl to use it here", m.path.replace('.', "/"))));
        }
        Ok(match m.items[item] {
            Kind::Native(builtin) => builtin.to_string(),
            _ => m.mangle(item),
        })
    }

    /// A name that can only be an item (a type, a called proc, a class in
    /// `is`, an interface in `@impl`): `Forma`, `Geometry.Forma`, `Box<Forma>`.
    fn name(&self, n: &str, span: Span) -> R<String> {
        if let Some((base, args)) = generic_parts(n) {
            let base = self.name(base, span)?;
            let args = args.into_iter().map(|t| self.ty(t, span)).collect::<R<Vec<_>>>()?;
            return Ok(generic_name(&base, &args).to_string());
        }
        if let Some((alias, item)) = n.split_once('.') {
            let Some(m) = self.used(alias) else {
                if self.scope.me.items.contains_key(alias) {
                    return Ok(n.to_string());
                }
                return Err(Diagnostic::new(format!("`{alias}` is not a file this one uses"), span)
                    .with_note(format!("add `use {alias};` at the top of the file")));
            };
            return self.qualified(m, item, span);
        }
        if self.type_params.iter().any(|t| t == n) {
            return Ok(n.to_string());
        }
        match self.scope.me.items.get(n) {
            Some(Kind::Native(builtin)) => return Ok(builtin.to_string()),
            Some(_) => return Ok(self.scope.me.mangle(n)),
            None => {}
        }
        if let Some(m) = self.used(n).filter(|m| m.items.contains_key(m.alias())) {
            return self.qualified(m, m.alias(), span);
        }
        Ok(n.to_string())
    }

    fn ty(&self, t: Type, span: Span) -> R<Type> {
        let n = |s: &'static str| self.name(s, span).map(|x| intern_name(&x));
        Ok(match t {
            Type::Named(s) => Type::Named(n(s)?),
            Type::LentNamed(s) => Type::LentNamed(n(s)?),
            Type::NamedOpt(s) => Type::NamedOpt(n(s)?),
            Type::LentNamedOpt(s) => Type::LentNamedOpt(n(s)?),
            Type::Own(Buf { elem: Elem::Named(s), sym }) => Type::Own(Buf { elem: Elem::Named(n(s)?), sym }),
            Type::Lent(Buf { elem: Elem::Named(s), sym }) => Type::Lent(Buf { elem: Elem::Named(n(s)?), sym }),
            t => t,
        })
    }

    fn type_expr(&self, t: &mut TypeExpr) -> R<()> {
        t.ty = self.ty(t.ty, t.span)?;
        Ok(())
    }

    fn ident(&self, i: &mut Ident) -> R<()> {
        i.name = self.name(&i.name, i.span)?;
        Ok(())
    }

    fn item(&mut self, item: &mut Item) -> R<()> {
        let me = self.scope.me.clone();
        match item {
            Item::Const(c) => {
                c.name.name = me.mangle(&c.name.name);
                self.type_expr(&mut c.ty)?;
                self.locals.clear();
                self.expr(&mut c.value)?;
            }
            Item::Proc(p) => {
                p.name.name = me.mangle(&p.name.name);
                self.proc(p, &[])?;
            }
            Item::Enum(e) => {
                e.name.name = me.mangle(&e.name.name);
                self.locals.clear();
                for (_, v) in &mut e.variants {
                    if let Some(v) = v {
                        self.expr(v)?;
                    }
                }
            }
            Item::Struct(s) => {
                s.name.name = me.mangle(&s.name.name);
                for (_, t) in &mut s.fields {
                    self.type_expr(t)?;
                }
            }
            Item::Class(c) => {
                c.name.name = me.mangle(&c.name.name);
                for a in c.annotations.iter_mut().filter(|a| a.name.name == "impl") {
                    a.args.iter_mut().try_for_each(|i| self.ident(i))?;
                }
                let params = self.type_params(&mut c.generics)?;
                self.type_params.extend(params.iter().cloned());
                for f in &mut c.fields {
                    self.type_expr(&mut f.ty)?;
                }
                for m in &mut c.methods {
                    self.proc(&mut m.decl, &params)?;
                }
                self.type_params.clear();
            }
            Item::Interface(d) => {
                d.name.name = me.mangle(&d.name.name);
                for m in &mut d.methods {
                    self.proc(m, &[])?;
                }
            }
        }
        Ok(())
    }

    /// The bounds of `<T extends Forma>`, and the parameter names.
    fn type_params(&mut self, params: &mut [TypeParam]) -> R<Vec<String>> {
        for p in params.iter_mut() {
            if let Some(b) = &mut p.bound {
                self.ident(b)?;
            }
        }
        Ok(params.iter().map(|p| p.name.name.clone()).collect())
    }

    fn proc(&mut self, p: &mut ProcDecl, class_params: &[String]) -> R<()> {
        let own = self.type_params(&mut p.generics)?;
        self.type_params = class_params.iter().cloned().chain(own).collect();
        self.locals = p.params.iter().map(|x| x.name.name.clone()).collect();
        self.locals.insert("self".into());
        collect_locals(&p.body, &mut self.locals);
        for x in &mut p.params {
            self.type_expr(&mut x.ty)?;
        }
        if let Some(r) = &mut p.ret {
            self.type_expr(r)?;
        }
        for t in &mut p.throws {
            self.ident(t)?;
        }
        self.block(&mut p.body)?;
        self.type_params = class_params.to_vec();
        Ok(())
    }

    fn block(&mut self, b: &mut Block) -> R<()> {
        b.stmts.iter_mut().try_for_each(|s| self.stmt(s))
    }

    fn stmt(&mut self, s: &mut Stmt) -> R<()> {
        match &mut s.kind {
            StmtKind::Assert { cond, .. } => self.expr(cond),
            StmtKind::Let { ty, value, on_nat, .. } => {
                if let Some(t) = ty {
                    self.type_expr(t)?;
                }
                if let Some(v) = value {
                    self.expr(v)?;
                }
                if let Some(b) = on_nat {
                    self.block(b)?;
                }
                Ok(())
            }
            StmtKind::Assign { target, value } => {
                self.expr(target)?;
                self.expr(value)
            }
            StmtKind::Burn(_) | StmtKind::Break | StmtKind::Continue | StmtKind::Return(None) => Ok(()),
            StmtKind::Return(Some(e)) | StmtKind::Expr(e) => self.expr(e),
            StmtKind::Tri { head, neg, zero, pos } => {
                self.expr(head)?;
                self.block(neg)?;
                self.block(zero)?;
                self.block(pos)
            }
            StmtKind::If { cond, then, otherwise } => {
                self.expr(cond)?;
                self.block(then)?;
                if let Some(o) = otherwise {
                    self.block(o)?;
                }
                Ok(())
            }
            StmtKind::Walk { range, body, .. } => {
                match range {
                    Range::Between(a, b) => {
                        self.expr(a)?;
                        self.expr(b)?;
                    }
                    Range::Span(k) => self.expr(k)?,
                }
                self.block(body)
            }
            StmtKind::Match { scrut, arms } => {
                self.expr(scrut)?;
                for arm in arms {
                    arm.pats.iter_mut().try_for_each(|p| self.expr(p))?;
                    self.block(&mut arm.body)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body } => {
                self.expr(cond)?;
                self.block(body)
            }
            StmtKind::For { init, cond, step, body } => {
                self.stmt(init)?;
                self.expr(cond)?;
                self.stmt(step)?;
                self.block(body)
            }
            StmtKind::Throw(e) => self.expr(e),
            StmtKind::Try { body, catches } => {
                self.block(body)?;
                for c in catches {
                    self.ident(&mut c.class)?;
                    self.block(&mut c.body)?;
                }
                Ok(())
            }
        }
    }

    fn expr(&mut self, e: &mut Expr) -> R<()> {
        let span = e.span;
        match &mut e.kind {
            ExprKind::Format(pieces) => {
                for p in pieces {
                    if let FmtPiece::Value { expr, .. } = p {
                        self.expr(expr)?;
                    }
                }
            }
            ExprKind::Num(_)
            | ExprKind::Real(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::NaT
            | ExprKind::Bool(_)
            | ExprKind::Give(_) => {}
            ExprKind::Var(n) => {
                if self.locals.contains(n.as_str()) || self.type_params.contains(n) {
                    return Ok(());
                }
                // `List<int>.empty()`: a generic class as the receiver.
                if generic_parts(n).is_some() {
                    *n = self.name(n, span)?;
                    return Ok(());
                }
                // Procs are only called, never named alone, so a variable
                // with a proc's name is a variable.
                match self.scope.me.items.get(n.as_str()) {
                    Some(Kind::Const | Kind::Type) => *n = self.scope.me.mangle(n),
                    Some(Kind::Proc | Kind::Native(_)) => {}
                    None => {
                        if let Some(m) = self.used(n) {
                            let main = m.alias().to_string();
                            if !m.items.contains_key(&main) {
                                return Err(Diagnostic::new(format!("`{n}` is a file, not a value"), span)
                                    .with_note(format!("write one of its items: `{n}.name`")));
                            }
                            *n = self.qualified(&m.clone(), &main, span)?;
                        }
                    }
                }
            }
            // `Geometry.PI`, `Geometry.Cor.Vermelho`.
            ExprKind::Member(base, field) => {
                if let ExprKind::Var(alias) = &base.kind
                    && let Some(m) = self.used(alias).cloned()
                {
                    if m.items.contains_key(&field.name) {
                        e.kind = ExprKind::Var(self.qualified(&m, &field.name, span)?);
                        return Ok(());
                    }
                    if !m.items.contains_key(m.alias()) {
                        return Err(Diagnostic::new(format!("`{}` has no item `{}`", m.path, field.name), field.span));
                    }
                }
                self.expr(base)?;
            }
            // `Geometry.area(2.0)`, `Geometry.Circle(1.0)`, `Collections.List<int>()`.
            ExprKind::MethodCall { recv, name, args } => {
                args.iter_mut().try_for_each(|a| self.expr(a))?;
                if let ExprKind::Var(alias) = &recv.kind
                    && let Some(m) = self.used(alias).cloned()
                {
                    let (base, gargs) = match generic_parts(&name.name) {
                        Some((b, a)) => (b.to_string(), Some(a)),
                        None => (name.name.clone(), None),
                    };
                    if m.items.contains_key(&base) {
                        let full = self.qualified(&m, &base, name.span)?;
                        let full = match gargs {
                            Some(a) => {
                                let a = a.into_iter().map(|t| self.ty(t, name.span)).collect::<R<Vec<_>>>()?;
                                generic_name(&full, &a).to_string()
                            }
                            None => full,
                        };
                        let name = Ident { name: full, span: name.span };
                        e.kind = ExprKind::Call { name, args: std::mem::take(args) };
                        return Ok(());
                    }
                    if !m.items.contains_key(m.alias()) {
                        return Err(Diagnostic::new(format!("`{}` has no item `{base}`", m.path), name.span));
                    }
                }
                self.expr(recv)?;
            }
            ExprKind::Call { name, args } => {
                self.moved(name)?;
                self.ident(name)?;
                args.iter_mut().try_for_each(|a| self.expr(a))?;
            }
            ExprKind::StructLit { name, fields } => {
                self.ident(name)?;
                fields.iter_mut().try_for_each(|(_, v)| self.expr(v))?;
            }
            ExprKind::Is(x, class) => {
                self.expr(x)?;
                self.ident(class)?;
            }
            ExprKind::Unary(_, a)
            | ExprKind::Cast(a, _)
            | ExprKind::Claim(a)
            | ExprKind::GiveField(a)
            | ExprKind::Lend(a) => self.expr(a)?,
            ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) | ExprKind::Elvis(a, b) => {
                self.expr(a)?;
                self.expr(b)?;
            }
            ExprKind::ArrayLit(items) => items.iter_mut().try_for_each(|a| self.expr(a))?,
            ExprKind::Safe { recv, rest, .. } => {
                self.expr(recv)?;
                self.expr(rest)?;
            }
        }
        Ok(())
    }
}

/// Every name a proc gives a variable, wherever it is declared.
fn collect_locals(b: &Block, out: &mut HashSet<String>) {
    for s in &b.stmts {
        collect_stmt(s, out);
    }
}

fn collect_stmt(s: &Stmt, out: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::Let { name, on_nat, .. } => {
            out.insert(name.name.clone());
            if let Some(b) = on_nat {
                collect_locals(b, out);
            }
        }
        StmtKind::Tri { neg, zero, pos, .. } => {
            for b in [neg, zero, pos] {
                collect_locals(b, out);
            }
        }
        StmtKind::If { then, otherwise, .. } => {
            collect_locals(then, out);
            if let Some(o) = otherwise {
                collect_locals(o, out);
            }
        }
        StmtKind::Walk { var, body, .. } => {
            out.insert(var.name.clone());
            collect_locals(body, out);
        }
        StmtKind::Match { arms, .. } => arms.iter().for_each(|a| collect_locals(&a.body, out)),
        StmtKind::While { body, .. } => collect_locals(body, out),
        StmtKind::For { init, step, body, .. } => {
            collect_stmt(init, out);
            collect_stmt(step, out);
            collect_locals(body, out);
        }
        StmtKind::Try { body, catches } => {
            collect_locals(body, out);
            for c in catches {
                out.insert(c.var.name.clone());
                collect_locals(&c.body, out);
            }
        }
        _ => {}
    }
}

/// Interns a resolved name (types keep `&'static str` names).
fn intern_name(s: &str) -> &'static str {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<HashSet<&'static str>>> = std::sync::OnceLock::new();
    let mut seen = SEEN.get_or_init(Default::default).lock().expect("not poisoned");
    if let Some(k) = seen.get(s) {
        return k;
    }
    let k = intern(s);
    seen.insert(k);
    k
}
