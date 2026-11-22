//! Generics: one copy of a generic class or proc per list of types used.
//!
//! Runs between the parser and the checker. `class Box<T>` used as
//! `Box<int>` and `Box<Conta>` becomes two plain classes, `Box<int>` and
//! `Box<Conta>`, with `T` replaced in each (C++ and Rust do the same; Java
//! instead keeps one copy and boxes every value, which Kestrel cannot do:
//! values have different sizes).
//!
//! Each generic is also checked once on its own, with every `T` standing for
//! its bound (`T extends Forma` -> `Forma`) or, without one, for an empty
//! interface named `T`: an object of unknown class with no methods. So code
//! that compiles there uses only what every `T` allows (moving it, storing
//! it, and the bound's methods), and each real copy compiles too.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diag::{Diagnostic, Span};

type R<T> = Result<T, Diagnostic>;

/// At most this many copies of generics (a generic that uses itself with a
/// bigger type, like `A<T>` using `A<A<T>>`, would never stop).
const MAX_INSTANCES: usize = 729;

/// The program with its generics replaced by one copy per use.
pub fn expand(program: Program) -> R<Program> {
    let mut templates: HashMap<String, Item> = HashMap::new();
    let mut items = Vec::new();
    let Program { items: program_items, uses, public, external, docs, overloads } = program;
    let program_rest = Program { items: Vec::new(), uses, public, external: external.clone(), docs, overloads };
    for item in program_items {
        let generic = match &item {
            Item::Class(c) => {
                if let Some(m) = c.methods.iter().find(|m| !m.decl.generics.is_empty()) {
                    return Err(Diagnostic::new("a method cannot have its own type parameters", m.decl.name.span)
                        .with_note("put them on the class (`class Box<T>`), or use a generic proc outside it"));
                }
                (!c.generics.is_empty()).then(|| c.name.name.clone())
            }
            Item::Proc(p) => (!p.generics.is_empty()).then(|| p.name.name.clone()),
            _ => None,
        };
        match generic {
            Some(name) => {
                templates.insert(name, item);
            }
            None => items.push(item),
        }
    }
    let enums = items.iter().filter_map(|i| if let Item::Enum(e) = i { Some(e.name.name.clone()) } else { None }).collect();
    let mut cx = Expander { templates, objects: HashSet::new(), impls: HashMap::new(), enums, done: HashSet::new(), queue: Vec::new() };
    cx.objects.extend(items.iter().filter_map(|i| match i {
        Item::Class(c) => Some(c.name.name.clone()),
        Item::Interface(d) => Some(d.name.name.clone()),
        _ => None,
    }));
    for item in &items {
        if let Item::Class(c) = item {
            cx.impls.insert(c.name.name.clone(), class_impls(c));
        }
    }
    let names: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            Item::Class(c) => Some(c.name.name.clone()),
            Item::Interface(d) => Some(d.name.name.clone()),
            Item::Struct(s) => Some(s.name.name.clone()),
            Item::Enum(e) => Some(e.name.name.clone()),
            _ => None,
        })
        .collect();
    // A `T` without a bound stands for an empty interface named `T`.
    let mut stand_ins: Vec<Item> = Vec::new();
    let mut checks = Vec::new();
    let mut template_names: Vec<String> = cx.templates.keys().cloned().collect();
    template_names.sort();
    for t in &template_names {
        let params = cx.params(t).to_vec();
        // A generic of another file was checked when that file was compiled.
        if external.contains(t) {
            continue;
        }
        if let Some(Item::Class(c)) = cx.templates.get(t) {
            cx.objects.insert(c.name.name.clone());
        }
        let mut args = Vec::new();
        for p in &params {
            if names.contains(&p.name.name) || cx.templates.contains_key(&p.name.name) {
                return Err(Diagnostic::new(format!("type parameter `{}` has the name of a type", p.name.name), p.name.span)
                    .with_note("pick a name that no class, interface, struct or enum uses"));
            }
            match &p.bound {
                Some(b) => args.push(Type::Named(intern(&b.name))),
                None => {
                    if !stand_ins.iter().any(|i| matches!(i, Item::Interface(d) if d.name.name == p.name.name)) {
                        stand_ins.push(Item::Interface(InterfaceDecl { name: p.name.clone(), methods: Vec::new(), bodies: Vec::new() }));
                        cx.objects.insert(p.name.name.clone());
                    }
                    args.push(Type::Named(intern(&p.name.name)));
                }
            }
        }
        checks.push((generic_name(t, &args), Span::default()));
    }
    for item in &items {
        let mut item = item.clone();
        walk_item(&mut item, &mut |site| {
            if let Some(name) = site.generic() {
                cx.queue.push((name, site.span()));
            }
        });
    }
    cx.queue.extend(checks);
    let mut made = Vec::new();
    while let Some((name, span)) = cx.queue.pop() {
        if !cx.done.insert(name.to_string()) {
            continue;
        }
        if cx.done.len() > MAX_INSTANCES {
            return Err(Diagnostic::new(format!("too many copies of generics (more than {MAX_INSTANCES})"), span)
                .with_note("a generic probably uses itself with a bigger type, like `A<A<T>>`"));
        }
        made.push(cx.instance(name, span)?);
    }
    items.extend(stand_ins);
    items.extend(made);
    Ok(Program { items, ..program_rest })
}

/// The interfaces `@impl(...)` lists for a class.
fn class_impls(c: &ClassDecl) -> Vec<String> {
    c.annotations.iter().filter(|a| a.name.name == "impl").flat_map(|a| a.args.iter().map(|i| i.name.clone())).collect()
}

struct Expander {
    templates: HashMap<String, Item>,
    /// Classes and interfaces: a `T` that is one of them is lent with `lent`;
    /// any other `T` (a number, text, enum or struct) is just copied.
    objects: HashSet<String>,
    /// The interfaces each class implements (for `T extends Forma`).
    impls: HashMap<String, Vec<String>>,
    /// Enum names (enums are `Comparable`).
    enums: HashSet<String>,
    done: HashSet<String>,
    queue: Vec<(&'static str, Span)>,
}

impl Expander {
    fn params(&self, template: &str) -> &[TypeParam] {
        match &self.templates[template] {
            Item::Class(c) => &c.generics,
            Item::Proc(p) => &p.generics,
            _ => unreachable!("only classes and procs are templates"),
        }
    }

    /// The copy of a generic named `name` (`Box<int>`), used at `span`.
    fn instance(&mut self, name: &'static str, span: Span) -> R<Item> {
        let (base, args) = generic_parts(name).expect("only generic names are queued");
        let Some(template) = self.templates.get(base) else {
            return Err(Diagnostic::new(format!("`{base}` is not generic, so it takes no `<...>`"), span));
        };
        let params = self.params(base).to_vec();
        if params.len() != args.len() {
            let s = if params.len() == 1 { "" } else { "s" };
            return Err(Diagnostic::new(format!("`{base}` takes {} type argument{s}, found {}", params.len(), args.len()), span));
        }
        for (p, a) in params.iter().zip(&args) {
            if let Some(b) = &p.bound
                && !self.satisfies(*a, &b.name)
            {
                return Err(Diagnostic::new(format!("`{a}` does not implement `{}`, which `{}` needs", b.name, p.name.name), span)
                    .with_note(format!("`{base}<{} extends {}>`", p.name.name, b.name)));
            }
        }
        let map: HashMap<&str, Type> = params.iter().map(|p| p.name.name.as_str()).zip(args.iter().copied()).collect();
        let mut item = template.clone();
        let mut failure = None;
        match &mut item {
            Item::Class(c) => {
                c.name.name = name.to_string();
                c.generics.clear();
                c.instance = true;
                for m in &mut c.methods {
                    m.decl.instance = true;
                }
                self.objects.insert(name.to_string());
                let impls = class_impls(c);
                self.impls.insert(name.to_string(), impls);
            }
            Item::Proc(p) => {
                p.name.name = name.to_string();
                p.generics.clear();
                p.instance = true;
            }
            _ => unreachable!(),
        }
        let objects = &self.objects;
        let queue = &mut self.queue;
        walk_item(&mut item, &mut |mut site| {
            if failure.is_some() {
                return;
            }
            if let Err(e) = site.substitute(&map, objects) {
                failure = Some(e);
                return;
            }
            if let Some(n) = site.generic() {
                queue.push((n, site.span()));
            }
        });
        if let Some(e) = failure {
            return Err(e);
        }
        // A parameter whose type became a value (`lent x: T` with `T = int`) is just copied.
        let fix = |p: &mut ProcDecl| {
            for param in &mut p.params {
                // A `T` that became a value (`int`) is copied, `&T` too: generic
                // code cannot change a `T`, so a copy behaves the same.
                if param.mode.is_some() && !matches!(param.ty.ty, Type::Named(_) | Type::NamedOpt(_) | Type::LentNamed(_) | Type::LentNamedOpt(_) | Type::Own(_) | Type::Lent(_)) {
                    param.mode = None;
                }
            }
        };
        match &mut item {
            Item::Class(c) => c.methods.iter_mut().for_each(|m| fix(&mut m.decl)),
            Item::Proc(p) => fix(p),
            _ => {}
        }
        Ok(item)
    }

    /// Whether type argument `a` implements interface `bound`.
    fn satisfies(&self, a: Type, bound: &str) -> bool {
        match a {
            // Numbers, texts and enums are `Comparable` by themselves.
            Type::Named(n) if bound == crate::object::COMPARABLE && self.enums.contains(n) => true,
            Type::Scalar(_) | Type::Text => bound == crate::object::COMPARABLE,
            Type::Named(n) => n == bound || self.impls.get(n).is_some_and(|i| i.iter().any(|x| x == bound)),
            _ => false,
        }
    }
}

/// A place in the program that may name a generic or a type parameter.
enum Site<'a> {
    Type(&'a mut Type, Span),
    /// A name in an expression: `Box<int>` in `Box<int>.new()`, a proc call
    /// `max<T>(...)`, a struct literal `Box<T> { ... }`.
    Name(&'a mut String, Span),
}

impl Site<'_> {
    fn span(&self) -> Span {
        match self {
            Site::Type(_, s) | Site::Name(_, s) => *s,
        }
    }

    /// The generic name here (`Box<int>`), if there is one.
    fn generic(&self) -> Option<&'static str> {
        let n: &str = match self {
            Site::Type(t, _) => match **t {
                Type::Named(n) | Type::NamedOpt(n) | Type::LentNamed(n) | Type::LentNamedOpt(n) => n,
                Type::Own(b) | Type::Lent(b) => match b.elem {
                    Elem::Named(n) => n,
                    _ => return None,
                },
                _ => return None,
            },
            Site::Name(s, _) => s.as_str(),
        };
        generic_parts(n).map(|_| generic_name_of(n))
    }

    /// Replaces the type parameters in `map` (`T` -> `int`).
    fn substitute(&mut self, map: &HashMap<&str, Type>, objects: &HashSet<String>) -> R<()> {
        let span = self.span();
        match self {
            Site::Type(t, _) => **t = subst_type(**t, map, objects, span)?,
            Site::Name(s, _) => {
                if let Some(n) = subst_name(s, map, objects, span)? {
                    **s = n.to_string();
                }
            }
        }
        Ok(())
    }
}

/// The interned name for a generic's text (already registered).
fn generic_name_of(n: &str) -> &'static str {
    let (base, args) = generic_parts(n).expect("checked");
    generic_name(base, &args)
}

/// `Box<T>` with `T = int` -> `Box<int>`; `None` if nothing changes.
fn subst_name(n: &str, map: &HashMap<&str, Type>, objects: &HashSet<String>, span: Span) -> R<Option<&'static str>> {
    let Some((base, args)) = generic_parts(n) else { return Ok(None) };
    let mut new = Vec::new();
    for a in &args {
        new.push(subst_type(*a, map, objects, span)?);
    }
    Ok((new != args).then(|| generic_name(base, &new)))
}

/// A type with the type parameters replaced. `lent T` stays lent only if `T`
/// became a class or interface; `T?` becomes `int?`, `Conta?`, ...
fn subst_type(t: Type, map: &HashMap<&str, Type>, objects: &HashSet<String>, span: Span) -> R<Type> {
    let named = |n: &str| -> R<Type> {
        if let Some(a) = map.get(n) {
            return Ok(*a);
        }
        Ok(match subst_name(n, map, objects, span)? {
            Some(g) => Type::Named(g),
            None => Type::Named(intern_static(n)),
        })
    };
    let is_object = |t: Type| matches!(t, Type::Named(n) if objects.contains(n) || generic_parts(n).is_some());
    let opt = |t: Type| -> R<Type> {
        Ok(match t {
            Type::Named(n) => Type::NamedOpt(n),
            t => t.opt().ok_or_else(|| Diagnostic::new(format!("{t} cannot be null, so `T?` cannot be used with it"), span))?,
        })
    };
    Ok(match t {
        Type::Named(n) => named(n)?,
        Type::NamedOpt(n) => opt(named(n)?)?,
        Type::LentNamed(n) => match named(n)? {
            Type::Named(c) if is_object(Type::Named(c)) => Type::LentNamed(c),
            Type::NamedOpt(c) => Type::LentNamedOpt(c),
            other => other,
        },
        Type::LentNamedOpt(n) => match named(n)? {
            Type::Named(c) | Type::NamedOpt(c) if is_object(Type::Named(c)) => Type::LentNamedOpt(c),
            other => opt(other)?,
        },
        Type::Own(b) | Type::Lent(b) => {
            let Elem::Named(n) = b.elem else { return Ok(t) };
            let e = named(n)?;
            let Some(elem) = Elem::of(e) else {
                return Err(Diagnostic::new(format!("an array cannot hold {e}"), span).with_note("array elements cannot be null or arrays"));
            };
            let buf = Buf { elem, ..b };
            if let Type::Own(_) = t { Type::Own(buf) } else { Type::Lent(buf) }
        }
        other => other,
    })
}

/// Interns a name, once per distinct text.
fn intern_static(n: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut seen = SEEN.get_or_init(Default::default).lock().expect("not poisoned");
    if let Some(k) = seen.get(n) {
        return k;
    }
    let k = intern(n);
    seen.insert(k);
    k
}

// ---- walking every type and name ----

fn walk_item(item: &mut Item, f: &mut dyn FnMut(Site)) {
    match item {
        Item::Proc(p) => walk_proc(p, f),
        Item::Class(c) => {
            for field in &mut c.fields {
                walk_type(&mut field.ty, f);
            }
            for m in &mut c.methods {
                walk_proc(&mut m.decl, f);
            }
        }
        Item::Interface(d) => d.methods.iter_mut().for_each(|m| walk_proc(m, f)),
        Item::Struct(s) => s.fields.iter_mut().for_each(|(_, t)| walk_type(t, f)),
        Item::Const(c) => {
            walk_type(&mut c.ty, f);
            walk_expr(&mut c.value, f);
        }
        Item::Enum(e) => {
            for (_, v) in &mut e.variants {
                if let Some(v) = v {
                    walk_expr(v, f);
                }
            }
        }
    }
}

fn walk_type(t: &mut TypeExpr, f: &mut dyn FnMut(Site)) {
    f(Site::Type(&mut t.ty, t.span));
}

fn walk_proc(p: &mut ProcDecl, f: &mut dyn FnMut(Site)) {
    for param in &mut p.params {
        walk_type(&mut param.ty, f);
    }
    if let Some(r) = &mut p.ret {
        walk_type(r, f);
    }
    walk_block(&mut p.body, f);
}

fn walk_block(b: &mut Block, f: &mut dyn FnMut(Site)) {
    for s in &mut b.stmts {
        walk_stmt(s, f);
    }
}

fn walk_stmt(s: &mut Stmt, f: &mut dyn FnMut(Site)) {
    match &mut s.kind {
        StmtKind::Assert { cond, .. } => walk_expr(cond, f),
        StmtKind::Let { ty, value, on_nat, .. } => {
            if let Some(t) = ty {
                walk_type(t, f);
            }
            if let Some(v) = value {
                walk_expr(v, f);
            }
            if let Some(b) = on_nat {
                walk_block(b, f);
            }
        }
        StmtKind::Assign { target, value } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }
        StmtKind::Burn(_) | StmtKind::Break | StmtKind::Continue | StmtKind::Return(None) => {}
        StmtKind::Return(Some(e)) | StmtKind::Expr(e) | StmtKind::Throw(e) => walk_expr(e, f),
        StmtKind::Try { body, catches } => {
            walk_block(body, f);
            for c in catches {
                walk_block(&mut c.body, f);
            }
        }
        StmtKind::Tri { head, neg, zero, pos } => {
            walk_expr(head, f);
            for b in [neg, zero, pos] {
                walk_block(b, f);
            }
        }
        StmtKind::If { cond, then, otherwise } => {
            walk_expr(cond, f);
            walk_block(then, f);
            if let Some(b) = otherwise {
                walk_block(b, f);
            }
        }
        StmtKind::Walk { range, body, .. } => {
            match range {
                Range::Between(a, b) => {
                    walk_expr(a, f);
                    walk_expr(b, f);
                }
                Range::Span(k) => walk_expr(k, f),
            }
            walk_block(body, f);
        }
        StmtKind::Match { scrut, arms } => {
            walk_expr(scrut, f);
            for a in arms {
                a.pats.iter_mut().for_each(|p| walk_expr(p, f));
                walk_block(&mut a.body, f);
            }
        }
        StmtKind::While { cond, body } => {
            walk_expr(cond, f);
            walk_block(body, f);
        }
        StmtKind::For { init, cond, step, body } => {
            walk_stmt(init, f);
            walk_expr(cond, f);
            walk_stmt(step, f);
            walk_block(body, f);
        }
    }
}

fn walk_expr(e: &mut Expr, f: &mut dyn FnMut(Site)) {
    let span = e.span;
    match &mut e.kind {
        ExprKind::Num(_) | ExprKind::Real(_) | ExprKind::Char(_) | ExprKind::Str(_) | ExprKind::NaT | ExprKind::Bool(_) => {}
        ExprKind::Give(_) => {}
        ExprKind::Format(pieces) => {
            for p in pieces {
                if let FmtPiece::Value { expr, .. } = p {
                    walk_expr(expr, f);
                }
            }
        }
        ExprKind::Var(n) => f(Site::Name(n, span)),
        ExprKind::Unary(_, a) | ExprKind::Claim(a) | ExprKind::GiveField(a) | ExprKind::Lend(a) => walk_expr(a, f),
        ExprKind::Cast(a, t) => {
            walk_expr(a, f);
            f(Site::Type(t, span));
        }
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) | ExprKind::Elvis(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::ArrayLit(xs) => xs.iter_mut().for_each(|x| walk_expr(x, f)),
        ExprKind::Member(a, _) => walk_expr(a, f),
        ExprKind::StructLit { name, fields } => {
            f(Site::Name(&mut name.name, name.span));
            fields.iter_mut().for_each(|(_, x)| walk_expr(x, f));
        }
        ExprKind::Call { name, args } => {
            f(Site::Name(&mut name.name, name.span));
            args.iter_mut().for_each(|x| walk_expr(x, f));
        }
        ExprKind::Safe { recv, rest, .. } => {
            walk_expr(recv, f);
            walk_expr(rest, f);
        }
        ExprKind::Is(a, c) => {
            walk_expr(a, f);
            f(Site::Name(&mut c.name, c.span));
        }
        ExprKind::MethodCall { recv, args, .. } => {
            walk_expr(recv, f);
            args.iter_mut().for_each(|x| walk_expr(x, f));
        }
    }
}
