//! Name resolution and type checking: AST -> [`Hir`] (SPEC §1-§8).
//!
//! Ownership rules (use after give, leaks, double burn, tri symmetry) are
//! checked afterwards by the linear checker on the HIR.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use kestrel_core::charset::char_to_tryte;
use kestrel_core::limits::MAX_LOCALS;
use kestrel_core::word::{TRIT_MAX, TRYTE_MAX};
use kestrel_core::Word;

use crate::ast::{self, Expr, ExprKind, Item, Program, Range, StmtKind, UnOp};
use crate::diag::{Diagnostic, Span};
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

const BUILTINS: [&str; 35] = [
    "print", "printl", "printf", "toText", "emit", "input", "len", "dump", "sub", "toInt", "toReal", "isDigit",
    "isLetter", "isUpper", "isLower", "isSpace", "readFile", "fileExists", "writeFile", "writeBytes",
    "readLine", "eof", "argCount", "arg", "exit", "append", "pop", "insert", "remove", "resize", "clear", "replace",
    "isNaT", "assert", "hashMix",
];
/// Built-ins that change the size of the array they get first.
const RESIZERS: [&str; 6] = ["append", "pop", "insert", "remove", "resize", "clear"];
/// Most cells one array element may take (a struct takes one per field).
const MAX_ELEM_WIDTH: usize = 243;

/// Names of the `is_*` built-ins, in `charset::Class::ALL` order.
const CHAR_CLASSES: [&str; 5] = ["isDigit", "isLetter", "isUpper", "isLower", "isSpace"];

/// Checks a program; it must have a `proc main`.
pub fn check(program: &Program) -> R<Hir> {
    check_with(program, true)
}

/// Checks a program; `need_main: false` (for `ksc --test`) lets it have
/// only `@test` procs. `Hir::entry` is then `main` if there is one, or 0.
pub fn check_with(program: &Program, need_main: bool) -> R<Hir> {
    let mut cx = Checker {
        warnings: RefCell::new(Vec::new()),
        deprecated: HashMap::new(),
        deprecated_classes: HashMap::new(),
        tests: Vec::new(),
        current: None,
        sigs: Vec::new(),
        proc_ids: HashMap::new(),
        consts: HashMap::new(),
        enums: HashMap::new(),
        structs: Structs::new(),
        classes: HashMap::new(),
        ifaces: HashMap::new(),
        class_tags: HashMap::new(),
        dispatchers: HashMap::new(),
        extra: Vec::new(),
        class: None,
        f: FnCx::new(Type::Void),
    };
    // Enums first, so types like `c: Cor` resolve anywhere. Each enum gets a
    // hidden proc that turns a value into its name, for `print` and `toText`.
    let mut name_procs = Vec::new();
    for item in &program.items {
        if let Item::Enum(e) = item {
            name_procs.push(cx.declare_enum(e)?);
        }
    }
    // Struct names next, so a field can name any struct; then their fields.
    let decls: Vec<&ast::StructDecl> =
        program.items.iter().filter_map(|i| if let Item::Struct(s) = i { Some(s) } else { None }).collect();
    let classes: Vec<&ast::ClassDecl> =
        program.items.iter().filter_map(|i| if let Item::Class(c) = i { Some(c) } else { None }).collect();
    let ifaces: Vec<&ast::InterfaceDecl> =
        program.items.iter().filter_map(|i| if let Item::Interface(d) = i { Some(d) } else { None }).collect();
    let names = decls.iter().map(|s| &s.name).chain(classes.iter().map(|c| &c.name)).chain(ifaces.iter().map(|d| &d.name));
    for (k, n) in names.enumerate() {
        let name = &n.name;
        if cx.structs.contains_key(name.as_str())
            || cx.enums.contains_key(name)
            || cx.classes.contains_key(name)
            || cx.ifaces.contains_key(name)
        {
            return Err(err(format!("type `{name}` is defined twice"), n.span));
        }
        if k < decls.len() {
            cx.structs.insert(ast::intern(name), Vec::new());
        } else if k < decls.len() + classes.len() {
            let info = ClassInfo { name: ast::intern(name), fields: Vec::new(), methods: HashMap::new() };
            cx.classes.insert(name.clone(), info);
        } else {
            cx.ifaces.insert(name.clone(), IfaceInfo { name: ast::intern(name), methods: Vec::new(), impls: Vec::new() });
        }
    }
    // Each class's number comes from its name, so every file of the
    // program gives a class the same one (the linker checks they differ).
    cx.class_tags = cx.classes.values().map(|c| (c.name, class_tag(c.name))).collect();
    for s in &decls {
        cx.declare_struct(s)?;
    }
    let mut widths = HashMap::new();
    for s in &decls {
        cx.struct_width(s.name.name.as_str(), &mut widths, &mut Vec::new(), s.name.span)?;
    }
    for c in &classes {
        cx.declare_class_fields(c)?;
    }
    for c in &classes {
        cx.class_cycle(&c.name.name, &mut Vec::new(), c.name.span)?;
    }
    let mut default_decls = Vec::new();
    for d in &ifaces {
        default_decls.extend(cx.declare_iface(d)?);
    }
    let mut method_decls = Vec::new();
    for c in &classes {
        method_decls.extend(cx.declare_methods(c)?);
    }
    for c in &classes {
        cx.check_impls(c)?;
    }
    for item in &program.items {
        match item {
            Item::Const(c) => cx.declare_const(c)?,
            Item::Proc(p) => cx.declare_proc(p)?,
            Item::Enum(_) | Item::Struct(_) | Item::Class(_) | Item::Interface(_) => {}
        }
    }
    for p in &name_procs {
        cx.declare_proc(p)?;
    }
    let entry = match cx.proc_ids.get("main") {
        Some(&id) => id,
        None if !need_main => 0,
        None => return Err(Diagnostic::new("the program has no `proc main`", Span::default())),
    };
    if let Some(main) = cx.proc_ids.get("main").map(|&id| &cx.sigs[id]) {
        if !main.params.is_empty() {
            return Err(Diagnostic::new("`main` cannot take parameters", main.span));
        }
        if !matches!(main.ret, Type::Void | Type::Scalar(Scalar::Int)) {
            return Err(Diagnostic::new(format!("`main` must return an int or nothing, not {}", main.ret), main.span));
        }
    }
    // Where each proc's code is: an item of another file is only declared
    // here; a copy of a generic is compiled by every file that uses it.
    let instances: HashSet<&str> =
        classes.iter().filter(|c| c.instance).map(|c| c.name.name.as_str()).collect();
    let link_of = |owner: &str, instance: bool| {
        if program.external.contains(owner) {
            Link::Import
        } else if instance || instances.contains(owner) {
            Link::Shared
        } else {
            Link::Own
        }
    };
    let mut procs = Vec::new();
    for item in &program.items {
        if let Item::Proc(p) = item {
            procs.push(cx.check_proc(p, link_of(&p.name.name, p.instance))?);
        }
    }
    for (class, p) in &method_decls {
        cx.class = Some(class);
        procs.push(cx.check_proc(p, link_of(class, false))?);
        cx.class = None;
    }
    for p in &name_procs {
        let e = p.name.name.strip_prefix("$name.").unwrap_or(&p.name.name);
        procs.push(cx.check_proc(p, link_of(e, false))?);
    }
    // Default methods of interfaces: `self` is the interface, so they use
    // only its methods, never a class's fields.
    for p in &default_decls {
        let i = p.name.name.rsplit_once('.').map_or(p.name.name.as_str(), |(i, _)| i);
        procs.push(cx.check_proc(p, link_of(i, false))?);
    }
    procs.append(&mut cx.extra);
    // Procs were declared in another order than they were checked: put each at its id.
    procs.sort_by_key(|p| cx.proc_ids[&p.name]);
    let classes = cx.classes.values().map(|c| (c.name, c.fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect())).collect();
    // The classes of this file and how they answer interface calls.
    let mut class_info = Vec::new();
    let mut impls = Vec::new();
    let mut names: Vec<&'static str> = cx.classes.values().map(|c| c.name).collect();
    names.sort_unstable();
    let mut iface_names: Vec<&'static str> = cx.ifaces.values().map(|i| i.name).collect();
    iface_names.sort_unstable();
    for c in names {
        let link = link_of(c, false);
        let tag = cx.class_tags[c];
        let mine: Vec<&'static str> = iface_names.iter().copied().filter(|i| cx.ifaces[*i].impls.contains(&c)).collect();
        if link != Link::Import {
            for i in &mine {
                for m in &cx.ifaces[*i].methods {
                    let cname = if m.name == "equals" || (*i == crate::object::COMPARABLE && m.name == "compareTo") {
                        format!("{}${i}", m.name)
                    } else {
                        m.name.clone()
                    };
                    if let Some(found) = cx.classes[c].methods.get(&cname) {
                        impls.push((format!("$dispatch.{i}.{}", m.name), tag, found.id, found.iface.is_some()));
                    }
                }
            }
        }
        class_info.push(ClassLink { name: c, tag, link, ifaces: mine });
    }
    // Only this file's tests run.
    cx.tests.retain(|(_, id)| procs[*id].link == Link::Own);
    // A warning found twice (an expression checked twice) is shown once.
    let mut warnings = cx.warnings.into_inner();
    warnings.sort_by_key(|w| (w.span.start, w.span.end));
    warnings.dedup();
    let borrows = cx.sigs.iter().map(|sig| if is_lent(sig.ret) { sig.params.iter().position(|(_, t)| is_lent(*t)) } else { None }).collect();
    Ok(Hir { procs, entry, structs: cx.structs, classes, class_info, iface_names, impls, tests: cx.tests, warnings, borrows })
}

#[derive(Clone)]
struct Sig {
    params: Vec<(String, Type)>,
    /// `x: &int` parameters: the caller passes `&n` and gets the value back.
    refs: Vec<bool>,
    ret: Type,
    span: Span,
}

/// Names declared in one block. `names` keeps the latest local per name, so
/// lookups stay fast even in procs with many variables.
#[derive(Default)]
struct Scope {
    ids: Vec<LocalId>,
    names: HashMap<String, LocalId>,
}

struct FnCx {
    locals: Vec<Local>,
    scopes: Vec<Scope>,
    ret: Type,
    loops: usize,
    /// Arrays whose size is known here (`{...}` or `claim 5`), for index checks.
    sizes: HashMap<LocalId, i64>,
    /// Names of arrays that may change size in this proc (given to `append`
    /// & co. or lent to a proc): their size is never known.
    resized: HashSet<String>,
    /// Checking a copy of a generic: `give`, `lend` and `burn` of a value
    /// (a `T` that became `int`) just copy it (`burn` does nothing).
    instance: bool,
    /// What `isNaT` checks proved non-empty for the code being checked
    /// (`None`: proved, then something could have emptied it).
    narrowed: Vec<Option<Narrow>>,
}

impl FnCx {
    fn new(ret: Type) -> FnCx {
        FnCx {
            locals: Vec::new(),
            scopes: vec![Scope::default()],
            ret,
            loops: 0,
            sizes: HashMap::new(),
            resized: HashSet::new(),
            narrowed: Vec::new(),
            instance: false,
        }
    }
}

/// A local or a chain of object fields: `n.proximo.proximo` is `n` and the
/// offsets of the two fields.
type Path = (LocalId, Vec<usize>);

/// What a check proved: a `T?` is not empty (`if !isNaT(x)`, `if isNaT(x)
/// { return; }` & co.), or an interface value holds a given class (`if f is C`).
#[derive(Clone)]
enum Narrow {
    /// A local, retyped meanwhile (`No?` -> `No`, `Forma` -> `Circulo`); the
    /// type it had before is kept here.
    Local(LocalId, Type),
    /// A field of an object: not empty (`None`), or of that class.
    Field(Path, Option<&'static str>),
}

/// `ty` once a check proved it: not empty (`class: None`), or of `class`.
fn proved(ty: Type, class: Option<&'static str>) -> Type {
    let lent = matches!(ty, Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_));
    match class {
        None => ty.unopt(),
        Some(c) if lent => Type::LentClass(c),
        Some(c) => Type::Class(c),
    }
}

/// The interface of an interface value (owned, lent, maybe empty).
fn iface_of(ty: Type) -> Option<&'static str> {
    match ty {
        Type::Iface(i) | Type::LentIface(i) | Type::OptIface(i) | Type::LentOptIface(i) => Some(i),
        _ => None,
    }
}

/// The path `h` reads, if it is a local or fields of objects.
fn path_of(h: &HExpr) -> Option<Path> {
    match &h.kind {
        HExprKind::Local(id) | HExprKind::Lend(id) => Some((*id, Vec::new())),
        HExprKind::ElemField(obj, idx, off)
            if matches!(obj.ty, Type::Class(_) | Type::LentClass(_)) && matches!(idx.kind, HExprKind::Word(w) if w == Word::ZERO) =>
        {
            let (id, mut fields) = path_of(obj)?;
            fields.push(*off);
            Some((id, fields))
        }
        _ => None,
    }
}

/// What `h` being `when` proves: `!isNaT(x)` (true) and `isNaT(x)` (false)
/// that `x` is not empty, `f is C` (true) that `f` is a `C`; joined by `&&`
/// (true) or `||` (false).
fn facts(h: &HExpr, when: bool, out: &mut Vec<(Path, Option<&'static str>)>) {
    match &h.kind {
        HExprKind::Builtin(Builtin::IsNaT, a) if !when && a[0].ty.is_opt() => out.extend(path_of(&a[0]).map(|p| (p, None))),
        HExprKind::IsClass(x, _, c) if when => out.extend(path_of(x).map(|p| (p, Some(*c)))),
        HExprKind::Neg(x) if h.ty == Type::Bool => facts(x, !when, out),
        HExprKind::Binary(BinOp::And, a, b) if when => {
            facts(a, true, out);
            facts(b, true, out);
        }
        HExprKind::Binary(BinOp::Or, a, b) if !when => {
            facts(a, false, out);
            facts(b, false, out);
        }
        _ => {}
    }
}

struct Checker {
    /// `@deprecated` uses found so far (`resolve` only gets `&self`).
    warnings: RefCell<Vec<Diagnostic>>,
    /// Procs and methods marked `@deprecated`, with their message.
    deprecated: HashMap<ProcId, Option<String>>,
    deprecated_classes: HashMap<&'static str, Option<String>>,
    /// `@test` procs, in the order they are written.
    tests: Vec<(String, ProcId)>,
    /// The proc being checked.
    current: Option<ProcId>,
    sigs: Vec<Sig>,
    proc_ids: HashMap<String, ProcId>,
    consts: HashMap<String, HExpr>,
    enums: HashMap<String, EnumInfo>,
    structs: Structs,
    classes: HashMap<String, ClassInfo>,
    ifaces: HashMap<String, IfaceInfo>,
    /// Each class's number, carried by interface values to pick the method.
    class_tags: HashMap<&'static str, i64>,
    /// `$dispatch.I.m` procs made so far, and their HIR (no body: the
    /// linker writes them, knowing every class of the program).
    dispatchers: HashMap<String, ProcId>,
    extra: Vec<HProc>,
    /// The class whose method is being checked (it may use private members).
    class: Option<&'static str>,
    f: FnCx,
}

/// A class: its fields (with visibility) and methods.
struct ClassInfo {
    name: &'static str,
    fields: Vec<(String, Type, bool)>,
    methods: HashMap<String, Method>,
}

#[derive(Clone, Copy)]
struct Method {
    id: ProcId,
    this: Option<ast::SelfMode>,
    public: bool,
    /// A default method of this interface, not written in the class: it
    /// takes `self` as the interface (pointer and class number).
    iface: Option<&'static str>,
}

/// An interface: the methods a class must have to `@impl` it.
struct IfaceInfo {
    name: &'static str,
    methods: Vec<IfaceMethod>,
    /// Classes that implement it.
    impls: Vec<&'static str>,
}

#[derive(Clone)]
struct IfaceMethod {
    name: String,
    this: Option<ast::SelfMode>,
    params: Vec<(String, Type)>,
    ret: Type,
    /// The proc of a default method (`Forma.descrever`), if it has a body.
    default: Option<ProcId>,
}

struct EnumInfo {
    name: &'static str,
    variants: Vec<(String, i64)>,
    /// Hidden proc `$name.E(v: int) -> text`.
    name_proc: String,
}

pub use kestrel_core::class_tag;

fn err(msg: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(msg, span)
}

/// A condition: a `bool`, or a `trit` (whose `0` traps).
/// A borrowed type: `lent` parameters and variables.
fn is_lent(t: Type) -> bool {
    matches!(t, Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_))
}

fn is_truth(t: Type) -> bool {
    matches!(t, Type::Bool | Type::Scalar(Scalar::Trit))
}

fn fits(from: Type, to: Type) -> bool {
    match (from, to) {
        // true/false are +1/-1: a bool is a trit that is never 0.
        (Type::Bool, Type::Scalar(Scalar::Trit)) => true,
        (Type::Scalar(a), Type::Scalar(b)) if a.is_int() && b.is_int() => a.rank() <= b.rank(),
        // A value fits where null is allowed too: `int` -> `int?`.
        (f, t) if t.is_opt() && matches!(t, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_)) => {
            fits(f.unopt(), t.unopt())
        }
        _ => from == to,
    }
}

/// An untyped literal adapts to the type of the other operand.
fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Num(_) | ExprKind::NaT => true,
        ExprKind::Unary(_, inner) => is_literal(inner),
        _ => false,
    }
}

fn hx(kind: HExprKind, ty: Type, span: Span) -> HExpr {
    HExpr { kind, ty, span }
}

impl Checker {
    // ---- declarations ----

    fn declare_const(&mut self, c: &ast::ConstDecl) -> R<()> {
        if self.consts.contains_key(&c.name.name) {
            return Err(err(format!("const `{}` is defined twice", c.name.name), c.name.span));
        }
        let ty = self.resolve(&c.ty)?;
        if !matches!(ty, Type::Scalar(_) | Type::Text | Type::Bool) {
            return Err(err("a const must be a number, a bool or text", c.ty.span));
        }
        let value = self.expect(&c.value, ty)?;
        if !matches!(value.kind, HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_)) {
            return Err(err("a const value must be a literal", c.value.span));
        }
        // The const takes its declared type, so `const T: tryte = 5;` is a tryte.
        let value = HExpr { ty, ..value };
        self.consts.insert(c.name.name.clone(), value);
        Ok(())
    }

    fn declare_enum(&mut self, e: &ast::EnumDecl) -> R<ast::ProcDecl> {
        let name = &e.name.name;
        if self.enums.contains_key(name) {
            return Err(err(format!("enum `{name}` is defined twice"), e.name.span));
        }
        if e.variants.is_empty() {
            return Err(err(format!("enum `{name}` needs at least one value"), e.name.span));
        }
        let mut variants: Vec<(String, i64)> = Vec::new();
        let mut next = 0;
        for (v, value) in &e.variants {
            if variants.iter().any(|(n, _)| *n == v.name) {
                return Err(err(format!("`{}` appears twice in enum `{name}`", v.name), v.span));
            }
            let k = match value {
                None => next,
                Some(x) => match self.expect(x, Type::Scalar(Scalar::Int))?.kind {
                    HExprKind::Word(w) if !w.is_nat() => w.get().unwrap_or(0),
                    _ => return Err(err("an enum value must be an integer literal", x.span)),
                },
            };
            if let Some((other, _)) = variants.iter().find(|(_, val)| *val == k) {
                return Err(err(format!("`{}` and `{other}` both have the value {k}", v.name), v.span));
            }
            variants.push((v.name.clone(), k));
            next = k + 1;
        }
        // proc $name.E(v: int) -> text { if v == k0 { return "A"; } ... return "Last"; }
        let sp = e.name.span;
        let ex = |kind| Expr { kind, span: sp };
        let st = |kind| ast::Stmt { kind, span: sp };
        // A value that is not in the enum (like the 0 that `claim` fills
        // with) prints as its number.
        let mut stmts = Vec::new();
        for (n, k) in &variants {
            let v = ex(ExprKind::Var("v".into()));
            let cond = ex(ExprKind::Binary(BinOp::Eq, Box::new(v), Box::new(ex(ExprKind::Num(Word::new(*k))))));
            let then = ast::Block { stmts: vec![st(StmtKind::Return(Some(ex(ExprKind::Str(n.clone())))))], span: sp };
            stmts.push(st(StmtKind::If { cond, then, otherwise: None }));
        }
        let num = ex(ExprKind::Call { name: ast::Ident { name: "toText".into(), span: sp }, args: vec![ex(ExprKind::Var("v".into()))] });
        stmts.push(st(StmtKind::Return(Some(num))));
        let name_proc = format!("$name.{name}");
        let int = ast::TypeExpr { ty: Type::Scalar(Scalar::Int), span: sp };
        let decl = ast::ProcDecl {
            annotations: Vec::new(),
            generics: Vec::new(),
            instance: false,
            name: ast::Ident { name: name_proc.clone(), span: sp },
            this: None,
            params: vec![ast::Param { name: ast::Ident { name: "v".into(), span: sp }, ty: int, mode: None }],
            ret: Some(ast::TypeExpr { ty: Type::Text, span: sp }),
            body: ast::Block { stmts, span: sp },
        };
        self.enums.insert(name.clone(), EnumInfo { name: ast::intern(name), variants, name_proc });
        Ok(decl)
    }

    /// Turns a type name into its enum or struct type.
    fn resolve(&self, t: &ast::TypeExpr) -> R<Type> {
        let buf = match t.ty {
            Type::Own(b) | Type::Lent(b) => b,
            Type::Named(_) | Type::LentNamed(_) | Type::NamedOpt(_) | Type::LentNamedOpt(_) => return self.resolve_name(t),
            other => return Ok(other),
        };
        let elem_ty = self.resolve(&ast::TypeExpr { ty: buf.ty(), span: t.span })?;
        if let Type::Struct(n) = elem_ty {
            let w = width(&self.structs, elem_ty);
            if w > MAX_ELEM_WIDTH {
                return Err(err(format!("struct `{n}` has {w} fields (counting nested ones); an array element can have at most {MAX_ELEM_WIDTH}"), t.span));
            }
        }
        let buf = ast::Buf { elem: ast::Elem::of(elem_ty).expect("not an array"), ..buf };
        Ok(if let Type::Own(_) = t.ty { Type::Own(buf) } else { Type::Lent(buf) })
    }

    fn resolve_name(&self, t: &ast::TypeExpr) -> R<Type> {
        // `No?` / `lent No?`: an object (or interface value) that may be empty.
        if let Type::NamedOpt(n) | Type::LentNamedOpt(n) = t.ty {
            let lent = matches!(t.ty, Type::LentNamedOpt(_));
            let inner = self.resolve_name(&ast::TypeExpr { ty: if lent { Type::LentNamed(n) } else { Type::Named(n) }, span: t.span });
            return match inner {
                Ok(Type::Class(c)) => Ok(Type::OptClass(c)),
                Ok(Type::Iface(i)) => Ok(Type::OptIface(i)),
                Ok(Type::LentClass(c)) => Ok(Type::LentOptClass(c)),
                Ok(Type::LentIface(i)) => Ok(Type::LentOptIface(i)),
                Ok(Type::Enum(e)) if !lent => Ok(Type::OptEnum(e)),
                Ok(other) => Err(err(format!("{other} cannot be null"), t.span)
                    .with_note("a struct is never null; make its fields `?` instead")),
                Err(e) => Err(e),
            };
        }
        if let Type::LentNamed(n) = t.ty {
            if let Some(i) = self.ifaces.get(n) {
                return Ok(Type::LentIface(i.name));
            }
            return match self.classes.get(n) {
                Some(c) => Ok(Type::LentClass(c.name)),
                None => Err(err(format!("`&{n}`: `{n}` is not a class or interface"), t.span)
                    .with_note("`&` borrows arrays and objects; numbers, texts and structs are copied")),
            };
        }
        let Type::Named(n) = t.ty else { return Ok(t.ty) };
        if let Some(c) = self.classes.get(n) {
            if let Some(msg) = self.deprecated_classes.get(c.name)
                && self.class != Some(c.name)
            {
                self.warn(format!("class `{n}` is deprecated"), t.span, msg.clone());
            }
            return Ok(Type::Class(c.name));
        }
        if let Some(i) = self.ifaces.get(n) {
            return Ok(Type::Iface(i.name));
        }
        if let Some(info) = self.enums.get(n) {
            return Ok(Type::Enum(info.name));
        }
        if let Some((&k, _)) = self.structs.get_key_value(n) {
            return Ok(Type::Struct(k));
        }
        Err(err(format!("unknown type `{n}`"), t.span)
            .with_note("types are trit, tryte, int, real, text, arrays (`x[]: int`), or a declared enum or struct"))
    }

    fn declare_class_fields(&mut self, c: &ast::ClassDecl) -> R<()> {
        let mut fields: Vec<(String, Type, bool)> = Vec::new();
        for f in &c.fields {
            if fields.iter().any(|(n, _, _)| *n == f.name.name) {
                return Err(err(format!("field `{}` appears twice in class `{}`", f.name.name, c.name.name), f.name.span));
            }
            let ty = self.resolve(&f.ty)?;
            fields.push((f.name.name.clone(), ty, f.public));
        }
        let w: usize = fields.iter().map(|(_, t, _)| width(&self.structs, *t)).sum();
        if w > MAX_ELEM_WIDTH {
            return Err(err(format!("class `{}` has {w} fields (counting nested ones); a class can have at most {MAX_ELEM_WIDTH}", c.name.name), c.name.span));
        }
        self.classes.get_mut(&c.name.name).expect("registered").fields = fields;
        Ok(())
    }

    /// An object holding an object of its own class could never be created
    /// (there is no null to start from).
    fn class_cycle(&self, name: &str, path: &mut Vec<String>, span: Span) -> R<()> {
        if let Some(at) = path.iter().position(|p| p == name) {
            let mut cycle = path[at..].to_vec();
            cycle.push(name.to_string());
            return Err(err(format!("class `{name}` contains itself"), span).with_note(format!(
                "{}: such an object could never be created; keep the others in an array instead",
                cycle.join(" -> ")
            )));
        }
        path.push(name.to_string());
        for (_, t, _) in &self.classes[name].fields {
            if let Type::Class(inner) = t {
                self.class_cycle(inner, path, span)?;
            }
        }
        path.pop();
        Ok(())
    }

    /// Each method becomes a proc `Class.method`; `self` is its first parameter.
    fn declare_methods(&mut self, c: &ast::ClassDecl) -> R<Vec<(&'static str, ast::ProcDecl)>> {
        let class = self.classes[&c.name.name].name;
        for a in &c.annotations {
            match a.name.name.as_str() {
                "impl" if a.args.is_empty() => {
                    return Err(err("`@impl` names the interfaces: `@impl(Desenhavel)`", a.name.span));
                }
                "impl" => {}
                "deprecated" => {
                    self.deprecated_classes.insert(class, a.text.clone());
                }
                _ => {
                    return Err(err(format!("unknown annotation `@{}` on a class", a.name.name), a.name.span)
                        .with_note("a class takes `@impl(Interface)` and `@deprecated`"));
                }
            }
        }
        let mut out = Vec::new();
        for m in &c.methods {
            let mname = &m.decl.name;
            if let Some(a) = m.annotations.iter().find(|a| !matches!(a.name.name.as_str(), "override" | "deprecated") || !a.args.is_empty()) {
                let note = if a.name.name == "test" {
                    "a test is a proc outside the classes: `@test proc nome() { ... }`"
                } else {
                    "a method takes `@override` (when it fills in an interface method) and `@deprecated`"
                };
                return Err(err(format!("unknown annotation `@{}` on a method", a.name.name), a.name.span).with_note(note));
            }
            // `toString`, `hashCode` and `equals` come from `Object`.
            if crate::object::METHODS.contains(&mname.name.as_str()) && !m.annotations.iter().any(|a| a.name.name == "override") {
                return Err(err(format!("`{}` comes from `Object`; mark it `@override`", mname.name), mname.span)
                    .with_note("every class has `toString`, `hashCode` and `equals`; writing one replaces the default"));
            }
            if self.classes[class].methods.contains_key(&mname.name) {
                return Err(err(format!("method `{}` is defined twice in `{class}`", mname.name), mname.span));
            }
            if self.classes[class].fields.iter().any(|(n, _, _)| *n == mname.name) {
                return Err(err(format!("`{}` is both a field and a method of `{class}`", mname.name), mname.span));
            }
            let mut decl = m.decl.clone();
            decl.name.name = format!("{class}.{}", mname.name);
            if let Some(mode) = decl.this {
                let ty = if mode == ast::SelfMode::Lent { Type::LentNamed(class) } else { Type::Named(class) };
                let sp = mname.span;
                let this = ast::Param { name: ast::Ident { name: "self".into(), span: sp }, ty: ast::TypeExpr { ty, span: sp }, mode: Some(mode) };
                decl.params.insert(0, this);
            }
            // The class may name itself in its methods without a warning.
            self.class = Some(class);
            let declared = self.declare_proc(&decl);
            self.class = None;
            declared?;
            let id = self.proc_ids[&decl.name.name];
            // `Object` methods have fixed signatures.
            let sig = &self.sigs[id];
            let int = Type::Scalar(Scalar::Int);
            let ok = match mname.name.as_str() {
                "toString" => Some(sig.params.len() == 1 && sig.ret == Type::Text),
                "hashCode" => Some(sig.params.len() == 1 && sig.ret == int),
                "equals" => Some(sig.params.len() == 2 && sig.params[1].1 == Type::LentClass(class) && sig.ret == Type::Bool),
                _ => None,
            };
            if ok == Some(false) || (ok.is_some() && m.decl.this != Some(ast::SelfMode::Lent)) {
                let want = match mname.name.as_str() {
                    "toString" => "proc toString(&self) -> text".to_string(),
                    "hashCode" => "proc hashCode(&self) -> int".to_string(),
                    _ => format!("proc equals(&self, other: &{class}) -> bool"),
                };
                return Err(err(format!("`{}` must be `{want}`", mname.name), mname.span).with_note("it replaces the method every class has"));
            }
            // A method with the class's name is its constructor.
            let base = ast::short_name(class);
            if mname.name == base && (m.decl.this.is_some() || self.sigs[id].ret != Type::Class(class)) {
                return Err(err(format!("`{base}` is the constructor: it takes no `self` and returns `{class}`"), mname.span)
                    .with_note(format!("`public proc {base}(...) -> {class} {{ return {class} {{ ... }}; }}`")));
            }
            if let Some(a) = m.annotations.iter().find(|a| a.name.name == "deprecated") {
                self.deprecated.insert(id, a.text.clone());
            }
            let method = Method { id, this: m.decl.this, public: m.public, iface: None };
            self.classes.get_mut(class).expect("registered").methods.insert(mname.name.clone(), method);
            out.push((class, decl));
        }
        Ok(out)
    }

    /// Records an interface's methods. Each default method becomes a proc
    /// `I.m` whose `self` is `lent I`; they are returned to be checked later.
    fn declare_iface(&mut self, d: &ast::InterfaceDecl) -> R<Vec<ast::ProcDecl>> {
        let iname = self.ifaces[&d.name.name].name;
        let mut methods: Vec<IfaceMethod> = Vec::new();
        let mut defaults = Vec::new();
        for (m, &has_body) in d.methods.iter().zip(&d.bodies) {
            if methods.iter().any(|x| x.name == m.name.name) {
                return Err(err(format!("method `{}` appears twice in interface `{}`", m.name.name, d.name.name), m.name.span));
            }
            if m.this.is_none() {
                return Err(err(format!("interface method `{}` needs `self`", m.name.name), m.name.span)
                    .with_note("an interface describes what objects do: `proc area(&self) -> real;`"));
            }
            let mut params = Vec::new();
            for p in &m.params {
                let ty = self.resolve(&p.ty)?;
                if p.mode == Some(ast::SelfMode::Lent) && !is_lent(ty) {
                    return Err(err("an interface method cannot take a value with `&` yet", p.name.span)
                        .with_note("return the new value instead"));
                }
                params.push((p.name.name.clone(), ty));
            }
            let ret = match &m.ret {
                Some(t) => self.resolve(t)?,
                None => Type::Void,
            };
            let mut default = None;
            if has_body {
                if m.this != Some(ast::SelfMode::Lent) {
                    return Err(err(format!("default method `{}` must take `&self`", m.name.name), m.name.span)
                        .with_note("it works on objects of every class that implements the interface; it cannot consume them"));
                }
                let mut decl = m.clone();
                decl.name.name = format!("{iname}.{}", m.name.name);
                let sp = m.name.span;
                let this = ast::Param {
                    name: ast::Ident { name: "self".into(), span: sp },
                    ty: ast::TypeExpr { ty: Type::LentNamed(iname), span: sp },
                    mode: Some(ast::SelfMode::Lent),
                };
                decl.params.insert(0, this);
                self.declare_proc(&decl)?;
                default = Some(self.proc_ids[&decl.name.name]);
                defaults.push(decl);
            }
            methods.push(IfaceMethod { name: m.name.name.clone(), this: m.this, params, ret, default });
        }
        // Every interface has the `Object` methods (every class has them).
        let lent = Some(ast::SelfMode::Lent);
        let object = [
            ("toString", vec![], Type::Text),
            ("hashCode", vec![], Type::Scalar(Scalar::Int)),
            ("equals", vec![("other".to_string(), Type::LentIface(iname))], Type::Bool),
        ];
        for (n, params, ret) in object {
            if !methods.iter().any(|m| m.name == n) {
                methods.push(IfaceMethod { name: n.to_string(), this: lent, params, ret, default: None });
            }
        }
        self.ifaces.get_mut(&d.name.name).expect("registered").methods = methods;
        Ok(defaults)
    }

    /// `@impl(I)`: the class has every method of `I` with the same signature,
    /// each marked `@override`; and every `@override` fills in some interface.
    fn check_impls(&mut self, c: &ast::ClassDecl) -> R<()> {
        let class = self.classes[&c.name.name].name;
        let mut filled: Vec<String> = Vec::new();
        for a in &c.annotations {
            for i in &a.args {
                let Some(iface) = self.ifaces.get(&i.name) else {
                    return Err(err(format!("`{}` is not an interface", i.name), i.span));
                };
                let iname = iface.name;
                for im in iface.methods.clone() {
                    // Not written in the class: the interface's default method fills it in.
                    if !self.classes[class].methods.contains_key(&im.name)
                        && let Some(id) = im.default
                    {
                        let m = Method { id, this: Some(ast::SelfMode::Lent), public: true, iface: Some(iname) };
                        self.classes.get_mut(class).expect("registered").methods.insert(im.name.clone(), m);
                        continue;
                    }
                    // `equals` seen through `I` compares with any `I`: the class's `equals$I`.
                    let cname = if im.name == "equals" || (iname == crate::object::COMPARABLE && im.name == "compareTo") {
                        format!("{}${iname}", im.name)
                    } else {
                        im.name.clone()
                    };
                    // `Comparable`: the class compares with itself, `compareTo(&self, other: &C) -> int`.
                    if cname.starts_with("compareTo$") {
                        let want = format!("@override public proc compareTo(&self, other: &{class}) -> int");
                        let Some(own) = self.classes[class].methods.get("compareTo").copied() else {
                            return Err(err(format!("`{class}` implements `Comparable` but has no method `compareTo`"), c.name.span)
                                .with_note(format!("add `{want}`")));
                        };
                        let sig = &self.sigs[own.id];
                        let decl = c.methods.iter().find(|d| d.decl.name.name == "compareTo").expect("declared");
                        let int = Type::Scalar(Scalar::Int);
                        if own.this != Some(ast::SelfMode::Lent) || sig.params.len() != 2 || sig.params[1].1 != Type::LentClass(class) || sig.ret != int {
                            return Err(err(format!("`{class}.compareTo` must be `{want}`"), decl.decl.name.span));
                        }
                        if !own.public || !decl.annotations.iter().any(|a| a.name.name == "override") {
                            return Err(err("`compareTo` comes from `Comparable`; mark it `@override public`", decl.decl.name.span));
                        }
                        filled.push("compareTo".to_string());
                    }
                    let Some(m) = self.classes[class].methods.get(&cname).copied() else {
                        return Err(err(format!("`{class}` implements `{iname}` but has no method `{}`", im.name), c.name.span)
                            .with_note(format!("add `@override public proc {}(...)`", im.name)));
                    };
                    let sig = &self.sigs[m.id];
                    let params: Vec<Type> = sig.params.iter().skip(1).map(|(_, t)| *t).collect();
                    let want: Vec<Type> = im.params.iter().map(|(_, t)| *t).collect();
                    let decl = c.methods.iter().find(|d| d.decl.name.name == cname).expect("declared");
                    if m.this != im.this || params != want || sig.ret != im.ret {
                        return Err(err(format!("`{class}.{}` does not match `{iname}.{}`", im.name, im.name), decl.decl.name.span)
                            .with_note("the `self`, the parameter types and the return type must be the same"));
                    }
                    if !m.public {
                        return Err(err(format!("`{class}.{}` must be public: it fills in `{iname}`", im.name), decl.decl.name.span));
                    }
                    if !decl.annotations.iter().any(|a| a.name.name == "override") {
                        return Err(err(format!("`{}` comes from `{iname}`; mark it `@override`", im.name), decl.decl.name.span));
                    }
                    filled.push(cname);
                }
                self.ifaces.get_mut(&i.name).expect("checked").impls.push(class);
            }
        }
        for m in &c.methods {
            if let Some(a) = m.annotations.iter().find(|a| a.name.name == "override")
                && !filled.contains(&m.decl.name.name)
                && !crate::object::METHODS.contains(&m.decl.name.name.as_str())
            {
                return Err(err(format!("`{}` overrides nothing", m.decl.name.name), a.name.span)
                    .with_note(format!("no interface in `@impl(...)` of `{class}` has a method `{}`", m.decl.name.name)));
            }
        }
        Ok(())
    }

    /// Field `f` of class `c`: offset and type. Private fields only inside `c`.
    fn class_field(&self, c: &str, f: &ast::Ident) -> R<(usize, Type)> {
        let info = &self.classes[c];
        let mut off = 0;
        for (n, t, public) in &info.fields {
            if *n == f.name {
                if !public && self.class != Some(info.name) {
                    return Err(err(format!("`{}` is private in `{c}`", f.name), f.span)
                        .with_note(format!("only `{c}`'s methods can use it; mark it `public` to allow")));
                }
                return Ok((off, *t));
            }
            off += width(&self.structs, *t);
        }
        let names: Vec<&str> = info.fields.iter().map(|(n, _, _)| n.as_str()).collect();
        Err(err(format!("class `{c}` has no field `{}`", f.name), f.span).with_note(format!("its fields are {}", names.join(", "))))
    }

    /// `arr[idx]`. An object stays in the array: it is read lent, through
    /// its pointer cell (like a field holding an object).
    fn element(arr: HExpr, idx: HExpr, ty: Type, span: Span) -> HExpr {
        match ty {
            Type::Class(_) | Type::Iface(_) => hx(HExprKind::ElemField(Box::new(arr), Box::new(idx), 0), Self::view(ty), span),
            _ => hx(HExprKind::Index(Box::new(arr), Box::new(idx)), ty, span),
        }
    }

    /// How a field looks when read through its object: something it owns is lent.
    fn view(ty: Type) -> Type {
        match ty {
            Type::Own(b) => Type::Lent(b),
            Type::Class(n) => Type::LentClass(n),
            Type::Iface(n) => Type::LentIface(n),
            Type::OptClass(n) => Type::LentOptClass(n),
            Type::OptIface(n) => Type::LentOptIface(n),
            t => t,
        }
    }

    /// Whether `e` is an object (for `e.field`), without reporting errors.
    fn is_object(&mut self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Var(n) => self.lookup(n).is_some_and(|id| matches!(self.f.locals[id].ty, Type::Class(_) | Type::LentClass(_))),
            _ => self.expr(e, None).is_ok_and(|h| matches!(h.ty, Type::Class(_) | Type::LentClass(_))),
        }
    }

    /// The object `e` refers to, for `e.field` and `e.method()`: a local, or a
    /// field holding an object. A new object (`f().x`) would be lost.
    fn object(&mut self, e: &Expr) -> R<Option<(HExpr, &'static str)>> {
        let h = match &e.kind {
            ExprKind::Var(n) if self.lookup(n).is_some_and(|id| matches!(self.f.locals[id].ty, Type::Class(_) | Type::LentClass(_))) => {
                let id = self.lookup(n).expect("checked");
                hx(HExprKind::Local(id), self.f.locals[id].ty, e.span)
            }
            ExprKind::Var(n) if self.lookup(n).is_none() && (self.enums.contains_key(n) || self.classes.contains_key(n)) => return Ok(None),
            _ => self.expr(e, None)?,
        };
        let (Type::Class(c) | Type::LentClass(c)) = h.ty else { return Ok(None) };
        // A call that returns a borrow (`contas.get(0).saldo`) loses nothing.
        let lent_call = is_lent(h.ty) && matches!(h.kind, HExprKind::Call(..));
        if !matches!(h.kind, HExprKind::Local(_) | HExprKind::ElemField(..)) && !lent_call {
            return Err(err("the object made here would be lost", e.span).with_note("store it in a variable first, then use it and burn it"));
        }
        Ok(Some((h, c)))
    }

    fn method(&self, c: &str, name: &ast::Ident) -> R<Method> {
        let info = &self.classes[c];
        let Some(m) = info.methods.get(&name.name) else {
            let mut names: Vec<&str> = info.methods.keys().map(String::as_str).collect();
            names.sort_unstable();
            let note = if names.is_empty() { "it has no methods".to_string() } else { format!("its methods are {}", names.join(", ")) };
            return Err(err(format!("class `{c}` has no method `{}`", name.name), name.span).with_note(note));
        };
        if !m.public && self.class != Some(info.name) {
            return Err(err(format!("method `{}` is private in `{c}`", name.name), name.span)
                .with_note(format!("only `{c}`'s methods can call it; mark it `public` to allow")));
        }
        Ok(*m)
    }

    /// `Conta { saldo: 0.0, ... }`: only inside the class, every field given.
    fn new_object(&mut self, name: &ast::Ident, fields: &[(ast::Ident, Expr)], span: Span) -> R<HExpr> {
        let c = self.classes[&name.name].name;
        if self.class != Some(c) {
            return Err(err(format!("`{c}` objects are created inside the class"), name.span)
                .with_note(format!("call its constructor, `{c}(...)`, or another static method")));
        }
        let decl: Vec<(String, Type)> = self.classes[c].fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect();
        if decl.is_empty() {
            return Err(err(format!("`{c}` has no fields, so it cannot be created"), name.span));
        }
        for (i, (f, _)) in fields.iter().enumerate() {
            if !decl.iter().any(|(n, _)| *n == f.name) {
                self.class_field(c, f)?;
            }
            if fields[..i].iter().any(|(g, _)| g.name == f.name) {
                return Err(err(format!("field `{}` is given twice", f.name), f.span));
            }
        }
        let missing: Vec<String> = decl.iter().filter(|(n, _)| !fields.iter().any(|(f, _)| f.name == *n)).map(|(n, _)| format!("`{n}`")).collect();
        if !missing.is_empty() {
            let s = if missing.len() == 1 { "" } else { "s" };
            return Err(err(format!("missing field{s} {} in `{c}`", missing.join(", ")), span).with_note("every field must get a value"));
        }
        let mut values = Vec::new();
        for (n, t) in &decl {
            let (_, e) = fields.iter().find(|(f, _)| f.name == *n).expect("checked above");
            let h = match (t, &e.kind) {
                (Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_), ExprKind::Give(x)) => {
                    let lid = self.lookup(&x.name).ok_or_else(|| self.unknown(&x.name, x.span))?;
                    let h = self.give(lid, e.span)?;
                    self.coerce(h, *t)?
                }
                _ => self.expect(e, *t)?,
            };
            values.push(h);
        }
        Ok(hx(HExprKind::NewObj(values), Type::Class(c), span))
    }

    /// `obj.method(args)` or `Class.method(args)`.
    fn method_call(&mut self, recv: &Expr, name: &ast::Ident, args: &[Expr], span: Span) -> R<HExpr> {
        if let ExprKind::Var(c) = &recv.kind
            && self.lookup(c).is_none()
            && self.classes.contains_key(c)
        {
            let m = self.method(c, name)?;
            if m.this.is_some() {
                return Err(err(format!("`{}` needs an object", name.name), name.span)
                    .with_note(format!("call it on one: `x.{}(...)`", name.name)));
            }
            return self.call_proc(m.id, &format!("{c}.{}", name.name), args, None, span);
        }
        // `x.toString()`, `x.hashCode()`, `x.equals(&y)` on numbers, texts,
        // bools, enums and structs (the `Object` methods every type has).
        if matches!(name.name.as_str(), "toString" | "hashCode" | "equals" | "compareTo")
            && let Some(h) = self.value_recv(recv)?
        {
            return self.value_method(h, name, args, span);
        }
        if let Some(this) = self.iface_recv(recv)? {
            let Type::LentIface(i) = this.ty else { unreachable!("iface_recv gives lent interfaces") };
            let info = &self.ifaces[i];
            let Some(m) = info.methods.iter().find(|m| m.name == name.name).cloned() else {
                let names: Vec<&str> = info.methods.iter().map(|m| m.name.as_str()).collect();
                return Err(err(format!("interface `{i}` has no method `{}`", name.name), name.span)
                    .with_note(format!("its methods are {}", names.join(", "))));
            };
            if m.this == Some(ast::SelfMode::Own) {
                return Err(err(format!("`{}` consumes the object (`self`)", name.name), recv.span)
                    .with_note("an object called through an interface is borrowed and cannot be given away"));
            }
            let hargs = self.call_args(&format!("{i}.{}", name.name), &m.params, args, Some(this), span)?;
            let id = self.dispatcher(i, &name.name, &m.params, m.ret);
            return Ok(hx(HExprKind::Call(id, hargs), m.ret, span));
        }
        if let ExprKind::Var(n) = &recv.kind
            && let Some(id) = self.lookup(n)
            && self.f.locals[id].ty.is_opt()
        {
            return Err(self.maybe_empty(self.f.locals[id].ty, recv.span));
        }
        let Some((obj, c)) = self.object(recv)? else {
            if let Ok(h) = self.expr(recv, None)
                && h.ty.is_opt()
            {
                return Err(self.maybe_empty(h.ty, recv.span));
            }
            let ty = self.expr(recv, None).map(|h| h.ty.to_string()).unwrap_or_else(|_| "this".into());
            return Err(err(format!("{ty} has no methods"), recv.span));
        };
        let m = self.method(c, name)?;
        let this = match (m.this, &obj.kind) {
            (None, _) => {
                return Err(err(format!("`{}` is static", name.name), name.span)
                    .with_note(format!("call it on the class: `{c}.{}(...)`", name.name)));
            }
            (Some(ast::SelfMode::Lent), HExprKind::Local(id)) => hx(HExprKind::Lend(*id), Type::LentClass(c), recv.span),
            (Some(ast::SelfMode::Lent), _) => HExpr { ty: Type::LentClass(c), ..obj },
            (Some(ast::SelfMode::Own), HExprKind::Local(id)) if matches!(obj.ty, Type::Class(_)) => {
                hx(HExprKind::Give(*id), Type::Class(c), recv.span)
            }
            (Some(ast::SelfMode::Own), _) => {
                return Err(err(format!("`{}` consumes the object (`self`)", name.name), recv.span)
                    .with_note("call it on a variable you own; a borrowed object or a field cannot be given away"));
            }
        };
        // A default method of an interface takes the object as that interface.
        let this = match m.iface {
            Some(i) => hx(HExprKind::ToIface(Box::new(this), self.class_tags[c]), Type::LentIface(i), recv.span),
            None => this,
        };
        self.call_proc(m.id, &format!("{c}.{}", name.name), args, Some(this), span)
    }

    /// The receiver of `x.m()` when `x` is an interface value, lent to the
    /// call: a variable, an element `v[i]` or a field. `None` otherwise.
    fn iface_recv(&mut self, recv: &Expr) -> R<Option<HExpr>> {
        let h = match &recv.kind {
            ExprKind::Var(n) => match self.lookup(n) {
                Some(id) => match self.f.locals[id].ty {
                    Type::LentIface(i) | Type::Iface(i) => hx(HExprKind::Lend(id), Type::LentIface(i), recv.span),
                    _ => return Ok(None),
                },
                None => return Ok(None),
            },
            _ => self.expr(recv, None)?,
        };
        match h.ty {
            Type::LentIface(_) => Ok(Some(h)),
            Type::Iface(_) => Err(err("the object made here would be lost", recv.span).with_note("store it in a variable first, then use it and burn it")),
            _ => Ok(None),
        }
    }

    fn declare_struct(&mut self, s: &ast::StructDecl) -> R<()> {
        if s.fields.is_empty() {
            return Err(err(format!("struct `{}` needs at least one field", s.name.name), s.name.span));
        }
        let mut fields: Vec<(String, Type)> = Vec::new();
        for (f, t) in &s.fields {
            if fields.iter().any(|(n, _)| *n == f.name) {
                return Err(err(format!("field `{}` appears twice in struct `{}`", f.name, s.name.name), f.span));
            }
            let ty = self.resolve(t)?;
            if let Type::Class(c) | Type::Iface(c) | Type::OptClass(c) | Type::OptIface(c) = ty {
                return Err(err(format!("a struct cannot hold an object (`{c}`)"), t.span)
                    .with_note("a struct is copied, and an object has one owner; use a class instead"));
            }
            fields.push((f.name.clone(), ty));
        }
        *self.structs.get_mut(s.name.name.as_str()).expect("names come first") = fields;
        Ok(())
    }

    /// The slots a struct takes, each struct visited once (`widths` keeps
    /// the ones already done). A struct cannot hold itself, not even through
    /// another struct: it would never end. A struct can also not take more
    /// slots than a proc has.
    fn struct_width(&self, name: &str, widths: &mut HashMap<String, usize>, path: &mut Vec<String>, span: Span) -> R<usize> {
        if let Some(&w) = widths.get(name) {
            return Ok(w);
        }
        if let Some(at) = path.iter().position(|p| p == name) {
            let mut cycle = path[at..].to_vec();
            cycle.push(name.to_string());
            return Err(err(format!("struct `{name}` contains itself"), span)
                .with_note(format!("{}: it would never end; keep the others in an array and store an index", cycle.join(" -> "))));
        }
        path.push(name.to_string());
        let mut w = 0usize;
        for (_, t) in &self.structs[name] {
            w = w.saturating_add(match t {
                Type::Struct(inner) => self.struct_width(inner, widths, path, span)?,
                _ => 1,
            });
        }
        path.pop();
        if w > MAX_LOCALS {
            return Err(err(format!("struct `{name}` is too big: it takes more than {MAX_LOCALS} slots"), span)
                .with_note("each field takes a slot, and a nested struct takes one per field of its own"));
        }
        widths.insert(name.to_string(), w);
        Ok(w)
    }

    /// The offset (in slots) and type of field `f` of struct `sname`.
    fn field(&self, sname: &str, f: &ast::Ident) -> R<(usize, Type)> {
        let mut off = 0;
        for (n, t) in &self.structs[sname] {
            if *n == f.name {
                return Ok((off, *t));
            }
            off += width(&self.structs, *t);
        }
        let names: Vec<&str> = self.structs[sname].iter().map(|(n, _)| n.as_str()).collect();
        Err(err(format!("struct `{sname}` has no field `{}`", f.name), f.span).with_note(format!("its fields are {}", names.join(", "))))
    }

    /// Field at `off` of the struct value `base`. Fields are read from a
    /// local, so a struct that is not in one (`f().x`) is copied to a hidden one.
    fn field_of(&mut self, base: HExpr, off: usize, ty: Type, span: Span) -> HExpr {
        match base.kind {
            HExprKind::Local(id) => hx(HExprKind::Field(id, off), ty, span),
            HExprKind::Field(id, o) => hx(HExprKind::Field(id, o + off), ty, span),
            HExprKind::Bind(tmp, value, body) => {
                let body = self.field_of(*body, off, ty, span);
                hx(HExprKind::Bind(tmp, value, Box::new(body)), ty, span)
            }
            // `pts[i].x`: read just that field of the element.
            HExprKind::Index(arr, idx) if arr.ty != Type::Text => hx(HExprKind::ElemField(arr, idx, off), ty, span),
            HExprKind::ElemField(a, idx, o) => hx(HExprKind::ElemField(a, idx, o + off), ty, span),
            _ => {
                let tmp = self.new_local("$struct", base.ty, LocalKind::Hidden, span);
                let read = hx(HExprKind::Field(tmp, off), ty, span);
                hx(HExprKind::Bind(tmp, Box::new(base), Box::new(read)), ty, span)
            }
        }
    }

    /// How a value prints: an enum by its name, a struct as `Ponto { x: 1, y: 2 }`.
    fn displayed(&mut self, h: HExpr) -> HExpr {
        // A `T?` prints `null`, or its value: `x == null ? "null" : toText(x)`.
        if matches!(h.ty, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_)) {
            let span = h.span;
            let inner = h.ty.unopt();
            let tmp = self.new_local("$shown", inner, LocalKind::Hidden, span);
            let local = hx(HExprKind::Local(tmp), inner, span);
            let then = match inner {
                Type::Text => local,
                Type::Enum(_) => self.named(local),
                _ => hx(HExprKind::Builtin(Builtin::ToText(Fmt::Plain), vec![local]), Type::Text, span),
            };
            let null = hx(HExprKind::Text("null".into()), Type::Text, span);
            return hx(HExprKind::IfNull { value: Box::new(h), tmp, then: Box::new(then), null: Some(Box::new(null)) }, Type::Text, span);
        }
        if h.ty == Type::Bool {
            let span = h.span;
            return hx(HExprKind::Builtin(Builtin::ToText(Fmt::Plain), vec![h]), Type::Text, span);
        }
        // An object prints with its `toString` (the default shows every field).
        if let Type::Class(c) | Type::LentClass(c) = h.ty {
            let id = self.classes[c].methods["toString"].id;
            let span = h.span;
            let this = match h.kind {
                HExprKind::Local(l) => hx(HExprKind::Lend(l), Type::LentClass(c), span),
                _ => HExpr { ty: Type::LentClass(c), ..h },
            };
            return hx(HExprKind::Call(id, vec![this]), Type::Text, span);
        }
        if let Type::LentIface(i) = h.ty {
            let span = h.span;
            let id = self.dispatcher(i, "toString", &[], Type::Text);
            return hx(HExprKind::Call(id, vec![h]), Type::Text, span);
        }
        let Type::Struct(sname) = h.ty else { return self.named(h) };
        let span = h.span;
        let text = |s: &str| (Fmt::Plain, hx(HExprKind::Text(s.to_string()), Type::Text, span));
        let (base, bind) = match h.kind {
            HExprKind::Local(_) | HExprKind::Field(..) => (h, None),
            _ => {
                let tmp = self.new_local("$struct", h.ty, LocalKind::Hidden, span);
                (hx(HExprKind::Local(tmp), h.ty, span), Some((tmp, h)))
            }
        };
        let mut pieces = vec![text(&format!("{sname} {{ "))];
        let mut off = 0;
        for (i, (fname, fty)) in self.structs[sname].clone().into_iter().enumerate() {
            pieces.push(text(&format!("{}{fname}: ", if i > 0 { ", " } else { "" })));
            let f = self.field_of(base.clone(), off, fty, span);
            if fty == Type::Text {
                pieces.push((Fmt::Quoted, f));
            } else {
                pieces.push((Fmt::Plain, self.displayed(f)));
            }
            off += width(&self.structs, fty);
        }
        pieces.push(text(" }"));
        let print = hx(HExprKind::Print(pieces), Type::Void, span);
        match bind {
            Some((tmp, value)) => hx(HExprKind::Bind(tmp, Box::new(value), Box::new(print)), Type::Void, span),
            None => print,
        }
    }

    /// An enum value as its name (a call to the enum's hidden name proc).
    fn named(&self, h: HExpr) -> HExpr {
        let Type::Enum(n) = h.ty else { return h };
        let id = self.proc_ids[&self.enums[n].name_proc];
        let span = h.span;
        hx(HExprKind::Call(id, vec![HExpr { ty: Type::Scalar(Scalar::Int), ..h }]), Type::Text, span)
    }

    fn declare_proc(&mut self, p: &ast::ProcDecl) -> R<()> {
        let name = &p.name.name;
        if BUILTINS.contains(&name.as_str()) {
            return Err(err(format!("`{name}` is a built-in and cannot be redefined"), p.name.span));
        }
        if self.proc_ids.contains_key(name) {
            return Err(err(format!("proc `{name}` is defined twice"), p.name.span));
        }
        let mut params: Vec<(String, Type)> = Vec::new();
        let mut refs = Vec::new();
        for param in &p.params {
            if params.iter().any(|(n, _)| *n == param.name.name) {
                return Err(err(format!("parameter `{}` appears twice", param.name.name), param.name.span));
            }
            // `c: &Cor` / `p: &Ponto`: an enum or struct borrowed like a number.
            let written = match param.ty.ty {
                Type::LentNamed(n) if self.enums.contains_key(n) || self.structs.contains_key(n) => {
                    ast::TypeExpr { ty: Type::Named(n), span: param.ty.span }
                }
                Type::LentNamedOpt(n) if self.enums.contains_key(n) => ast::TypeExpr { ty: Type::NamedOpt(n), span: param.ty.span },
                _ => param.ty,
            };
            let ty = self.resolve(&written)?;
            let by_ref = param.mode == Some(ast::SelfMode::Lent) && !is_lent(ty);
            params.push((param.name.name.clone(), ty));
            refs.push(by_ref);
        }
        let ret = match &p.ret {
            Some(t) => self.resolve(t)?,
            None => Type::Void,
        };
        // `-> &T` lends part of what the proc borrowed: it needs a `&` parameter.
        if is_lent(ret)
            && let Some(t) = &p.ret
            && !params.iter().any(|(_, pt)| is_lent(*pt))
            && p.this != Some(ast::SelfMode::Lent)
        {
            return Err(err("a proc that returns something borrowed (`&`) must take a `&` parameter", t.span)
                .with_note("it can only lend what it borrowed; the result borrows from its first `&` parameter (or `&self`)"));
        }
        let id = self.sigs.len();
        for a in &p.annotations {
            match a.name.name.as_str() {
                "test" => {
                    if !params.is_empty() || ret != Type::Void || name == "main" {
                        return Err(err(format!("test `{name}` cannot take parameters or return a value"), p.name.span)
                            .with_note("a test is `@test proc nome() { ... }`, checked with `assert`"));
                    }
                    self.tests.push((name.clone(), id));
                }
                "deprecated" => {
                    self.deprecated.insert(id, a.text.clone());
                }
                _ => {
                    return Err(err(format!("unknown annotation `@{}` on a proc", a.name.name), a.name.span)
                        .with_note("a proc takes `@test` and `@deprecated`"));
                }
            }
        }
        self.proc_ids.insert(name.clone(), id);
        self.sigs.push(Sig { params, refs, ret, span: p.name.span });
        Ok(())
    }

    /// Marks `p` as not empty: a local is retyped (`No?` -> `No`) until
    /// [`Self::unnarrow_to`]; a field is looked up by [`Self::narrowed_ty`].
    fn narrow(&mut self, (p, class): &(Path, Option<&'static str>)) {
        if p.1.is_empty() {
            let now = self.f.locals[p.0].ty;
            let then = proved(now, *class);
            if then != now {
                self.f.locals[p.0].ty = then;
                self.f.narrowed.push(Some(Narrow::Local(p.0, now)));
            }
        } else {
            self.f.narrowed.push(Some(Narrow::Field(p.clone(), *class)));
        }
    }

    /// Forgets what was proved after the first `len` entries.
    fn unnarrow_to(&mut self, len: usize) {
        while self.f.narrowed.len() > len {
            if let Some(Some(Narrow::Local(id, declared))) = self.f.narrowed.pop() {
                self.f.locals[id].ty = declared;
            }
        }
    }

    /// Something may have emptied the fields under `p` (a call that lends
    /// `p`, a method of `p`): they must be checked again. `inclusive` also
    /// forgets `p` itself (`give p`).
    fn invalidate(&mut self, p: &Path, inclusive: bool) {
        for n in self.f.narrowed.iter_mut() {
            if let Some(Narrow::Field((id, fields), _)) = n
                && *id == p.0
                && fields.starts_with(&p.1)
                && (inclusive || fields.len() > p.1.len())
            {
                *n = None;
            }
        }
    }

    /// `ty` for the field read by `h`, after what checks proved about it.
    fn narrowed_ty(&self, h: &HExpr, ty: Type) -> Type {
        if !ty.is_opt() && iface_of(ty).is_none() {
            return ty;
        }
        let Some(p) = path_of(h) else { return ty };
        let mut ty = ty;
        for n in &self.f.narrowed {
            if let Some(Narrow::Field(q, class)) = n
                && *q == p
            {
                ty = proved(ty, *class);
            }
        }
        ty
    }

    /// The type a local was declared with (a checked `No?` is `No` meanwhile).
    fn declared_ty(&self, id: LocalId) -> Type {
        // The first retype of `id` kept the type it was declared with.
        self.f.narrowed.iter().find_map(|n| match n {
            Some(Narrow::Local(l, t)) if *l == id => Some(*t),
            _ => None,
        }).unwrap_or(self.f.locals[id].ty)
    }

    /// Before a loop: whatever its body may empty is not proved inside it,
    /// not even before that point (the body runs again).
    fn invalidate_loop(&mut self, s: &ast::Stmt) {
        if self.f.narrowed.iter().all(|n| !matches!(n, Some(Narrow::Field(..)))) {
            return;
        }
        let mut events: Vec<(Expr, bool)> = Vec::new();
        visit_stmt(s, &mut |e| match &e.kind {
            ExprKind::Lend(x) => events.push(((**x).clone(), false)),
            ExprKind::MethodCall { recv, .. } => events.push(((**recv).clone(), false)),
            ExprKind::GiveField(x) => events.push(((**x).clone(), true)),
            _ => {}
        });
        for (e, inclusive) in events {
            if let Some(p) = self.probe_path(&e) {
                self.invalidate(&p, inclusive);
            }
        }
    }

    /// The path `e` names (`n`, `n.proximo`), found without checking `e`.
    /// `None` for anything else, or a name not in scope yet.
    fn probe_path(&self, e: &Expr) -> Option<Path> {
        fn walk(cx: &Checker, e: &Expr) -> Option<(Path, Type)> {
            match &e.kind {
                ExprKind::Var(n) => cx.lookup(n).map(|id| ((id, Vec::new()), cx.f.locals[id].ty.unopt())),
                ExprKind::Member(b, f) => {
                    let ((id, mut fields), ty) = walk(cx, b)?;
                    let (Type::Class(c) | Type::LentClass(c)) = ty else { return None };
                    let (off, fty) = cx.class_field(c, f).ok()?;
                    fields.push(off);
                    Some(((id, fields), fty.unopt()))
                }
                _ => None,
            }
        }
        walk(self, e).map(|(p, _)| p)
    }

    /// The error for using a `T?` that was not checked.
    fn maybe_empty(&self, ty: Type, span: Span) -> Diagnostic {
        err(format!("this is {ty}: it may be null"), span)
            .with_note("check it first: `if x != null { ... }`, leave early with `if x == null { return; }`, or use `x?.m()`")
    }

    /// `e` read in place if it is an interface value (a variable, field or
    /// element): `None` if it is something else.
    fn iface_value(&mut self, e: &Expr) -> R<Option<HExpr>> {
        let h = match &e.kind {
            ExprKind::Var(n) => match self.lookup(n) {
                Some(id) => hx(HExprKind::Local(id), self.f.locals[id].ty, e.span),
                None => return Ok(None),
            },
            _ => self.expr(e, None)?,
        };
        Ok(iface_of(h.ty).map(|_| h))
    }

    /// The class `name` names, which must implement `iface`.
    fn implementer(&self, iface: &'static str, name: &ast::Ident) -> R<&'static str> {
        let Some(info) = self.classes.get(&name.name) else {
            return Err(err(format!("`{}` is not a class", name.name), name.span));
        };
        let c = info.name;
        if !self.ifaces[iface].impls.contains(&c) {
            return Err(err(format!("`{c}` does not implement `{iface}`, so this can never match"), name.span));
        }
        Ok(c)
    }

    /// `match f { Circulo => ...; Quadrado => ...; else => ...; }` on an
    /// interface value: in an arm with one class, `f` is of that class.
    fn match_class(&mut self, scrut: &Expr, h: HExpr, arms: &[ast::MatchArm], span: Span) -> R<HStmtKind> {
        let iface = iface_of(h.ty).expect("an interface value");
        if h.ty.is_opt() {
            return Err(self.maybe_empty(h.ty, scrut.span));
        }
        let Some(path) = path_of(&h) else {
            return Err(err("`match` by class works on a variable or a field", scrut.span)
                .with_note("for an array element, pass it to a proc (`f: &Forma`) and match there"));
        };
        let mut seen: Vec<&'static str> = Vec::new();
        let mut tests: Vec<(Vec<&'static str>, HBlock)> = Vec::new();
        let mut otherwise = None;
        for (i, arm) in arms.iter().enumerate() {
            if arm.pats.is_empty() {
                if i + 1 != arms.len() {
                    return Err(err("the `else` arm must be the last one", arm.span));
                }
                otherwise = Some(self.block(&arm.body)?);
                continue;
            }
            let mut classes = Vec::new();
            for p in &arm.pats {
                let ExprKind::Var(n) = &p.kind else {
                    return Err(err("a pattern here is a class name, like `Circulo`", p.span));
                };
                let c = self.implementer(iface, &ast::Ident { name: n.clone(), span: p.span })?;
                if seen.contains(&c) {
                    return Err(err(format!("`{c}` already appears in an earlier arm"), p.span));
                }
                seen.push(c);
                classes.push(c);
            }
            // With one class, the arm sees `f` as that class.
            let mark = self.f.narrowed.len();
            if let [c] = classes[..] {
                self.narrow(&(path.clone(), Some(c)));
            }
            let body = self.block(&arm.body);
            self.unnarrow_to(mark);
            tests.push((classes, body?));
        }
        if otherwise.is_none() {
            let missing: Vec<String> = self.ifaces[iface].impls.iter().filter(|c| !seen.contains(c)).map(|c| format!("`{c}`")).collect();
            if !missing.is_empty() {
                return Err(err(format!("`match` does not cover {}", missing.join(", ")), scrut.span)
                    .with_note(format!("add an arm for each class that implements `{iface}`, or an `else` arm")));
            }
        }
        // Without `else`, every arm is still tested: another file of the
        // program may have a class this one does not know.
        let mut chain = otherwise.or_else(|| {
            let trap = HStmt { kind: HStmtKind::Trap(format!("match: an object of a class no arm of this `match` lists ({iface})")), span };
            Some(HBlock { stmts: vec![trap], scope: vec![], span })
        });
        for (classes, body) in tests.into_iter().rev() {
            chain = Some(match chain {
                None => body,
                Some(rest) => {
                    let cond = classes
                        .iter()
                        .map(|c| hx(HExprKind::IsClass(Box::new(h.clone()), self.class_tags[c], c), Type::Bool, scrut.span))
                        .reduce(|a, b| hx(HExprKind::Binary(BinOp::Or, Box::new(a), Box::new(b)), Type::Bool, scrut.span))
                        .expect("an arm has at least one class");
                    let trap = HStmt { kind: HStmtKind::Trap("match test is 0 (unknown)".into()), span: scrut.span };
                    let unknown = HBlock { stmts: vec![trap], scope: vec![], span: scrut.span };
                    let tri = HStmtKind::Tri { head: cond, arms: Box::new([rest, unknown, body]), origin: Branch::Match };
                    HBlock { stmts: vec![HStmt { kind: tri, span }], scope: vec![], span }
                }
            });
        }
        Ok(HStmtKind::Block(chain.unwrap_or(HBlock { stmts: vec![], scope: vec![], span })))
    }

    /// Something assigns `target`: what was proved about it no longer holds.
    fn forget(&mut self, target: &Expr) {
        if let ExprKind::Var(n) = &target.kind
            && let Some(id) = self.lookup(n)
        {
            let declared = self.declared_ty(id);
            if declared.is_opt() && self.f.locals[id].ty != declared {
                let now = self.f.locals[id].ty;
                self.f.locals[id].ty = declared;
                self.f.narrowed.push(Some(Narrow::Local(id, now)));
            }
        } else if let Some(p) = self.probe_path(target) {
            self.invalidate(&p, true);
        }
    }

    /// After `target = value`: a value that is not null proves `target` not
    /// null (until it changes again); a `T?` value proves nothing.
    fn after_assign(&mut self, target: &Expr, nonnull: bool) {
        self.forget(target);
        if nonnull
            && let Some(p) = self.probe_path(target)
        {
            self.narrow(&(p, None));
        }
    }

    /// What a `lent` variable gets: `&path` (checked like a `lent`
    /// parameter), or `null` if it is a `T?`.
    fn lent_value(&mut self, name: &str, ty: Type, value: &Expr) -> R<HExpr> {
        match &value.kind {
            ExprKind::Lend(_) => {
                let mut h = self.call_args(&format!("&{name}"), &[(name.to_string(), ty)], std::slice::from_ref(value), None, value.span)?;
                Ok(h.pop().expect("one argument"))
            }
            ExprKind::NaT if ty.is_opt() => self.expect(value, ty),
            // Another borrow, whole: `return b;`, `atual = outro;`.
            ExprKind::Var(n) if self.lookup(n).is_some_and(|id| is_lent(self.f.locals[id].ty)) => {
                let id = self.lookup(n).expect("checked");
                let h = hx(HExprKind::Lend(id), self.f.locals[id].ty, value.span);
                self.coerce(h, ty)
            }
            // The result of a proc that returns a borrow (`contas.get(0)`).
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                let h = self.expr(value, Some(ty))?;
                if !is_lent(h.ty) {
                    return Err(err(format!("`{name}` borrows, but this call gives {}", h.ty), value.span)
                        .with_note("a call can be borrowed from only if it returns `&T`"));
                }
                self.coerce(h, ty)
            }
            _ if name == "the result" => Err(err("a `-> &T` proc returns a borrow: write `return &...;`", value.span)
                .with_note("e.g. `return &self.items[i];`")),
            _ => Err(err(format!("`{name}` borrows: give it `&...`"), value.span)
                .with_note(format!("e.g. `{name} = &lista.proximo;`"))),
        }
    }

    /// Whether `e` is surely not null (checked before it is converted to a `T?`).
    fn never_null(&mut self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::NaT | ExprKind::GiveField(_) => false,
            ExprKind::Give(n) => self.lookup(&n.name).is_some_and(|id| !self.f.locals[id].ty.is_opt()),
            ExprKind::Var(n) => self.lookup(n).is_some_and(|id| !self.f.locals[id].ty.is_opt()),
            ExprKind::Lend(x) => self.never_null(x),
            ExprKind::Member(..) | ExprKind::Index(..) => self.expr(e, None).is_ok_and(|h| !h.ty.is_opt()),
            _ => self.expr(e, None).is_ok_and(|h| !h.ty.is_opt() && h.ty != Type::Void),
        }
    }

    /// `x` read where it is (a local is not copied), to test it: `x == null`.
    fn read_in_place(&mut self, x: &Expr) -> R<HExpr> {
        if let ExprKind::Var(n) = &x.kind
            && let Some(id) = self.lookup(n)
        {
            return Ok(hx(HExprKind::Local(id), self.f.locals[id].ty, x.span));
        }
        self.expr(x, None)
    }

    /// `recv?.rest`: `rest` (a member or method call on `var`) runs only if
    /// `recv` is not null; otherwise the result is null.
    fn safe(&mut self, recv: &Expr, var: &str, rest: &Expr, span: Span) -> R<HExpr> {
        let h = match &recv.kind {
            ExprKind::Var(n) if self.lookup(n).is_some() => {
                let id = self.lookup(n).expect("checked");
                let ty = self.f.locals[id].ty;
                let kind = if matches!(ty, Type::OptClass(_) | Type::OptIface(_)) { HExprKind::Lend(id) } else { HExprKind::Local(id) };
                hx(kind, ty, recv.span)
            }
            _ => self.expr(recv, None)?,
        };
        if !h.ty.is_opt() {
            return Err(err(format!("`?.` is for values that may be null; this is {}", h.ty), recv.span)
                .with_note("write `.` instead"));
        }
        if matches!(h.ty, Type::OptClass(_) | Type::OptIface(_)) && !matches!(h.kind, HExprKind::Lend(_) | HExprKind::ElemField(..)) {
            return Err(err("the object made here would be lost", recv.span).with_note("store it in a variable first, then use it and burn it"));
        }
        // The hidden variable sees the object lent, and not null.
        let seen = Self::view(h.ty).unopt();
        let tmp = self.new_local(var, seen, LocalKind::Hidden, recv.span);
        let mut scope = Scope::default();
        scope.names.insert(var.to_string(), tmp);
        self.f.scopes.push(scope);
        let body = self.expr(rest, None);
        self.f.scopes.pop();
        let body = body?;
        let ty = match body.ty {
            Type::Void => Type::Void,
            t => t.opt().ok_or_else(|| err(format!("`?.` gives null when the left side is null, and {t} cannot be null"), rest.span))?,
        };
        let then = HExpr { ty, ..body };
        Ok(hx(HExprKind::IfNull { value: Box::new(h), tmp, then: Box::new(then), null: None }, ty, span))
    }

    /// `recv` if it is a value (not an array, object or interface value),
    /// read where it is.
    fn value_recv(&mut self, recv: &Expr) -> R<Option<HExpr>> {
        if let ExprKind::Var(n) = &recv.kind
            && self.lookup(n).is_none()
        {
            return Ok(None);
        }
        let Ok(h) = self.read_in_place(recv) else { return Ok(None) };
        let value = matches!(h.ty.unopt(), Type::Scalar(_) | Type::Bool | Type::Text | Type::Enum(_) | Type::Struct(_));
        Ok(value.then_some(h))
    }

    /// The `Object` methods of a value.
    fn value_method(&mut self, h: HExpr, name: &ast::Ident, args: &[Expr], span: Span) -> R<HExpr> {
        let want = if matches!(name.name.as_str(), "equals" | "compareTo") { 1 } else { 0 };
        if args.len() != want {
            let s = if want == 1 { "" } else { "s" };
            return Err(err(format!("`{}` takes {want} argument{s}", name.name), span));
        }
        Ok(match name.name.as_str() {
            "toString" => self.text_of(h),
            "hashCode" => self.hash_of(h),
            "compareTo" => {
                if h.ty.is_opt() {
                    return Err(self.maybe_empty(h.ty, h.span));
                }
                if !matches!(h.ty, Type::Scalar(_) | Type::Text | Type::Enum(_)) {
                    return Err(err(format!("{} has no order (`compareTo`)", h.ty), h.span)
                        .with_note("numbers, texts, enums and `Comparable` classes have one"));
                }
                let other = match &args[0].kind {
                    ExprKind::Lend(x) => self.read_in_place(x)?,
                    _ => self.expr(&args[0], Some(h.ty))?,
                };
                let other = self.coerce(other, h.ty)?;
                let int = Type::Scalar(Scalar::Int);
                if h.ty == Type::Text {
                    hx(HExprKind::Builtin(Builtin::TCmp, vec![h, other]), int, span)
                } else {
                    let order = hx(HExprKind::Binary(BinOp::Cmp3, Box::new(h), Box::new(other)), Type::Scalar(Scalar::Trit), span);
                    HExpr { ty: int, ..order }
                }
            }
            _ => {
                let other = match &args[0].kind {
                    ExprKind::Lend(x) => self.read_in_place(x)?,
                    _ => self.expr(&args[0], Some(h.ty))?,
                };
                let other = self.coerce(other, h.ty)?;
                self.eq_of(h, other)
            }
        })
    }

    /// `h` kept in a hidden local if reading it twice could repeat work.
    fn stash(&mut self, h: HExpr) -> (HExpr, Option<(LocalId, HExpr)>) {
        match h.kind {
            HExprKind::Local(_) | HExprKind::Field(..) | HExprKind::Word(_) | HExprKind::Text(_) => (h, None),
            _ => {
                let tmp = self.new_local("$obj", h.ty, LocalKind::Hidden, h.span);
                (hx(HExprKind::Local(tmp), h.ty, h.span), Some((tmp, h)))
            }
        }
    }

    fn bind(bound: Option<(LocalId, HExpr)>, body: HExpr) -> HExpr {
        match bound {
            Some((tmp, value)) => {
                let (ty, span) = (body.ty, body.span);
                hx(HExprKind::Bind(tmp, Box::new(value), Box::new(body)), ty, span)
            }
            None => body,
        }
    }

    /// `x.toString()` of a value: `42`, `true`, `Verde`, `Ponto { x: 1, y: 2 }`; a null is `null`.
    fn text_of(&mut self, h: HExpr) -> HExpr {
        let span = h.span;
        let text = |s: &str| hx(HExprKind::Text(s.to_string()), Type::Text, span);
        let cat = |a: HExpr, b: HExpr| hx(HExprKind::Binary(BinOp::Add, Box::new(a), Box::new(b)), Type::Text, span);
        match h.ty {
            Type::Text => h,
            Type::Enum(_) => self.named(h),
            Type::Struct(sname) => {
                let (base, bound) = self.stash(h);
                let mut acc = text(&format!("{sname} {{ "));
                let mut off = 0;
                for (i, (fname, fty)) in self.structs[sname].clone().into_iter().enumerate() {
                    acc = cat(acc, text(&format!("{}{fname}: ", if i > 0 { ", " } else { "" })));
                    let f = self.field_of(base.clone(), off, fty, span);
                    let shown = if fty == Type::Text { cat(cat(text("\""), f), text("\"")) } else { self.text_of(f) };
                    acc = cat(acc, shown);
                    off += width(&self.structs, fty);
                }
                Self::bind(bound, cat(acc, text(" }")))
            }
            t if t.is_opt() => {
                let inner = t.unopt();
                let tmp = self.new_local("$shown", inner, LocalKind::Hidden, span);
                let then = self.text_of(hx(HExprKind::Local(tmp), inner, span));
                hx(HExprKind::IfNull { value: Box::new(h), tmp, then: Box::new(then), null: Some(Box::new(text("null"))) }, Type::Text, span)
            }
            _ => hx(HExprKind::Builtin(Builtin::ToText(Fmt::Plain), vec![h]), Type::Text, span),
        }
    }

    /// `x.hashCode()` of a value; a null is 0.
    fn hash_of(&mut self, h: HExpr) -> HExpr {
        let span = h.span;
        let int = Type::Scalar(Scalar::Int);
        match h.ty {
            Type::Struct(sname) => {
                let (base, bound) = self.stash(h);
                let mut acc = hx(HExprKind::Word(Word::new(17)), int, span);
                let mut off = 0;
                for (_, fty) in self.structs[sname].clone() {
                    let f = self.field_of(base.clone(), off, fty, span);
                    let fh = self.hash_of(f);
                    acc = hx(HExprKind::Builtin(Builtin::HashMix, vec![acc, fh]), int, span);
                    off += width(&self.structs, fty);
                }
                Self::bind(bound, acc)
            }
            t if t.is_opt() => {
                let inner = t.unopt();
                let tmp = self.new_local("$hashed", inner, LocalKind::Hidden, span);
                let then = self.hash_of(hx(HExprKind::Local(tmp), inner, span));
                let zero = hx(HExprKind::Word(Word::ZERO), int, span);
                hx(HExprKind::IfNull { value: Box::new(h), tmp, then: Box::new(then), null: Some(Box::new(zero)) }, int, span)
            }
            _ => hx(HExprKind::Builtin(Builtin::Hash, vec![h]), int, span),
        }
    }

    /// `a.equals(&b)` of two values of the same type: field by field for a
    /// struct; two nulls are equal.
    fn eq_of(&mut self, a: HExpr, b: HExpr) -> HExpr {
        let span = a.span;
        let boolean = Type::Bool;
        let and = |x: HExpr, y: HExpr| hx(HExprKind::Binary(BinOp::And, Box::new(x), Box::new(y)), Type::Bool, span);
        match a.ty {
            Type::Struct(sname) => {
                let (x, bx) = self.stash(a);
                let (y, by) = self.stash(b);
                let mut acc = hx(HExprKind::Word(Word::ONE), boolean, span);
                let mut off = 0;
                for (_, fty) in self.structs[sname].clone() {
                    let fx = self.field_of(x.clone(), off, fty, span);
                    let fy = self.field_of(y.clone(), off, fty, span);
                    let e = self.eq_of(fx, fy);
                    acc = and(acc, e);
                    off += width(&self.structs, fty);
                }
                Self::bind(bx, Self::bind(by, acc))
            }
            t if t.is_opt() => {
                let (x, bx) = self.stash(a);
                let (y, by) = self.stash(b);
                let nat = |e: &HExpr| hx(HExprKind::Builtin(Builtin::IsNaT, vec![e.clone()]), Type::Bool, span);
                let not = |e: HExpr| hx(HExprKind::Neg(Box::new(e)), Type::Bool, span);
                let both_null = and(nat(&x), nat(&y));
                let inner = t.unopt();
                let same = self.eq_of(HExpr { ty: inner, ..x.clone() }, HExpr { ty: inner, ..y.clone() });
                let both_set = and(and(not(nat(&x)), not(nat(&y))), same);
                let either = hx(HExprKind::Binary(BinOp::Or, Box::new(both_null), Box::new(both_set)), Type::Bool, span);
                Self::bind(bx, Self::bind(by, either))
            }
            _ => hx(HExprKind::Binary(BinOp::Eq, Box::new(a), Box::new(b)), boolean, span),
        }
    }

    fn warn(&self, message: String, span: Span, note: Option<String>) {
        let mut w = Diagnostic::warning(message, span);
        if let Some(n) = note {
            w = w.with_note(n);
        }
        self.warnings.borrow_mut().push(w);
    }

    /// The proc `$dispatch.I.m` that calls method `m` of interface `i` on an
    /// object of any class: the linker writes it (it tries every class of
    /// the program that implements `i`).
    fn dispatcher(&mut self, i: &'static str, m: &str, params: &[(String, Type)], ret: Type) -> ProcId {
        let name = format!("$dispatch.{i}.{m}");
        if let Some(&id) = self.dispatchers.get(&name) {
            return id;
        }
        let id = self.sigs.len();
        let mut all = vec![("self".to_string(), Type::LentIface(i))];
        all.extend(params.iter().cloned());
        let locals = all
            .iter()
            .map(|(n, ty)| Local { name: n.clone(), ty: *ty, kind: LocalKind::Param, span: Span::default(), is_final: false })
            .collect();
        let refs = vec![false; all.len()];
        let body = HBlock { stmts: Vec::new(), scope: Vec::new(), span: Span::default() };
        self.extra.push(HProc { name: name.clone(), params: all.len(), ret, locals, body, refs: refs.clone(), link: Link::Dispatch });
        self.sigs.push(Sig { params: all, refs, ret, span: Span::default() });
        self.proc_ids.insert(name.clone(), id);
        self.dispatchers.insert(name, id);
        id
    }

    fn check_proc(&mut self, p: &ast::ProcDecl, link: Link) -> R<HProc> {
        self.current = Some(self.proc_ids[&p.name.name]);
        let sig = self.sigs[self.proc_ids[&p.name.name]].clone();
        if link == Link::Import {
            // Checked when its own file was compiled: only the signature.
            let locals = p
                .params
                .iter()
                .zip(&sig.params)
                .map(|(x, (_, ty))| Local { name: x.name.name.clone(), ty: *ty, kind: LocalKind::Param, span: x.name.span, is_final: false })
                .collect();
            let body = HBlock { stmts: Vec::new(), scope: Vec::new(), span: p.body.span };
            return Ok(HProc { name: p.name.name.clone(), params: p.params.len(), ret: sig.ret, locals, body, refs: sig.refs, link });
        }
        self.f = FnCx::new(sig.ret);
        self.f.instance = p.instance;
        let mut resized = HashSet::new();
        visit_block(&p.body, &mut |e| match &e.kind {
            ExprKind::Call { name, args } if RESIZERS.contains(&name.name.as_str()) => {
                if let Some(Expr { kind: ExprKind::Var(n), .. }) = args.first() {
                    resized.insert(n.clone());
                }
            }
            ExprKind::Lend(n) => {
                if let ExprKind::Var(n) = &n.kind {
                    resized.insert(n.clone());
                }
            }
            _ => {}
        });
        self.f.resized = resized;
        for (param, (_, ty)) in p.params.iter().zip(&sig.params) {
            self.new_local(&param.name.name, *ty, LocalKind::Param, param.name.span);
        }
        let body = self.block(&p.body)?;
        if sig.ret != Type::Void && !body.diverges() {
            let end = Span::new(p.body.span.end - 1, p.body.span.end);
            return Err(err(format!("`{}` must end with `return` (it returns {})", p.name.name, sig.ret), end));
        }
        let f = std::mem::replace(&mut self.f, FnCx::new(Type::Void));
        Ok(HProc { name: p.name.name.clone(), params: p.params.len(), ret: sig.ret, locals: f.locals, body, refs: sig.refs.clone(), link })
    }

    // ---- scopes ----

    fn new_local(&mut self, name: &str, ty: Type, kind: LocalKind, span: Span) -> LocalId {
        let id = self.f.locals.len();
        self.f.locals.push(Local { name: name.to_string(), ty, kind, span, is_final: false });
        if kind != LocalKind::Hidden {
            let scope = self.f.scopes.last_mut().expect("a scope is open");
            scope.ids.push(id);
            scope.names.insert(name.to_string(), id);
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.f.scopes.iter().rev().find_map(|s| s.names.get(name).copied())
    }

    fn unknown(&self, name: &str, span: Span) -> Diagnostic {
        if self.proc_ids.contains_key(name) {
            err(format!("`{name}` is a proc; call it with `{name}(...)`"), span)
        } else if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            err(format!("unknown name `{name}`"), span)
                .with_note(format!("if it is another file (`{name}.ks`), add `use {name};` at the top of this one"))
        } else {
            err(format!("unknown name `{name}`"), span)
        }
    }

    fn block(&mut self, b: &ast::Block) -> R<HBlock> {
        let mark = self.f.narrowed.len();
        let r = self.block_here(b);
        self.unnarrow_to(mark);
        r
    }

    fn block_here(&mut self, b: &ast::Block) -> R<HBlock> {
        self.f.scopes.push(Scope::default());
        let mut stmts: Vec<HStmt> = Vec::new();
        for s in &b.stmts {
            if let Some(prev) = stmts.last() {
                let diverged = HBlock { stmts: vec![prev.clone()], scope: vec![], span: prev.span }.diverges();
                if diverged {
                    return Err(err("unreachable code", s.span).with_note("nothing runs after `return`, `break` or `continue`"));
                }
            }
            stmts.push(self.stmt(s)?);
        }
        let scope = self.f.scopes.pop().expect("scope pushed above").ids;
        Ok(HBlock { stmts, scope, span: b.span })
    }

    // ---- statements ----

    fn stmt(&mut self, s: &ast::Stmt) -> R<HStmt> {
        if matches!(s.kind, StmtKind::While { .. } | StmtKind::For { .. } | StmtKind::Walk { .. }) {
            self.invalidate_loop(s);
        }
        // What a compound statement assigns inside may be null after it (and,
        // in a loop, before that point too): forget what was proved about it.
        if matches!(s.kind, StmtKind::If { .. } | StmtKind::Tri { .. } | StmtKind::Match { .. } | StmtKind::While { .. } | StmtKind::For { .. } | StmtKind::Walk { .. }) {
            let mut targets = Vec::new();
            assigned_in(s, &mut targets);
            for t in &targets {
                self.forget(t);
            }
        }
        let kind = match &s.kind {
            StmtKind::Let { is_final, name, ty: Some(t), value: None, .. } => {
                let ty = self.resolve(t)?;
                if *is_final {
                    return Err(err(format!("final `{}` needs a value", name.name), name.span).with_note("a final variable can never be assigned later"));
                }
                // No value: it starts as null, so its type must allow null.
                if !ty.is_opt() {
                    let note = match ty.opt() {
                        Some(o) => format!("give it a value (`{0}: {ty} = ...;`), or declare `{0}: {o};` to start with null", name.name),
                        None => format!("give it a value: `{}: {ty} = ...;` (an array can start empty: `{{}}`)", name.name),
                    };
                    return Err(err(format!("`{}` needs a value", name.name), name.span).with_note(note));
                }
                let local = self.new_local(&name.name, ty, LocalKind::Let, name.span);
                HStmtKind::Declare(local)
            }
            StmtKind::Let { value: None, .. } => unreachable!("the parser always gives `x: T;` a type"),
            StmtKind::Let { is_final, name, ty, value: Some(value), on_nat } => {
                let (value, lty) = match ty {
                    Some(t) => {
                        let ty = self.resolve(t)?;
                        if is_lent(ty) {
                            if on_nat.is_some() {
                                return Err(err("`on NaT` does not work on a borrowing variable", t.span));
                            }
                            let h = self.lent_value(&name.name, ty, value)?;
                            (h, ty)
                        } else {
                        // `on NaT` handles a null: the value may be `T?`, the variable is `T`.
                        let want = if on_nat.is_some() { ty.opt().unwrap_or(ty) } else { ty };
                        (self.expect(value, want)?, ty)
                        }
                    }
                    None => {
                        let h = self.expr(value, None)?;
                        if h.ty == Type::Void {
                            return Err(err("this expression has no value", value.span));
                        }
                        let t = h.ty;
                        (h, t)
                    }
                };
                let on_nat = match on_nat {
                    Some(b) => {
                        if width(&self.structs, value.ty) != 1 || matches!(value.ty.unopt(), Type::Own(_) | Type::Struct(_)) {
                            return Err(err(format!("`on NaT` needs a value that can be null, found {}", value.ty), value.span));
                        }
                        let hb = self.block(b)?;
                        if !hb.diverges() {
                            return Err(err("the `on NaT` block must end with `return`, `break` or `continue`", b.span)
                                .with_note("after it, the variable is guaranteed not to be NaT"));
                        }
                        Some(hb)
                    }
                    None => None,
                };
                let in_scope = self.f.scopes.last().and_then(|s| s.names.get(&name.name));
                if let Some(&prev) = in_scope
                    && self.f.locals[prev].is_final
                {
                    return Err(err(format!("`{}` is final and cannot be declared again", name.name), name.span)
                        .with_note("pick another name, or declare the first one without `final`"));
                }
                let local = self.new_local(&name.name, lty, LocalKind::Let, name.span);
                self.note_size(local, &value);
                self.f.locals[local].is_final = *is_final;
                HStmtKind::Let { local, value, on_nat }
            }
            StmtKind::Assign { target, value } => {
                let nonnull = self.never_null(value);
                let k = self.assign(target, value)?;
                self.after_assign(target, nonnull);
                k
            }
            StmtKind::Burn(name) => {
                let id = self.lookup(&name.name).ok_or_else(|| self.unknown(&name.name, name.span))?;
                match self.f.locals[id].ty {
                    Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_) => HStmtKind::Burn(id),
                    Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_) => {
                        return Err(err("cannot burn a borrowed object", name.span).with_note("only its owner can burn it"));
                    }
                    Type::Lent(_) => {
                        return Err(err("cannot burn a borrowed array", name.span).with_note("only its owner can burn it"));
                    }
                    // In a copy of a generic, `burn` of a value does nothing.
                    t if self.f.instance => {
                        let _ = t;
                        HStmtKind::Block(HBlock { stmts: Vec::new(), scope: Vec::new(), span: s.span })
                    }
                    t => return Err(err(format!("only an array or object you own can be burned, `{}` is {t}", name.name), name.span)),
                }
            }
            StmtKind::Return(v) => match (self.f.ret, v) {
                (Type::Void, None) => HStmtKind::Return(None),
                (Type::Void, Some(e)) => {
                    return Err(err("this proc has no return type, so it returns nothing", e.span).with_note("write `return;`"));
                }
                (t, None) => return Err(err(format!("this proc must return a {t}"), s.span)),
                (t @ (Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_)), Some(Expr { kind: ExprKind::Var(name), span })) => {
                    // `return b` hands ownership back to the caller.
                    let id = self.lookup(name).ok_or_else(|| self.unknown(name, *span))?;
                    let h = self.give(id, *span)?;
                    HStmtKind::Return(Some(self.coerce(h, t)?))
                }
                // `return &self.items[i];`: lends part of a `&` parameter.
                (t, Some(e)) if is_lent(t) => HStmtKind::Return(Some(self.lent_value("the result", t, e)?)),
                (t, Some(e)) => HStmtKind::Return(Some(self.expect(e, t)?)),
            },
            StmtKind::Break | StmtKind::Continue => {
                if self.f.loops == 0 {
                    let word = if s.kind == StmtKind::Break { "break" } else { "continue" };
                    return Err(err(format!("`{word}` outside of a loop"), s.span)
                        .with_note("`break` and `continue` work inside `walk` and `while`"));
                }
                if s.kind == StmtKind::Break { HStmtKind::Break } else { HStmtKind::Continue }
            }
            StmtKind::Assert { cond, msg } => {
                // `if !cond { trap }`, as a `tri`: false and 0 (unknown) fail.
                let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty.is_opt() {
                    return Err(self.maybe_empty(h.ty, h.span));
                }
                if !is_truth(h.ty) {
                    return Err(err(format!("`assert` needs a bool, found {}", h.ty), cond.span)
                        .with_note("compare, e.g. `assert(x > 0);`"));
                }
                let fail = || HBlock { stmts: vec![HStmt { kind: HStmtKind::Trap(msg.clone()), span: s.span }], scope: vec![], span: s.span };
                let ok = HBlock { stmts: vec![], scope: vec![], span: s.span };
                // After `assert(x != null)`, `x` is not null.
                let mut yes = Vec::new();
                facts(&h, true, &mut yes);
                yes.iter().for_each(|p| self.narrow(p));
                HStmtKind::Tri { head: h, arms: Box::new([fail(), fail(), ok]), origin: Branch::If }
            }
            StmtKind::If { cond, then, otherwise } => {
                let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty.is_opt() {
                    return Err(self.maybe_empty(h.ty, h.span));
                }
                if !is_truth(h.ty) {
                    return Err(err(format!("`if` needs a bool, found {}", h.ty), cond.span)
                        .with_note("compare first, e.g. `if x > 0 { ... }`"));
                }
                // `if` is a `tri` whose `0` arm traps: an unknown condition is an error.
                // `isNaT` checks in the condition prove fields and locals not empty.
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                facts(&h, true, &mut yes);
                facts(&h, false, &mut no);
                let mark = self.f.narrowed.len();
                yes.iter().for_each(|p| self.narrow(p));
                let then = self.block(then);
                self.unnarrow_to(mark);
                let then = then?;
                no.iter().for_each(|p| self.narrow(p));
                let otherwise = match otherwise {
                    Some(b) => self.block(b),
                    None => Ok(HBlock { stmts: vec![], scope: vec![], span: s.span }),
                };
                self.unnarrow_to(mark);
                let otherwise = otherwise?;
                // `if isNaT(x) { return; }`: after it, `x` is not empty (until the block ends).
                if then.diverges() {
                    no.iter().for_each(|p| self.narrow(p));
                } else if otherwise.diverges() {
                    yes.iter().for_each(|p| self.narrow(p));
                }
                let trap = HStmt { kind: HStmtKind::Trap("if condition is 0 (unknown)".into()), span: cond.span };
                let unknown = HBlock { stmts: vec![trap], scope: vec![], span: cond.span };
                HStmtKind::Tri { head: h, arms: Box::new([otherwise, unknown, then]), origin: Branch::If }
            }
            StmtKind::Tri { head, neg, zero, pos } => {
                let h = self.expr(head, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty.is_opt() {
                    return Err(self.maybe_empty(h.ty, h.span));
                }
                if !is_truth(h.ty) {
                    return Err(err(format!("`tri` needs a trit, found {}", h.ty), head.span)
                        .with_note("compare first, e.g. `tri x <=> 0 { ... }`"));
                }
                let arms = [self.block(neg)?, self.block(zero)?, self.block(pos)?];
                HStmtKind::Tri { head: h, arms: Box::new(arms), origin: Branch::Tri }
            }
            StmtKind::Match { scrut, arms } => self.match_stmt(scrut, arms, s.span)?,
            StmtKind::While { cond, body } => {
                let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty.is_opt() {
                    return Err(self.maybe_empty(h.ty, h.span));
                }
                if !is_truth(h.ty) {
                    return Err(err(format!("`while` needs a bool, found {}", h.ty), cond.span)
                        .with_note("compare first, e.g. `while i < 10 { ... }`"));
                }
                let mut yes = Vec::new();
                facts(&h, true, &mut yes);
                let mark = self.f.narrowed.len();
                yes.iter().for_each(|p| self.narrow(p));
                self.f.loops += 1;
                let body = self.block(body);
                self.f.loops -= 1;
                self.unnarrow_to(mark);
                HStmtKind::While { cond: h, body: body?, step: None }
            }
            // `for init; cond; step { body }` = `{ init; while cond { body } }` with `step`
            // after the body (and on `continue`). `init`'s variable lives only in the loop.
            StmtKind::For { init, cond, step, body } => {
                if !matches!(init.kind, StmtKind::Let { .. } | StmtKind::Assign { .. }) {
                    return Err(err("the first part of `for` must declare or assign a variable", init.span)
                        .with_note("e.g. `for i: int = 0; i < 10; i++ { ... }`"));
                }
                if !matches!(step.kind, StmtKind::Assign { .. }) {
                    return Err(err("the last part of `for` must change a variable", step.span)
                        .with_note("e.g. `i++`, `i += 2` or `i = i * 3`"));
                }
                self.f.scopes.push(Scope::default());
                let result = (|| {
                    let hinit = self.stmt(init)?;
                    let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                    if h.ty.is_opt() {
                        return Err(self.maybe_empty(h.ty, h.span));
                    }
                    if !is_truth(h.ty) {
                        return Err(err(format!("`for` needs a bool condition, found {}", h.ty), cond.span)
                            .with_note("compare, e.g. `i < 10`"));
                    }
                    self.f.loops += 1;
                    let hbody = self.block(body);
                    self.f.loops -= 1;
                    let hstep = self.stmt(step)?;
                    let step_block = HBlock { stmts: vec![hstep], scope: vec![], span: step.span };
                    let w = HStmt { kind: HStmtKind::While { cond: h, body: hbody?, step: Some(step_block) }, span: s.span };
                    Ok(vec![hinit, w])
                })();
                let scope = self.f.scopes.pop().expect("pushed above").ids;
                HStmtKind::Block(HBlock { stmts: result?, scope, span: s.span })
            }
            StmtKind::Walk { var, range, body } => {
                let word = Type::Scalar(Scalar::Int);
                let (span_tmp, start, end) = match range {
                    Range::Between(a, b) => (None, self.expect(a, word)?, self.expect(b, word)?),
                    Range::Span(k) => {
                        let hk = self.expect(k, word)?;
                        let tmp = self.new_local("$span", word, LocalKind::Hidden, k.span);
                        let local = hx(HExprKind::Local(tmp), word, k.span);
                        let start = hx(HExprKind::Neg(Box::new(local.clone())), word, k.span);
                        (Some((tmp, hk)), start, local)
                    }
                };
                let end_tmp = self.new_local("$end", word, LocalKind::Hidden, var.span);
                self.f.scopes.push(Scope::default());
                let v = self.new_local(&var.name, word, LocalKind::WalkVar, var.span);
                self.f.loops += 1;
                let body = self.block(body);
                self.f.loops -= 1;
                self.f.scopes.pop();
                HStmtKind::Walk { var: v, span_tmp, start, end, end_tmp, body: body? }
            }
            StmtKind::Expr(e) => {
                if let ExprKind::Call { name, args } = &e.kind
                    && name.name == "exit"
                {
                    let [code] = args.as_slice() else {
                        return Err(err("`exit` takes 1 argument: the exit code", e.span));
                    };
                    let h = self.expect(code, Type::Scalar(Scalar::Int))?;
                    return Ok(HStmt { kind: HStmtKind::Exit(h), span: s.span });
                }
                if let ExprKind::Call { name, args } = &e.kind
                    && name.name == "dump"
                {
                    return match args.as_slice() {
                        [Expr { kind: ExprKind::Str(label), .. }] => Ok(HStmt { kind: HStmtKind::Dump(label.clone()), span: s.span }),
                        _ => Err(err("`dump` takes one string literal: `dump(\"label\");`", e.span)),
                    };
                }
                let h = self.expr(e, None)?;
                if !matches!(h.kind, HExprKind::Call(..) | HExprKind::Builtin(..) | HExprKind::Print(_) | HExprKind::IfNull { .. }) {
                    return Err(err("this expression does nothing", e.span).with_note("only calls can be used as statements"));
                }
                if let Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_) = h.ty {
                    return Err(err("the buffer or object returned by this call would be lost", e.span)
                        .with_note("store it, e.g. `sym b[]: int = f(...);`, then burn it"));
                }
                HStmtKind::Expr(h)
            }
        };
        Ok(HStmt { kind, span: s.span })
    }

    /// `x = v` or `n.campo = v` where it is `T?`: allowed only while it is
    /// empty (checked when it runs), so no object is ever lost:
    /// `tri isNaT(target) { trap; trap; target = v }`.
    fn assign_opt(&mut self, target: &Expr, value: &Expr) -> R<Option<HStmtKind>> {
        let span = target.span;
        let zero = hx(HExprKind::Word(Word::ZERO), Type::Scalar(Scalar::Int), span);
        let (read, set) = match &target.kind {
            ExprKind::Var(name) => {
                let Some(id) = self.lookup(name) else { return Ok(None) };
                let declared = self.declared_ty(id);
                if !matches!(declared, Type::OptClass(_) | Type::OptIface(_)) {
                    return Ok(None);
                }
                if self.f.locals[id].is_final {
                    return Err(err(format!("`{name}` is final and cannot change"), span));
                }
                let hv = self.expect(value, declared)?;
                (hx(HExprKind::Local(id), declared, span), HStmtKind::Assign { local: id, index: None, field: 0, value: hv })
            }
            ExprKind::Member(base, f) => {
                let Ok(lv) = self.lvalue(base) else { return Ok(None) };
                let (Type::Class(c) | Type::LentClass(c)) = lv.ty() else { return Ok(None) };
                let (off, fty) = self.class_field(c, f)?;
                if !fty.is_opt() {
                    return Ok(None);
                }
                let obj = match lv {
                    LVal::Local { id, ty, .. } => hx(HExprKind::Local(id), ty, base.span),
                    LVal::Elem { arr, idx, off, .. } => hx(HExprKind::ElemField(Box::new(arr), Box::new(idx), off), Type::LentClass(c), base.span),
                };
                let hv = self.expect(value, fty)?;
                let read = hx(HExprKind::ElemField(Box::new(obj.clone()), Box::new(zero.clone()), off), fty, span);
                let set = match obj.kind {
                    HExprKind::Local(id) => HStmtKind::Assign { local: id, index: Some(zero), field: off, value: hv },
                    _ => HStmtKind::SetElem { arr: obj, index: zero, field: off, value: hv },
                };
                (read, set)
            }
            _ => return Ok(None),
        };
        let name = path_text(target);
        let msg = format!("`{name}` still holds an object; take it out first (`old: T? = give {name};`) and burn it");
        let fail = || HBlock { stmts: vec![HStmt { kind: HStmtKind::Trap(msg.clone()), span }], scope: vec![], span };
        let ok = HBlock { stmts: vec![HStmt { kind: set, span }], scope: vec![], span };
        let head = hx(HExprKind::Builtin(Builtin::IsNaT, vec![read]), Type::Bool, span);
        Ok(Some(HStmtKind::Tri { head, arms: Box::new([fail(), fail(), ok]), origin: Branch::If }))
    }

    /// `give n.campo`: takes the object out of a `T?` field, leaving it empty.
    fn give_field(&mut self, path: &Expr, span: Span) -> R<HExpr> {
        let ExprKind::Member(base, f) = &path.kind else { unreachable!("the parser makes `give a.b` a field path") };
        let lv = self.lvalue(base)?;
        let (Type::Class(c) | Type::LentClass(c)) = lv.ty() else {
            return Err(err(format!("{} has no fields to give away", lv.ty()), base.span));
        };
        let (off, fty) = self.class_field(c, f)?;
        if !fty.is_opt() {
            return Err(err(format!("only a field that may be empty can be given away; `{}` is {fty}", f.name), path.span)
                .with_note(format!("the object would be left without it; declare it `{}: {fty}?` to allow this", f.name)));
        }
        let obj = match lv {
            LVal::Local { id, ty, .. } => hx(HExprKind::Local(id), ty, base.span),
            LVal::Elem { arr, idx, off, .. } => hx(HExprKind::ElemField(Box::new(arr), Box::new(idx), off), Type::LentClass(c), base.span),
        };
        let zero = hx(HExprKind::Word(Word::ZERO), Type::Scalar(Scalar::Int), span);
        let read = hx(HExprKind::ElemField(Box::new(obj.clone()), Box::new(zero), off), fty, span);
        let ty = self.narrowed_ty(&read, fty);
        // From here on the field is empty again.
        if let Some(p) = path_of(&read) {
            self.invalidate(&p, true);
        }
        Ok(hx(HExprKind::TakeField(Box::new(obj), off), ty, span))
    }

    fn assign(&mut self, target: &Expr, value: &Expr) -> R<HStmtKind> {
        // `atual = &atual.proximo;`: a `lent` variable borrows something else.
        if let ExprKind::Var(n) = &target.kind
            && let Some(id) = self.lookup(n)
            && self.f.locals[id].kind == LocalKind::Let
            && is_lent(self.declared_ty(id))
        {
            let ty = self.declared_ty(id);
            let h = self.lent_value(n, ty, value)?;
            return Ok(HStmtKind::Assign { local: id, index: None, field: 0, value: h });
        }
        if let Some(k) = self.assign_opt(target, value)? {
            return Ok(k);
        }
        match self.lvalue(target)? {
            LVal::Local { id, off, ty } => {
                let local = &self.f.locals[id];
                if local.is_final {
                    return Err(err(format!("`{}` is final and cannot change", local.name), target.span)
                        .with_note("declare it without `final` if it needs to change"));
                }
                // A `T?` local checked as not null is written as declared (it may become null).
                let declared = self.declared_ty(id);
                let ty = if off == 0 && declared.is_opt() { declared } else { ty };
                let local = &self.f.locals[id];
                let whole = off == 0 && (ty == local.ty || ty == declared);
                if whole && let Type::Lent(_) | Type::LentClass(_) = ty {
                    return Err(err("cannot replace something borrowed", target.span).with_note("change what is inside it instead: `v[i] = ...`, `c.x = ...`"));
                }
                let hv = self.expect(value, ty)?;
                if whole {
                    self.f.sizes.remove(&id);
                    self.note_size(id, &hv);
                }
                Ok(HStmtKind::Assign { local: id, index: None, field: off, value: hv })
            }
            LVal::Elem { arr, idx, off, ty } => {
                if let (Type::Class(_) | Type::Iface(_), Type::Own(_) | Type::Lent(_)) = (ty, arr.ty) {
                    return Err(err("an object in an array cannot be replaced", target.span).with_note(
                        "the old one would be lost: take it out first (`old: T = remove(v, i);`, burn it), then `insert(v, i, give new)`",
                    ));
                }
                if let Type::Own(_) | Type::Class(_) | Type::Iface(_) = ty {
                    return Err(err("a field that owns an array or an object cannot be replaced", target.span)
                        .with_note("change what is inside it instead (`append(self.items, x)`, `self.items[i] = ...`)"));
                }
                let hv = self.expect(value, ty)?;
                Ok(match arr.kind {
                    HExprKind::Local(id) => HStmtKind::Assign { local: id, index: Some(idx), field: off, value: hv },
                    _ => HStmtKind::SetElem { arr, index: idx, field: off, value: hv },
                })
            }
        }
    }

    /// What the left side of `=` writes to.
    fn lvalue(&mut self, e: &Expr) -> R<LVal> {
        match &e.kind {
            ExprKind::Var(name) => {
                let Some(id) = self.lookup(name) else {
                    if self.consts.contains_key(name) {
                        return Err(err(format!("cannot assign to const `{name}`"), e.span));
                    }
                    return Err(self.unknown(name, e.span));
                };
                if self.f.locals[id].kind == LocalKind::WalkVar {
                    return Err(err(format!("`{name}` is the walk variable and is read-only"), e.span));
                }
                Ok(LVal::Local { id, off: 0, ty: self.f.locals[id].ty })
            }
            ExprKind::Member(base, f) => {
                let lv = self.lvalue(base)?;
                if let Type::Class(c) | Type::LentClass(c) = lv.ty() {
                    // A field of an object: a cell of its heap block.
                    let (off, ty) = self.class_field(c, f)?;
                    let obj = match lv {
                        LVal::Local { id, ty, .. } => hx(HExprKind::Local(id), ty, base.span),
                        LVal::Elem { arr, idx, off, .. } => {
                            hx(HExprKind::ElemField(Box::new(arr), Box::new(idx), off), Type::LentClass(c), base.span)
                        }
                    };
                    let zero = hx(HExprKind::Word(Word::ZERO), Type::Scalar(Scalar::Int), base.span);
                    let read = hx(HExprKind::ElemField(Box::new(obj.clone()), Box::new(zero.clone()), off), ty, base.span);
                    let ty = self.narrowed_ty(&read, ty);
                    return Ok(LVal::Elem { arr: obj, idx: zero, off, ty });
                }
                if lv.ty().is_opt() {
                    return Err(self.maybe_empty(lv.ty(), base.span));
                }
                let Type::Struct(sname) = lv.ty() else {
                    return Err(err(format!("{} has no fields", lv.ty()), f.span));
                };
                let (o, t) = self.field(sname, f)?;
                Ok(match lv {
                    LVal::Local { id, off, .. } => LVal::Local { id, off: off + o, ty: t },
                    LVal::Elem { arr, idx, off, .. } => LVal::Elem { arr, idx, off: off + o, ty: t },
                })
            }
            ExprKind::Index(base, idx) => {
                let lv = self.lvalue(base)?;
                match lv.ty() {
                    Type::Own(b) | Type::Lent(b) => {
                        let hidx = self.index(idx)?;
                        let arr = match lv {
                            LVal::Local { id, ty, .. } => {
                                self.check_index(id, &hidx)?;
                                hx(HExprKind::Local(id), ty, base.span)
                            }
                            LVal::Elem { arr, idx, off, ty } => hx(HExprKind::ElemField(Box::new(arr), Box::new(idx), off), ty, base.span),
                        };
                        Ok(LVal::Elem { arr, idx: hidx, off: 0, ty: b.ty() })
                    }
                    Type::Text => Err(err("text is read-only", e.span)),
                    t => Err(err(format!("cannot index a {t}"), base.span)),
                }
            }
            _ => Err(err("cannot assign to this expression", e.span)
                .with_note("the left side of `=` must be a variable, `array[index]` or a field like `p.x`")),
        }
    }

    // ---- expressions ----

    fn expect(&mut self, e: &Expr, ty: Type) -> R<HExpr> {
        let h = self.expr(e, Some(ty))?;
        self.coerce(h, ty)
    }

    fn coerce(&self, h: HExpr, ty: Type) -> R<HExpr> {
        if fits(h.ty, ty) {
            return Ok(h);
        }
        // An object (or interface value) fits where it may be empty: `No` -> `No?`.
        match (h.ty, ty) {
            (Type::Class(c), Type::OptClass(d)) if c == d => return Ok(HExpr { ty, ..h }),
            (Type::Iface(i), Type::OptIface(j)) if i == j => return Ok(HExpr { ty, ..h }),
            (Type::Class(_), Type::OptIface(i)) => {
                let as_iface = self.coerce(h, Type::Iface(i))?;
                return Ok(HExpr { ty, ..as_iface });
            }
            // A `T?` where a `T` is needed: it must be checked first.
            (from, to) if from.is_opt() && !to.is_opt() && fits(from.unopt(), to) => return Err(self.maybe_empty(from, h.span)),
            _ => {}
        }
        // An object becomes an interface value: it keeps its class number.
        if let (Type::Class(c), Type::Iface(i)) = (h.ty, ty) {
            if self.ifaces[i].impls.contains(&c) {
                let span = h.span;
                return Ok(hx(HExprKind::ToIface(Box::new(h), self.class_tags[c]), ty, span));
            }
            return Err(err(format!("`{c}` does not implement `{i}`"), h.span)
                .with_note(format!("add `@impl({i})` to the class and the methods it lists")));
        }
        let d = err(format!("expected {ty}, found {}", h.ty), h.span);
        Err(match (h.ty, ty) {
            (from, Type::Bool) if from.is_int() => d.with_note("write `true` or `false`, or compare: `x != 0`"),
            (Type::LentClass(_), Type::Class(_)) | (Type::LentIface(_), Type::Iface(_)) => d.with_note(
                "an object held in an array or a field stays there: take it out with `remove(v, i)` or `pop(v)`, or borrow it with `&`",
            ),
            (Type::Scalar(Scalar::Real), t) | (t, Type::Scalar(Scalar::Real)) if t.is_int() => {
                d.with_note(format!("convert explicitly with `as {ty}`"))
            }
            (from, to) if from.is_int() && to.is_int() => {
                d.with_note(format!("narrow explicitly with `as {to}` (values out of range become NaT)"))
            }
            _ => d,
        })
    }

    /// Remembers the size of an array made by `{...}` or `claim <literal>`.
    fn note_size(&mut self, id: LocalId, value: &HExpr) {
        if self.f.resized.contains(&self.f.locals[id].name) {
            return;
        }
        let n = match &value.kind {
            HExprKind::Pack(items) => items.len() as i64,
            HExprKind::Claim(n) => match n.kind {
                HExprKind::Word(w) if !w.is_nat() => w.get().unwrap_or(0),
                _ => return,
            },
            _ => return,
        };
        self.f.sizes.insert(id, n);
    }

    /// A literal index outside an array of known size would always trap:
    /// reject it now, like Rust does for fixed-size arrays.
    fn check_index(&self, id: LocalId, idx: &HExpr) -> R<()> {
        let (HExprKind::Word(w), Some(&n)) = (&idx.kind, self.f.sizes.get(&id)) else { return Ok(()) };
        let (Some(i), Type::Own(b) | Type::Lent(b)) = (w.get(), self.f.locals[id].ty) else { return Ok(()) };
        let (lo, hi) = if b.sym { (-(n / 2), n / 2) } else { (0, n - 1) };
        if (lo..=hi).contains(&i) {
            return Ok(());
        }
        let name = &self.f.locals[id].name;
        let range = if n == 0 { "none: it is empty".to_string() } else if b.sym { format!("{lo}..+{hi}") } else { format!("{lo}..{hi}") };
        let note = if b.sym {
            format!("`{name}` is `sym` with {n} elements, so index 0 is the middle one")
        } else {
            format!("`{name}` has {n} elements, counted from 0")
        };
        Err(err(format!("index {i} is out of bounds: valid indices of `{name}` are {range}"), idx.span).with_note(note))
    }

    fn index(&mut self, idx: &Expr) -> R<HExpr> {
        let h = self.expr(idx, Some(Type::Scalar(Scalar::Int)))?;
        if h.ty.is_opt() {
            return Err(self.maybe_empty(h.ty, h.span));
        }
        if !h.ty.is_int() {
            return Err(err(format!("an index must be an integer, found {}", h.ty), idx.span));
        }
        Ok(h)
    }

    fn give(&self, id: LocalId, span: Span) -> R<HExpr> {
        match self.f.locals[id].ty {
            Type::Own(e) => Ok(hx(HExprKind::Give(id), Type::Own(e), span)),
            Type::Class(c) => Ok(hx(HExprKind::Give(id), Type::Class(c), span)),
            t @ (Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_)) => Ok(hx(HExprKind::Give(id), t, span)),
            Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_) => Err(err("cannot give away something borrowed", span).with_note("only its owner can give it")),
            // In a copy of a generic, `give` of a value copies it.
            t if self.f.instance => Ok(hx(HExprKind::Local(id), t, span)),
            t => Err(err(format!("only an array or object you own can be given, this is {t}"), span)),
        }
    }

    fn numeric(&self, h: &HExpr) -> R<()> {
        if h.ty.is_numeric() {
            return Ok(());
        }
        if h.ty.is_opt() {
            return Err(self.maybe_empty(h.ty, h.span));
        }
        let d = err(format!("expected a number, found {}", h.ty), h.span);
        Err(match h.ty {
            Type::Text => d.with_note("texts work with `+`, `==` and `!=`; `toInt(s)` reads a number"),
            Type::Struct(_) => d.with_note("compare or compute with the fields, e.g. `a.x == b.x`"),
            _ => d,
        })
    }

    /// Checks both operands of an arithmetic or comparison operator and
    /// returns them with their common type.
    fn numeric_pair(&mut self, a: &Expr, b: &Expr, want: Option<Type>) -> R<(HExpr, HExpr, Type)> {
        let want = want.filter(|t| t.is_numeric());
        let (ha, hb) = if is_literal(a) && !is_literal(b) {
            let hb = self.expr(b, want)?;
            let ha = self.expr(a, Some(hb.ty))?;
            (ha, hb)
        } else {
            let ha = self.expr(a, want)?;
            let hb = self.expr(b, Some(ha.ty))?;
            (ha, hb)
        };
        self.numeric(&ha)?;
        self.numeric(&hb)?;
        let ty = match (ha.ty, hb.ty) {
            (Type::Scalar(x), Type::Scalar(y)) if x.is_int() && y.is_int() => {
                Type::Scalar(if x.rank() >= y.rank() { x } else { y })
            }
            (x, y) if x == y => x,
            (x, y) => {
                return Err(err(format!("cannot mix {x} and {y}"), ha.span.to(hb.span))
                    .with_note("convert one side with `(real) x` or `(int) x`"));
            }
        };
        Ok((ha, hb, ty))
    }

    fn expr(&mut self, e: &Expr, want: Option<Type>) -> R<HExpr> {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Num(w) => {
                let s = match want {
                    Some(Type::Scalar(s)) if s.is_int() => s,
                    Some(Type::Scalar(Scalar::Real)) => {
                        return Err(err("integer literal where a real is expected", span).with_note("write it with a point, e.g. `5.0`"));
                    }
                    _ => Scalar::Int,
                };
                let max = match s {
                    Scalar::Trit => TRIT_MAX,
                    Scalar::Tryte => TRYTE_MAX,
                    _ => i64::MAX,
                };
                if w.get().is_some_and(|v| v.abs() > max) {
                    return Err(err(format!("`{w}` does not fit in a {s} (-{max}..={max})"), span));
                }
                hx(HExprKind::Word(*w), Type::Scalar(s), span)
            }
            ExprKind::Real(r) => hx(HExprKind::Real(*r), Type::Scalar(Scalar::Real), span),
            ExprKind::Char(c) => hx(HExprKind::Word(kestrel_core::Word::new(*c)), Type::Scalar(Scalar::Tryte), span),
            ExprKind::Str(s) => hx(HExprKind::Text(s.clone()), Type::Text, span),
            ExprKind::Bool(b) => hx(HExprKind::Word(if *b { Word::ONE } else { Word::NEG_ONE }), Type::Bool, span),
            ExprKind::NaT => match want {
                // null is NaT inside: a NaT pointer (and class number for an interface).
                Some(Type::OptScalar(Scalar::Real)) => hx(HExprKind::Real(kestrel_core::Real::NAT), Type::OptScalar(Scalar::Real), span),
                Some(t @ (Type::OptClass(_) | Type::LentOptClass(_) | Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_))) => {
                    hx(HExprKind::Word(kestrel_core::Word::NAT), t, span)
                }
                Some(t @ (Type::OptIface(_) | Type::LentOptIface(_))) => {
                    let nat = hx(HExprKind::Word(kestrel_core::Word::NAT), Type::Scalar(Scalar::Int), span);
                    hx(HExprKind::StructLit(vec![nat.clone(), nat]), t, span)
                }
                // Inside an `on NaT` line, arithmetic may give NaT; elsewhere null needs a `?` type.
                Some(t) => {
                    return Err(err(format!("{t} is never null"), span)
                        .with_note(format!("declare it `{t}?` if it may have no value")));
                }
                None => return Err(err("`null` needs to know its type", span).with_note("e.g. `x: int? = null;`")),
            },
            ExprKind::Var(name) => {
                if let Some(id) = self.lookup(name) {
                    let ty = self.f.locals[id].ty;
                    if matches!(
                        ty,
                        Type::Class(_) | Type::LentClass(_) | Type::Iface(_) | Type::LentIface(_) | Type::OptClass(_) | Type::OptIface(_)
                    ) || matches!(ty, Type::LentOptClass(_) | Type::LentOptIface(_))
                    {
                        return Err(err(format!("object `{name}` cannot be copied"), span)
                            .with_note(format!("pass it with `&{name}` or `give {name}`; read its fields with `{name}.x`")));
                    }
                    if let Type::Own(_) | Type::Lent(_) = ty {
                        return Err(err(format!("buffer `{name}` cannot be used as a value"), span)
                            .with_note(format!("index it (`{name}[i]`), pass it with `&{name}` or `give {name}`, or burn it")));
                    }
                    hx(HExprKind::Local(id), ty, span)
                } else if let Some(c) = self.consts.get(name) {
                    HExpr { span, ..c.clone() }
                } else {
                    return Err(self.unknown(name, span));
                }
            }
            ExprKind::Unary(UnOp::Not, inner) if self.expr(inner, None).is_ok_and(|h| h.ty == Type::Bool) => {
                let h = self.expr(inner, None)?;
                match h.kind {
                    HExprKind::Word(w) => hx(HExprKind::Word(-w), Type::Bool, span),
                    _ => hx(HExprKind::Neg(Box::new(h)), Type::Bool, span),
                }
            }
            ExprKind::Unary(op, inner) => {
                let h = self.expr(inner, want.filter(|t| t.is_numeric()))?;
                self.numeric(&h)?;
                let ty = h.ty;
                if *op == UnOp::Not && ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`!` works on bools and trits, found {ty}"), h.span)
                        .with_note("to flip the sign of a number use `-x`"));
                }
                match h.kind {
                    HExprKind::Word(w) => hx(HExprKind::Word(-w), ty, span),
                    HExprKind::Real(r) => hx(HExprKind::Real(-r), ty, span),
                    _ => hx(HExprKind::Neg(Box::new(h)), ty, span),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, span)?,
            ExprKind::Member(base, field) if self.is_object(base) => {
                let (obj, c) = self.object(base)?.expect("checked by the guard");
                let (off, ty) = self.class_field(c, field)?;
                let zero = hx(HExprKind::Word(Word::ZERO), Type::Scalar(Scalar::Int), span);
                let h = hx(HExprKind::ElemField(Box::new(obj), Box::new(zero), off), Self::view(ty), span);
                // A `T?` field proved not empty reads as a `T`.
                let ty = self.narrowed_ty(&h, h.ty);
                HExpr { ty, ..h }
            }
            ExprKind::GiveField(path) => self.give_field(path, span)?,
            // `a ?: b`: `a` if it is not null, otherwise `b`.
            ExprKind::Elvis(a, b) => {
                let ha = self.read_in_place(a)?;
                if !ha.ty.is_opt() {
                    return Err(err(format!("this is {}, so it is never null", ha.ty), a.span).with_note("`?:` gives a value for a null"));
                }
                if !matches!(ha.ty, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_)) {
                    return Err(err(format!("`?:` works on numbers, bools, texts and enums, found {}", ha.ty), a.span)
                        .with_note("for an object, check it: `if x != null { ... } else { ... }`"));
                }
                let inner = ha.ty.unopt();
                // `a ?: b ?: c` with `b` a `T?` too: the result may still be null.
                let hb = self.expr(b, Some(inner))?;
                let ty = if hb.ty.is_opt() { ha.ty } else { inner };
                let hb = self.coerce(hb, ty)?;
                let tmp = self.new_local("$elvis", inner, LocalKind::Hidden, span);
                let then = hx(HExprKind::Local(tmp), inner, span);
                let then = self.coerce(then, ty)?;
                hx(HExprKind::IfNull { value: Box::new(ha), tmp, then: Box::new(then), null: Some(Box::new(hb)) }, ty, span)
            }
            ExprKind::Safe { recv, var, rest } => self.safe(recv, var, rest, span)?,
            ExprKind::Is(value, class) => {
                let Some(h) = self.iface_value(value)? else {
                    let ty = self.expr(value, None).map(|h| h.ty.to_string()).unwrap_or_else(|_| "this".into());
                    return Err(err(format!("`is` tests an interface value (like `f: Forma`); this is {ty}"), value.span));
                };
                let c = self.implementer(iface_of(h.ty).expect("an interface value"), class)?;
                hx(HExprKind::IsClass(Box::new(h), self.class_tags[c], c), Type::Bool, span)
            }
            ExprKind::MethodCall { recv, name, args } => self.method_call(recv, name, args, span)?,
            ExprKind::Member(base, field) if self.iface_recv(base).is_ok_and(|h| h.is_some()) => {
                return Err(err(format!("`{}` is hidden: an interface shows only its methods", field.name), field.span)
                    .with_note("call one of them, or check the class first: `if x is Circulo { ... }`"));
            }
            ExprKind::Member(_, field)
                if matches!(&e.kind, ExprKind::Member(b, _) if matches!(&b.kind, ExprKind::Var(n)
                    if self.lookup(n).is_some_and(|id| matches!(self.f.locals[id].ty, Type::OptClass(_) | Type::LentOptClass(_))))) =>
            {
                let ExprKind::Member(base, _) = &e.kind else { unreachable!() };
                let ExprKind::Var(n) = &base.kind else { unreachable!() };
                let id = self.lookup(n).expect("checked");
                let _ = field;
                return Err(self.maybe_empty(self.f.locals[id].ty, base.span));
            }
            ExprKind::Member(base, field) => {
                let enum_name = match &base.kind {
                    ExprKind::Var(n) if self.lookup(n).is_none() && self.enums.contains_key(n) => Some(n),
                    _ => None,
                };
                let Some(tname) = enum_name else {
                    let hb = self.expr(base, None)?;
                    if hb.ty.is_opt() {
                        return Err(self.maybe_empty(hb.ty, base.span));
                    }
                    let Type::Struct(sname) = hb.ty else {
                        return Err(err(format!("{} has no fields", hb.ty), field.span)
                            .with_note("`.` reads a struct field (`p.x`) or an enum value (`Cor.Verde`)"));
                    };
                    let (off, ty) = self.field(sname, field)?;
                    return Ok(self.field_of(hb, off, ty, span));
                };
                let info = &self.enums[tname];
                let Some((_, k)) = info.variants.iter().find(|(n, _)| *n == field.name) else {
                    let names: Vec<&str> = info.variants.iter().map(|(n, _)| n.as_str()).collect();
                    return Err(err(format!("enum `{tname}` has no value `{}`", field.name), field.span)
                        .with_note(format!("its values are {}", names.join(", "))));
                };
                hx(HExprKind::Word(Word::new(*k)), Type::Enum(info.name), span)
            }
            ExprKind::Cast(inner, s) => {
                let h = self.expr(inner, None)?;
                // An enum value is stored as its number: `(int) c` just reads it.
                let h = if let Type::Enum(_) = h.ty { HExpr { ty: Type::Scalar(Scalar::Int), ..h } } else { h };
                let to = *s;
                match (h.ty, to) {
                    (from, to) if from == to => h,
                    // true/false are +1/-1: as a number they need no conversion.
                    (Type::Bool, t) if t.is_int() => HExpr { ty: t, ..h },
                    // A number becomes a bool only if it is 1 or -1 (checked when it runs).
                    (from, Type::Bool) if from.is_int() => hx(HExprKind::Convert(Box::new(h)), Type::Bool, span),
                    (from, Type::Scalar(_)) if from.is_numeric() => hx(HExprKind::Convert(Box::new(h)), to, span),
                    (from, _) => return Err(err(format!("cannot cast {from} to {to}"), inner.span)),
                }
            }
            ExprKind::Index(base, idx) => {
                if let ExprKind::Var(name) = &base.kind
                    && let Some(id) = self.lookup(name)
                    && let ty @ (Type::Own(el) | Type::Lent(el)) = self.f.locals[id].ty
                {
                    let hidx = self.index(idx)?;
                    self.check_index(id, &hidx)?;
                    let hb = hx(HExprKind::Local(id), ty, base.span);
                    return Ok(Self::element(hb, hidx, el.ty(), span));
                }
                let hb = self.expr(base, None)?;
                if let Type::Own(el) | Type::Lent(el) = hb.ty {
                    let hidx = self.index(idx)?;
                    return Ok(Self::element(hb, hidx, el.ty(), span));
                }
                if hb.ty != Type::Text {
                    return Err(err(format!("cannot index {}", hb.ty), base.span));
                }
                let hidx = self.index(idx)?;
                hx(HExprKind::Index(Box::new(hb), Box::new(hidx)), Type::Scalar(Scalar::Tryte), span)
            }
            ExprKind::Call { name, args } => self.call(name, args, span)?,
            ExprKind::Claim(size) => match want {
                Some(Type::Own(el)) => {
                    if let Type::Class(c) | Type::Iface(c) = el.ty() {
                        return Err(err(format!("`claim` would make empty `{c}` elements"), span)
                            .with_note("an array of objects holds only real objects: start with `{}` and `append` them"));
                    }
                    let n = self.index(size)?;
                    hx(HExprKind::Claim(Box::new(n)), Type::Own(el), span)
                }
                _ => {
                    return Err(err("`claim` needs to know the array type", span)
                        .with_note("declare it, e.g. `b[]: int = claim 5;`"));
                }
            },
            ExprKind::ArrayLit(items) => match want {
                Some(t @ Type::Own(buf)) => {
                    if buf.sym && items.len() % 2 == 0 {
                        return Err(err(format!("a sym array needs an odd number of values, found {}", items.len()), span)
                            .with_note("the middle value is index 0"));
                    }
                    let mut hs = Vec::new();
                    for item in items {
                        hs.push(self.expect(item, buf.ty())?);
                    }
                    hx(HExprKind::Pack(hs), t, span)
                }
                _ => {
                    return Err(err("`{...}` needs to know the array type", span)
                        .with_note("declare it, e.g. `b[]: int = {1, 2, 3};`"));
                }
            },
            ExprKind::Give(name) => {
                let id = self.lookup(&name.name).ok_or_else(|| self.unknown(&name.name, name.span))?;
                self.give(id, span)?
            }
            ExprKind::Range(..) => {
                return Err(err("a range `a..b` only works as an argument of `replace` here", span)
                    .with_note("in loops, write `walk i in a..b { ... }`"));
            }
            ExprKind::StructLit { name, fields } if self.classes.contains_key(&name.name) => self.new_object(name, fields, span)?,
            ExprKind::StructLit { name, fields } => {
                let Some((&sname, decl)) = self.structs.get_key_value(name.name.as_str()) else {
                    return Err(err(format!("unknown struct `{}`", name.name), name.span));
                };
                let decl = decl.clone();
                for (i, (f, _)) in fields.iter().enumerate() {
                    if !decl.iter().any(|(n, _)| *n == f.name) {
                        self.field(sname, f)?;
                    }
                    if fields[..i].iter().any(|(g, _)| g.name == f.name) {
                        return Err(err(format!("field `{}` is given twice", f.name), f.span));
                    }
                }
                let missing: Vec<String> =
                    decl.iter().filter(|(n, _)| !fields.iter().any(|(f, _)| f.name == *n)).map(|(n, _)| format!("`{n}`")).collect();
                if !missing.is_empty() {
                    let s = if missing.len() == 1 { "" } else { "s" };
                    return Err(err(format!("missing field{s} {} in `{sname}`", missing.join(", ")), span)
                        .with_note("every field must get a value"));
                }
                // Values are evaluated in the declared order of the fields.
                let mut values = Vec::new();
                for (n, t) in &decl {
                    let (_, e) = fields.iter().find(|(f, _)| f.name == *n).expect("checked above");
                    values.push(self.expect(e, *t)?);
                }
                hx(HExprKind::StructLit(values), Type::Struct(sname), span)
            }
            // In a copy of a generic, `lend` of a value copies it.
            ExprKind::Lend(inner) if self.f.instance => {
                let h = match &inner.kind {
                    ExprKind::Var(n) if self.lookup(n).is_some() => {
                        let id = self.lookup(n).expect("checked");
                        hx(HExprKind::Local(id), self.f.locals[id].ty, inner.span)
                    }
                    _ => self.expr(inner, want)?,
                };
                if is_lent(h.ty) || matches!(h.ty, Type::Own(_) | Type::Class(_) | Type::Iface(_)) || h.ty.is_opt() && matches!(h.ty.unopt(), Type::Class(_) | Type::Iface(_)) {
                    return Err(err("`&` (borrowing) only works in a call argument or a borrowing variable (`x: &T = &y;`)", span));
                }
                h
            }
            ExprKind::Lend(_) => {
                return Err(err("`&` (borrowing) only works in a call argument or a borrowing variable (`x: &T = &y;`)", span));
            }
        })
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, want: Option<Type>, span: Span) -> R<HExpr> {
        // `a < b` on objects and interface values: `a.compareTo(&b) < 0`;
        // on texts: alphabetical (character by character).
        if matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Cmp3) {
            let ty = self.read_in_place(a).map(|h| h.ty).ok();
            let object = matches!(ty, Some(Type::Class(_) | Type::LentClass(_) | Type::Iface(_) | Type::LentIface(_)));
            if object || ty == Some(Type::Text) {
                let arg = Expr { kind: ExprKind::Lend(Box::new(b.clone())), span: b.span };
                let call = Expr { kind: ExprKind::MethodCall { recv: Box::new(a.clone()), name: ast::Ident { name: "compareTo".into(), span }, args: vec![arg] }, span };
                let order = self.expr(&call, None)?;
                let zero = hx(HExprKind::Word(Word::ZERO), Type::Scalar(Scalar::Int), span);
                let ty = if op == BinOp::Cmp3 { Type::Scalar(Scalar::Trit) } else { Type::Bool };
                return Ok(hx(HExprKind::Binary(op, Box::new(order), Box::new(zero)), ty, span));
            }
        }
        // `a == b` on objects, interface values and structs: `a.equals(&b)`.
        if matches!(op, BinOp::Eq | BinOp::Ne)
            && a.kind != ExprKind::NaT
            && b.kind != ExprKind::NaT
            && self.read_in_place(a).is_ok_and(|h| matches!(h.ty, Type::Class(_) | Type::LentClass(_) | Type::Iface(_) | Type::LentIface(_) | Type::Struct(_)))
        {
            let arg = Expr { kind: ExprKind::Lend(Box::new(b.clone())), span: b.span };
            let call = Expr { kind: ExprKind::MethodCall { recv: Box::new(a.clone()), name: ast::Ident { name: "equals".into(), span }, args: vec![arg] }, span };
            let h = self.expr(&call, None)?;
            return Ok(if op == BinOp::Eq { h } else { hx(HExprKind::Neg(Box::new(h)), Type::Bool, span) });
        }
        // `x == null` / `x != null`: the only test that null does not poison.
        if matches!(op, BinOp::Eq | BinOp::Ne) && (a.kind == ExprKind::NaT || b.kind == ExprKind::NaT) {
            let x = if a.kind == ExprKind::NaT { b } else { a };
            if x.kind == ExprKind::NaT {
                return Err(err("`null == null` is always true", span));
            }
            let h = self.read_in_place(x)?;
            if !h.ty.is_opt() {
                return Err(err(format!("this is {}, so it is never null", h.ty), x.span)
                    .with_note("only a `T?` can be null"));
            }
            let test = hx(HExprKind::Builtin(Builtin::IsNaT, vec![h]), Type::Bool, span);
            return Ok(if op == BinOp::Eq { test } else { hx(HExprKind::Neg(Box::new(test)), Type::Bool, span) });
        }
        let trit = Type::Scalar(Scalar::Trit);
        let (ha, hb, ty) = match op {
            BinOp::And | BinOp::Or => {
                // `!isNaT(x) && x.v > 0`: the right side runs only when the left proved `x`.
                let ha = self.expr(a, Some(trit))?;
                let mut proved = Vec::new();
                facts(&ha, op == BinOp::And, &mut proved);
                let mark = self.f.narrowed.len();
                proved.iter().for_each(|p| self.narrow(p));
                let hb = self.expr(b, Some(trit));
                self.unnarrow_to(mark);
                let hb = hb?;
                for h in [&ha, &hb] {
                    if h.ty.is_opt() {
                        return Err(self.maybe_empty(h.ty, h.span));
                    }
                    if !is_truth(h.ty) {
                        return Err(err(format!("`{}` works on bools and trits, found {}", op.symbol(), h.ty), h.span)
                            .with_note("compare first, e.g. `a > 0 && b > 0`"));
                    }
                }
                // Two bools give a bool; with a trit, the result can be 0 (unknown).
                let ty = if ha.ty == Type::Bool && hb.ty == Type::Bool { Type::Bool } else { trit };
                (ha, hb, ty)
            }
            // Text: `a + b` makes a new text, `==`/`!=` compare characters.
            BinOp::Add | BinOp::Eq | BinOp::Ne if self.is_text(a) || self.is_text(b) => {
                let text = Type::Text;
                let ha = self.expr(a, Some(text))?;
                let hb = self.expr(b, Some(text))?;
                for h in [&ha, &hb] {
                    if h.ty != text {
                        let what = if op == BinOp::Add { "`+` joins text with text only" } else { "a text can only be compared with text" };
                        return Err(err(format!("{what}, found {}", h.ty), h.span).with_note("convert it first: `toText(x)`"));
                    }
                }
                let ty = if op == BinOp::Add { text } else { Type::Bool };
                (ha, hb, ty)
            }
            BinOp::Eq | BinOp::Ne if self.enum_type(a).or_else(|| self.enum_type(b)).is_some() => {
                let t = self.enum_type(a).or_else(|| self.enum_type(b)).expect("checked by the guard");
                let ha = self.expr(a, Some(t))?;
                let hb = self.expr(b, Some(t))?;
                for h in [&ha, &hb] {
                    if h.ty != t {
                        return Err(err(format!("cannot compare {t} with {}", h.ty), h.span));
                    }
                }
                (ha, hb, Type::Bool)
            }
            BinOp::Eq | BinOp::Ne if self.is_bool(a) || self.is_bool(b) => {
                let ha = self.expr(a, Some(Type::Bool))?;
                let hb = self.expr(b, Some(Type::Bool))?;
                for h in [&ha, &hb] {
                    if h.ty != Type::Bool {
                        return Err(err(format!("cannot compare bool with {}", h.ty), h.span));
                    }
                }
                (ha, hb, Type::Bool)
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge if self.is_bool(a) || self.is_bool(b) => {
                return Err(err(format!("`{}` does not work on bools", op.symbol()), span).with_note("only `==` and `!=` compare bools"));
            }
            // `<=>` answers in three ways (a trit); the others answer yes or no.
            BinOp::Cmp3 | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let (ha, hb, _) = self.numeric_pair(a, b, None)?;
                (ha, hb, if op == BinOp::Cmp3 { trit } else { Type::Bool })
            }
            BinOp::Pow | BinOp::Shl | BinOp::Shr => {
                let ha = self.expr(a, want.filter(|t| t.is_numeric()))?;
                self.numeric(&ha)?;
                if ha.ty == Type::Scalar(Scalar::Real) && op != BinOp::Pow {
                    return Err(err(format!("`{}` works on integers, not reals", op.symbol()), span));
                }
                let hb = self.expr(b, Some(Type::Scalar(Scalar::Int)))?;
                if !hb.ty.is_int() {
                    return Err(err(format!("the right side of `{}` must be an integer, found {}", op.symbol(), hb.ty), hb.span));
                }
                let ty = ha.ty;
                (ha, hb, ty)
            }
            _ => {
                let (ha, hb, ty) = self.numeric_pair(a, b, want)?;
                if op == BinOp::Mod && ty == Type::Scalar(Scalar::Real) {
                    return Err(err("`%` works on integers, not reals", span));
                }
                (ha, hb, ty)
            }
        };
        Ok(hx(HExprKind::Binary(op, Box::new(ha), Box::new(hb)), ty, span))
    }

    /// `match` becomes: store the value in a hidden local, then a chain of
    /// `if tmp == a || tmp == b { arm } else { ... }`. An enum match without
    /// `else` must list every value; its last arm then needs no test.
    fn match_stmt(&mut self, scrut: &Expr, arms: &[ast::MatchArm], span: Span) -> R<HStmtKind> {
        if let Some(h) = self.iface_value(scrut)? {
            return self.match_class(scrut, h, arms, span);
        }
        let h = self.expr(scrut, None)?;
        let ty = h.ty;
        if !(ty.is_int() || matches!(ty, Type::Enum(_))) {
            return Err(err(format!("`match` works on integers and enums, found {ty}"), scrut.span));
        }
        let tmp = self.new_local("$match", ty, LocalKind::Hidden, scrut.span);
        let mut seen: Vec<i64> = Vec::new();
        let mut tests: Vec<(Vec<i64>, HBlock)> = Vec::new();
        let mut otherwise = None;
        for (i, arm) in arms.iter().enumerate() {
            if arm.pats.is_empty() {
                if i + 1 != arms.len() {
                    return Err(err("the `else` arm must be the last one", arm.span));
                }
                otherwise = Some(self.block(&arm.body)?);
                continue;
            }
            let mut vals = Vec::new();
            for p in &arm.pats {
                let hp = self.expect(p, ty)?;
                let v = match hp.kind {
                    HExprKind::Word(w) if !w.is_nat() => w.get().unwrap_or(0),
                    _ => return Err(err("a `match` pattern must be a literal or an enum value", p.span)),
                };
                if seen.contains(&v) {
                    return Err(err("this value already appears in an earlier arm", p.span));
                }
                seen.push(v);
                vals.push(v);
            }
            tests.push((vals, self.block(&arm.body)?));
        }
        if otherwise.is_none() {
            if let Type::Enum(n) = ty {
                let missing: Vec<String> = self.enums[n]
                    .variants
                    .iter()
                    .filter(|(_, v)| !seen.contains(v))
                    .map(|(name, _)| format!("{n}.{name}"))
                    .collect();
                if !missing.is_empty() {
                    return Err(err(format!("`match` does not cover {}", missing.join(", ")), scrut.span)
                        .with_note("add an arm for each, or an `else` arm"));
                }
            } else {
                return Err(err(format!("`match` on {ty} needs an `else` arm"), scrut.span)
                    .with_note("integers have too many values to list them all"));
            }
        }
        let trit = Type::Scalar(Scalar::Trit);
        let local = || hx(HExprKind::Local(tmp), ty, scrut.span);
        let mut chain = otherwise;
        for (vals, body) in tests.into_iter().rev() {
            chain = Some(match chain {
                None => body, // last arm of a complete enum match
                Some(rest) => {
                    let cond = vals
                        .iter()
                        .map(|v| {
                            let k = hx(HExprKind::Word(Word::new(*v)), ty, scrut.span);
                            hx(HExprKind::Binary(BinOp::Eq, Box::new(local()), Box::new(k)), Type::Bool, scrut.span)
                        })
                        .reduce(|a, b| hx(HExprKind::Binary(BinOp::Or, Box::new(a), Box::new(b)), trit, scrut.span))
                        .expect("an arm has at least one pattern");
                    let trap = HStmt { kind: HStmtKind::Trap("match test is 0 (unknown)".into()), span: scrut.span };
                    let unknown = HBlock { stmts: vec![trap], scope: vec![], span: scrut.span };
                    let tri = HStmtKind::Tri { head: cond, arms: Box::new([rest, unknown, body]), origin: Branch::Match };
                    HBlock { stmts: vec![HStmt { kind: tri, span }], scope: vec![], span }
                }
            });
        }
        let store = HStmt { kind: HStmtKind::Let { local: tmp, value: h, on_nat: None }, span: scrut.span };
        let mut stmts = vec![store];
        if let Some(c) = chain {
            stmts.push(HStmt { kind: HStmtKind::Block(c), span });
        }
        Ok(HStmtKind::Block(HBlock { stmts, scope: vec![], span }))
    }

    /// Whether `e` is a text (checked without reporting errors).
    fn is_bool(&mut self, e: &Expr) -> bool {
        self.expr(e, None).is_ok_and(|h| h.ty == Type::Bool)
    }

    fn is_text(&mut self, e: &Expr) -> bool {
        self.expr(e, None).is_ok_and(|h| h.ty == Type::Text)
    }

    /// The enum type of `e`, if it has one.
    fn enum_type(&mut self, e: &Expr) -> Option<Type> {
        self.expr(e, None).ok().map(|h| h.ty).filter(|t| matches!(t, Type::Enum(_)))
    }

    /// The array a size-changing built-in works on, lent for the call.
    fn array_arg(&mut self, e: &Expr, op: &str) -> R<(HExpr, ast::Buf)> {
        let found = match &e.kind {
            ExprKind::Var(name) => self.lookup(name).map(|id| (id, self.f.locals[id].ty)),
            // An array held in an object's field (`self.items`).
            _ if self.expr(e, None).is_ok_and(|h| matches!(h.ty, Type::Lent(_))) => {
                let h = self.expr(e, None)?;
                let Type::Lent(buf) = h.ty else { unreachable!() };
                if buf.sym && op != "replace" {
                    return Err(err(format!("`{op}` does not work on a sym array"), e.span)
                        .with_note("a sym array must stay odd and centered on index 0"));
                }
                return Ok((h, buf));
            }
            _ => None,
        };
        let Some((id, Type::Own(buf) | Type::Lent(buf))) = found else {
            return Err(err(format!("`{op}` takes an array variable first"), e.span).with_note(format!("e.g. `{op}(v, ...)`")));
        };
        if buf.sym && op != "replace" {
            return Err(err(format!("`{op}` does not work on a sym array"), e.span)
                .with_note("a sym array must stay odd and centered on index 0"));
        }
        self.f.sizes.remove(&id);
        Ok((hx(HExprKind::Lend(id), Type::Lent(buf), e.span), buf))
    }

    /// Like `expr`, but an array name is lent, for built-ins that look at
    /// the whole array (`len`, `writeBytes`).
    fn viewed(&mut self, e: &Expr, want: Option<Type>) -> R<HExpr> {
        if let ExprKind::Var(name) = &e.kind
            && let Some(id) = self.lookup(name)
            && let Type::Own(el) | Type::Lent(el) = self.f.locals[id].ty
        {
            return Ok(hx(HExprKind::Lend(id), Type::Lent(el), e.span));
        }
        let h = self.expr(e, want)?;
        // A `T?` value prints `null` or its value; an object must be checked first.
        if matches!(h.ty, Type::OptClass(_) | Type::OptIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_)) {
            return Err(self.maybe_empty(h.ty, e.span));
        }
        Ok(h)
    }

    /// Like `viewed`, for `print`/`printf`/`toText`: only arrays of numbers
    /// and texts print whole.
    fn shown(&mut self, e: &Expr, want: Option<Type>) -> R<HExpr> {
        // An interface value prints with its class's `toString`.
        if let Ok(Some(h)) = self.iface_recv(e) {
            return Ok(h);
        }
        // An object that may be null must be checked first.
        if let ExprKind::Var(n) = &e.kind
            && let Some(id) = self.lookup(n)
            && matches!(self.f.locals[id].ty, Type::OptClass(_) | Type::OptIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_))
        {
            return Err(self.maybe_empty(self.f.locals[id].ty, e.span));
        }
        if self.is_object(e) {
            let (h, _) = self.object(e)?.expect("checked");
            return Ok(h);
        }
        if let ExprKind::Var(name) = &e.kind
            && let Some(id) = self.lookup(name)
            && let Type::Own(el) | Type::Lent(el) = self.f.locals[id].ty
        {
            if !matches!(el.ty(), Type::Scalar(_) | Type::Text) {
                return Err(err(format!("an array of {} cannot be printed whole", el.ty()), e.span)
                    .with_note(format!("print its elements one by one: `for i: int = 0; i < len({name}); i++ {{ printl({name}[i]); }}`")));
            }
            return Ok(hx(HExprKind::Lend(id), Type::Lent(el), e.span));
        }
        self.expr(e, want)
    }

    /// True if `e` is a `+` chain with a text piece in it: `print` then joins
    /// text instead of adding numbers.
    fn is_text_chain(&mut self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Binary(BinOp::Add, a, b) => self.is_text_chain(a) || self.is_text_chain(b),
            _ => self.expr(e, None).is_ok_and(|h| h.ty == Type::Text),
        }
    }

    /// Flattens `print("a" + x + "b")` into its text pieces; every piece must be text.
    fn text_pieces(&mut self, e: &Expr, out: &mut Vec<HExpr>) -> R<()> {
        let ExprKind::Binary(BinOp::Add, a, b) = &e.kind else {
            out.push(self.expect(e, Type::Text)?);
            return Ok(());
        };
        for side in [a, b] {
            if matches!(side.kind, ExprKind::Binary(BinOp::Add, ..)) {
                self.text_pieces(side, out)?;
                continue;
            }
            let h = self.expr(side, Some(Type::Text))?;
            if h.ty != Type::Text {
                return Err(err(format!("`+` joins text with text only, found {}", h.ty), h.span)
                    .with_note("print numbers on their own, or use `printf(\"x = %d\", x)`"));
            }
            out.push(h);
        }
        Ok(())
    }

    /// Splits a `printf` format into pieces. `{name}` / `{name:t|v|c}` read a
    /// variable; `%d/%i %f/%r %s %t %v %c` take the next argument; `{{ }} %%` are literal.
    fn format(&mut self, f: &str, fspan: Span, args: &mut std::slice::Iter<Expr>) -> R<Vec<(Fmt, HExpr)>> {
        let mut out = Vec::new();
        let mut lit = String::new();
        // Source span of each character, so errors point inside the literal
        // (skipping the opening quote; `\n` and `\t` take two source bytes).
        let mut at = fspan.start + 1;
        let located: Vec<(Span, char)> = f
            .chars()
            .map(|c| {
                let w = if matches!(c, '\n' | '\t') { 2 } else { c.len_utf8() };
                at += w;
                (Span { start: at - w, end: at }, c)
            })
            .collect();
        let mut chars = located.into_iter().peekable();
        let flush = |lit: &mut String, out: &mut Vec<(Fmt, HExpr)>| -> R<()> {
            if !lit.is_empty() {
                out.push((Fmt::Plain, hx(HExprKind::Text(std::mem::take(lit)), Type::Text, fspan)));
            }
            Ok(())
        };
        while let Some((cspan, c)) = chars.next() {
            if matches!(c, '{' | '}' | '%') && chars.peek().map(|p| p.1) == Some(c) {
                chars.next();
                lit.push(c);
                continue;
            }
            match c {
                '{' => {
                    let mut inner = String::new();
                    let mut pspan = cspan;
                    loop {
                        match chars.next() {
                            Some((sp, '}')) => {
                                pspan = pspan.to(sp);
                                break;
                            }
                            Some((sp, c)) => {
                                pspan = pspan.to(sp);
                                inner.push(c);
                            }
                            None => {
                                return Err(err("`{` is never closed in this format", cspan)
                                    .with_note("write `{{` to print a `{`"));
                            }
                        }
                    }
                    flush(&mut lit, &mut out)?;
                    let label = format!("{{{inner}}}");
                    // `{expr}` or `{expr:t}`: any expression, like Python's f-strings.
                    let (code, spec) = match inner.rsplit_once(':') {
                        Some((code, spec)) if spec.chars().all(|c| c.is_ascii_alphabetic()) => (code, spec),
                        _ => (inner.as_str(), ""),
                    };
                    let spec = match spec {
                        "" => ' ',
                        "t" | "v" | "c" => spec.chars().next().unwrap_or(' '),
                        _ => {
                            return Err(err(format!("unknown format `:{spec}` in `{label}`"), pspan)
                                .with_note("use `:t` (ternary), `:v` (heptavintimal) or `:c` (character)"));
                        }
                    };
                    if code.trim().is_empty() {
                        return Err(err(format!("`{label}` must hold a value"), pspan)
                            .with_note("e.g. `{x}`, `{p.nome}` or `{a + b}`; write `{{` to print a `{`"));
                    }
                    // Lex just the code, then move every position to where it sits in the
                    // file, so errors point inside the text.
                    let at = cspan.end;
                    let shift = |sp: Span| Span::new(sp.start + at, sp.end + at);
                    let mut tokens = crate::lexer::lex(code).map_err(|d| Diagnostic { span: shift(d.span), ..d })?;
                    for t in &mut tokens {
                        t.span = shift(t.span);
                    }
                    let mut e = crate::parser::parse_expr(tokens)?;
                    crate::resolve::format_expr(&mut e)?;
                    let h = self.shown(&e, None)?;
                    let (f, h) = piece(spec, &label, h)?;
                    out.push((f, self.displayed(h)));
                }
                '}' => return Err(err("`}` without a matching `{` in this format", cspan).with_note("write `}}` to print a `}`")),
                '%' => {
                    let (pspan, spec) = match chars.next() {
                        Some((sp, c)) => (cspan.to(sp), c),
                        None => (cspan, ' '),
                    };
                    let want = match spec {
                        'd' | 'i' | 't' | 'v' => Type::Scalar(Scalar::Int),
                        'f' | 'r' => Type::Scalar(Scalar::Real),
                        's' => Type::Text,
                        'c' => Type::Scalar(Scalar::Tryte),
                        _ => {
                            return Err(err(format!("unknown format `%{}`", spec.to_string().trim()), pspan)
                                .with_note("use %d or %i (integer), %f or %r (real), %s (text), %t (ternary), %v (heptavintimal), %c (character); `%%` prints `%`"));
                        }
                    };
                    let label = format!("%{spec}");
                    let Some(a) = args.next() else {
                        return Err(err(format!("`{label}` has no argument to print"), pspan)
                            .with_note(format!("every `%` takes the next argument, and none is left; to print `{label}` as text, write `%{label}`")));
                    };
                    flush(&mut lit, &mut out)?;
                    let h = self.shown(a, Some(want))?;
                    let (f, h) = piece(spec, &label, h)?;
                    out.push((f, self.displayed(h)));
                }
                _ => lit.push(c),
            }
        }
        flush(&mut lit, &mut out)?;
        Ok(out)
    }

    fn call(&mut self, name: &ast::Ident, args: &[Expr], span: Span) -> R<HExpr> {
        let arity = |n: usize| -> R<()> {
            if args.len() == n {
                Ok(())
            } else {
                let s = if n == 1 { "" } else { "s" };
                Err(err(format!("`{}` takes {n} argument{s}, found {}", name.name, args.len()), span))
            }
        };
        let text = Type::Text;
        let builtin = |b: Builtin, args: Vec<HExpr>, ty: Type| hx(HExprKind::Builtin(b, args), ty, span);
        match name.name.as_str() {
            "print" | "printl" => {
                if name.name == "print" && args.is_empty() {
                    return Err(err("`print` takes at least 1 argument", span).with_note("`printl()` prints an empty line"));
                }
                // `print(a, b, c)` prints the values separated by spaces, like Python.
                let mut pieces = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        pieces.push((Fmt::Plain, hx(HExprKind::Text(" ".into()), text, span)));
                    }
                    if self.is_text_chain(arg) {
                        let mut texts = Vec::new();
                        self.text_pieces(arg, &mut texts)?;
                        pieces.extend(texts.into_iter().map(|h| (Fmt::Plain, h)));
                    } else {
                        let a = self.shown(arg, None)?;
                        if !matches!(a.ty, Type::Scalar(_) | Type::Bool | Type::Text | Type::Lent(_) | Type::Enum(_) | Type::Struct(_) | Type::Class(_) | Type::LentClass(_) | Type::LentIface(_))
                            && !matches!(a.ty, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_))
                        {
                            return Err(err(format!("`{}` prints numbers and text, found {}", name.name, a.ty), a.span));
                        }
                        pieces.push((Fmt::Plain, self.displayed(a)));
                    }
                }
                if name.name == "printl" {
                    pieces.push((Fmt::Plain, hx(HExprKind::Text("\n".into()), text, span)));
                }
                return Ok(hx(HExprKind::Print(pieces), Type::Void, span));
            }
            "printf" => {
                let Some((first, rest)) = args.split_first() else {
                    return Err(err("`printf` takes a format text first", span));
                };
                // The format may join texts with `+`: literals are read as format,
                // anything else must be text and is printed as is.
                let mut parts = Vec::new();
                add_chain(first, &mut parts);
                let mut rest = rest.iter();
                let mut pieces = Vec::new();
                for part in parts {
                    if let ExprKind::Str(f) = &part.kind {
                        pieces.extend(self.format(f, part.span, &mut rest)?);
                        continue;
                    }
                    let h = self.expr(part, Some(Type::Text))?;
                    if h.ty != Type::Text {
                        return Err(err(format!("the format of `printf` joins text only, found {}", h.ty), h.span)
                            .with_note("pass numbers as arguments: `printf(\"x = %d\", x)`"));
                    }
                    pieces.push((Fmt::Plain, h));
                }
                if let Some(extra) = rest.next() {
                    return Err(err("this argument has no `%` in the format", extra.span)
                        .with_note("a `%` inside a text variable is printed as is, not read as format"));
                }
                return Ok(hx(HExprKind::Print(pieces), Type::Void, span));
            }
            "toText" => {
                let (x, base) = match args {
                    [x] => (x, None),
                    [x, b] => (x, Some(b)),
                    _ => return Err(err(format!("`toText` takes 1 or 2 arguments, found {}", args.len()), span)),
                };
                let fmt = match base.map(|b| &b.kind) {
                    None => Fmt::Plain,
                    Some(ExprKind::Char(c)) if char_to_tryte('t') == Some(*c) => Fmt::Trits,
                    Some(ExprKind::Char(c)) if char_to_tryte('v') == Some(*c) => Fmt::Hepta,
                    Some(_) => {
                        return Err(err("the base of `toText` must be 't' (ternary) or 'v' (heptavintimal)", base.map_or(span, |b| b.span))
                            .with_note("leave it out for decimal: `toText(x)`"));
                    }
                };
                let a = self.shown(x, None)?;
                if fmt == Fmt::Plain && matches!(a.ty, Type::Enum(_) | Type::Bool) {
                    return Ok(self.displayed(a));
                }
                let ok = match fmt {
                    Fmt::Plain => matches!(a.ty, Type::Scalar(_) | Type::Lent(_)),
                    _ => a.ty.is_int() || matches!(a.ty, Type::Lent(_)),
                };
                if !ok {
                    let wants = if fmt == Fmt::Plain { "a number" } else { "an integer" };
                    return Err(err(format!("`toText` takes {wants}, found {}", a.ty), a.span));
                }
                return Ok(builtin(Builtin::ToText(fmt), vec![a], text));
            }
            "sub" => {
                arity(3)?;
                let s = self.expect(&args[0], text)?;
                let a = self.expect(&args[1], Type::Scalar(Scalar::Int))?;
                let b = self.expect(&args[2], Type::Scalar(Scalar::Int))?;
                return Ok(builtin(Builtin::Sub, vec![s, a, b], text));
            }
            "toInt" | "toReal" => {
                arity(1)?;
                let s = self.expect(&args[0], text)?;
                // null when the text is not a number.
                let (b, ty) = if name.name == "toInt" { (Builtin::ToInt, Scalar::Int) } else { (Builtin::ToReal, Scalar::Real) };
                return Ok(builtin(b, vec![s], Type::OptScalar(ty)));
            }
            n if CHAR_CLASSES.contains(&n) => {
                arity(1)?;
                let c = self.expect(&args[0], Type::Scalar(Scalar::Tryte))?;
                let k = CHAR_CLASSES.iter().position(|&x| x == n).expect("checked by the guard") as u8;
                return Ok(builtin(Builtin::CharIs(k), vec![c], Type::Bool));
            }
            "emit" => {
                arity(1)?;
                let a = self.expect(&args[0], Type::Scalar(Scalar::Tryte))?;
                return Ok(builtin(Builtin::Emit, vec![a], Type::Void));
            }
            "input" => {
                arity(0)?;
                // null when the line is not an integer.
                return Ok(builtin(Builtin::Input, vec![], Type::OptScalar(Scalar::Int)));
            }
            "len" => {
                arity(1)?;
                let a = self.viewed(&args[0], None)?;
                if !matches!(a.ty, Type::Text | Type::Lent(_)) {
                    return Err(err(format!("`len` takes text or an array, found {}", a.ty), a.span));
                }
                return Ok(builtin(Builtin::Len, vec![a], Type::Scalar(Scalar::Int)));
            }
            "dump" | "exit" | "assert" => return Err(err(format!("`{}(...)` is a statement, not a value", name.name), span)),
            "readFile" | "fileExists" => {
                arity(1)?;
                let p = self.expect(&args[0], text)?;
                return Ok(if name.name == "readFile" {
                    builtin(Builtin::ReadFile, vec![p], text)
                } else {
                    builtin(Builtin::FileExists, vec![p], Type::Bool)
                });
            }
            "writeFile" => {
                arity(2)?;
                let p = self.expect(&args[0], text)?;
                let t = self.expect(&args[1], text)?;
                return Ok(builtin(Builtin::WriteFile, vec![p, t], Type::Void));
            }
            "writeBytes" => {
                arity(2)?;
                let p = self.expect(&args[0], text)?;
                let a = self.viewed(&args[1], None)?;
                if !matches!(a.ty, Type::Lent(b) if b.ty().is_int()) {
                    return Err(err(format!("`writeBytes` takes an array of bytes (0..255), found {}", a.ty), a.span));
                }
                return Ok(builtin(Builtin::WriteBytes, vec![p, a], Type::Void));
            }
            "readLine" => {
                arity(0)?;
                return Ok(builtin(Builtin::ReadLine, vec![], text));
            }
            "eof" => {
                arity(0)?;
                return Ok(builtin(Builtin::Eof, vec![], Type::Bool));
            }
            "argCount" => {
                arity(0)?;
                return Ok(builtin(Builtin::ArgCount, vec![], Type::Scalar(Scalar::Int)));
            }
            "arg" => {
                arity(1)?;
                let i = self.expect(&args[0], Type::Scalar(Scalar::Int))?;
                return Ok(builtin(Builtin::Arg, vec![i], text));
            }
            "hashMix" => {
                arity(2)?;
                let int = Type::Scalar(Scalar::Int);
                let h = self.expect(&args[0], int)?;
                let x = self.expect(&args[1], int)?;
                return Ok(builtin(Builtin::HashMix, vec![h, x], int));
            }
            "isNaT" => {
                return Err(err("`isNaT(x)` is written `x == null`", span).with_note("and `!isNaT(x)` is `x != null`"));
            }
            "append" | "pop" | "insert" | "remove" | "resize" | "clear" | "replace" => {
                let n = match name.name.as_str() {
                    "pop" | "clear" => 1,
                    "append" | "remove" | "resize" => 2,
                    "insert" => 3,
                    _ => 3,
                };
                arity(n)?;
                let (arr, buf) = self.array_arg(&args[0], &name.name)?;
                if let (Type::Class(c) | Type::Iface(c), "resize" | "clear") = (buf.ty(), name.name.as_str()) {
                    let note = if name.name == "resize" {
                        "growing would make empty elements and shrinking would lose objects: use `append` and `pop`/`remove`"
                    } else {
                        "the objects would be lost: take each one out with `pop` and burn it, or burn the whole array"
                    };
                    return Err(err(format!("`{}` does not work on an array of `{c}` objects", name.name), name.span).with_note(note));
                }
                let int = Type::Scalar(Scalar::Int);
                let (b, rest, ty) = match name.name.as_str() {
                    "append" => (Builtin::Append, vec![self.expect(&args[1], buf.ty())?], Type::Void),
                    "pop" => (Builtin::Pop, vec![], buf.ty()),
                    "insert" => (Builtin::Insert, vec![self.index(&args[1])?, self.expect(&args[2], buf.ty())?], Type::Void),
                    "remove" => (Builtin::Remove, vec![self.index(&args[1])?], buf.ty()),
                    "resize" => (Builtin::Resize, vec![self.expect(&args[1], int)?], Type::Void),
                    "clear" => (Builtin::Clear, vec![], Type::Void),
                    _ => {
                        let mut bounds = Vec::new();
                        for a in &args[1..] {
                            let ExprKind::Range(x, y) = &a.kind else {
                                return Err(err("`replace` takes two ranges", a.span).with_note("e.g. `replace(v, 0..1, 3..4)`"));
                            };
                            bounds.push(self.index(x)?);
                            bounds.push(self.index(y)?);
                        }
                        (Builtin::Replace, bounds, Type::Void)
                    }
                };
                let mut all = vec![arr];
                all.extend(rest);
                return Ok(builtin(b, all, ty));
            }
            _ => {}
        }
        // `Conta(10.0)`, `List<int>()`: the constructor, the static method
        // with the class's name.
        if let Some(info) = self.classes.get(&name.name) {
            let c = info.name;
            let base = ast::short_name(c);
            if !info.methods.contains_key(base) {
                return Err(err(format!("`{c}` has no constructor"), name.span)
                    .with_note(format!("declare one in the class: `public proc {base}(...) -> {c} {{ return {c} {{ ... }}; }}`")));
            }
            let m = self.method(c, &ast::Ident { name: base.to_string(), span: name.span })?;
            return self.call_proc(m.id, c, args, None, span);
        }
        if self.ifaces.contains_key(&name.name) {
            return Err(err(format!("`{}` is an interface: it has no constructor", name.name), name.span)
                .with_note("create an object of a class that implements it"));
        }
        let Some(&id) = self.proc_ids.get(&name.name) else {
            return Err(if self.lookup(&name.name).is_some() {
                err(format!("`{}` is a variable, not a proc", name.name), name.span)
            } else {
                let d = err(format!("unknown proc `{}`", name.name), name.span);
                // Built-ins are camelCase: `to_text` became `toText`.
                let mut parts = name.name.split('_');
                let first = parts.next().unwrap_or_default().to_string();
                let camel = parts.fold(first, |acc, p| {
                    let mut c = p.chars();
                    acc + &c.next().map(|h| h.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                });
                if camel != name.name && BUILTINS.contains(&camel.as_str()) {
                    d.with_note(format!("built-ins are written in camelCase: `{camel}`"))
                } else {
                    d
                }
            });
        };
        self.call_proc(id, &name.name, args, None, span)
    }

    /// A call to a user proc or method. `this` is the method's `self`
    /// argument, already checked.
    fn call_proc(&mut self, id: ProcId, shown: &str, args: &[Expr], this: Option<HExpr>, span: Span) -> R<HExpr> {
        // A deprecated method used inside its own class (or a proc calling
        // itself) is not warned about: only the code outside has to move.
        let own_class = shown.split_once('.').is_some_and(|(c, _)| self.class == Some(c));
        if let Some(msg) = self.deprecated.get(&id)
            && self.current != Some(id)
            && !own_class
        {
            self.warn(format!("`{shown}` is deprecated"), span, msg.clone());
        }
        let sig = self.sigs[id].clone();
        let skip = usize::from(this.is_some());
        let params = &sig.params[skip..];
        let hargs = self.call_args_refs(shown, params, &sig.refs[skip..], args, this, span)?;
        Ok(hx(HExprKind::Call(id, hargs), sig.ret, span))
    }

    /// Like [`Self::call_args`], where `refs[i]` marks an `x: &int` parameter:
    /// its argument is `&n` (a variable, field or element), read now and
    /// written back when the call ends.
    fn call_args_refs(&mut self, shown: &str, params: &[(String, Type)], refs: &[bool], args: &[Expr], this: Option<HExpr>, span: Span) -> R<Vec<HExpr>> {
        if !refs.contains(&true) || args.len() != params.len() {
            return self.call_args(shown, params, args, this, span);
        }
        let mut out: Vec<HExpr> = this.into_iter().collect();
        let mut seen: Vec<HExpr> = Vec::new();
        for (k, (arg, (pname, pty))) in args.iter().zip(params).enumerate() {
            if !refs[k] {
                let mut h = self.call_args(shown, std::slice::from_ref(&(pname.clone(), *pty)), std::slice::from_ref(arg), None, arg.span)?;
                out.push(h.pop().expect("one argument"));
                continue;
            }
            let ExprKind::Lend(x) = &arg.kind else {
                return Err(err(format!("parameter `{pname}` may change your value: pass it with `&`"), arg.span)
                    .with_note("e.g. `&n`; the proc writes the new value back into it"));
            };
            self.lvalue(x)?;
            let h = match &x.kind {
                ExprKind::Var(n) => {
                    let id = self.lookup(n).ok_or_else(|| self.unknown(n, x.span))?;
                    if self.f.locals[id].is_final {
                        return Err(err(format!("`{n}` is final and cannot change"), x.span));
                    }
                    hx(HExprKind::Local(id), self.f.locals[id].ty, x.span)
                }
                _ => self.expr(x, Some(*pty))?,
            };
            if !matches!(h.kind, HExprKind::Local(_) | HExprKind::Field(..) | HExprKind::ElemField(..) | HExprKind::Index(..)) {
                return Err(err("only a variable, a field or an array element can be passed with `&`", x.span));
            }
            if let HExprKind::Index(_, i) | HExprKind::ElemField(_, i, _) = &h.kind
                && !matches!(i.kind, HExprKind::Word(_) | HExprKind::Local(_))
            {
                return Err(err("pass an element whose index is a number or a variable", x.span)
                    .with_note("store the index in a variable first: `i: int = ...; f(&v[i]);`"));
            }
            if h.ty != *pty {
                return Err(err(format!("parameter `{pname}` is &{pty}, found {}", h.ty), x.span)
                    .with_note("the value is written back, so the types must be the same"));
            }
            if seen.iter().any(|s| s.kind == h.kind) {
                return Err(err("the same value is passed with `&` twice", x.span)
                    .with_note("the proc would see it under two names"));
            }
            seen.push(h.clone());
            // After the call it may hold anything of its type.
            self.forget(x);
            out.push(HExpr { span: arg.span, ..h });
        }
        Ok(out)
    }

    /// Checks call arguments against `params` (`this` already checked).
    fn call_args(&mut self, shown: &str, params: &[(String, Type)], args: &[Expr], this: Option<HExpr>, span: Span) -> R<Vec<HExpr>> {
        if args.len() != params.len() {
            let s = if params.len() == 1 { "" } else { "s" };
            return Err(err(format!("`{shown}` takes {} argument{s}, found {}", params.len(), args.len()), span));
        }
        let mut hargs: Vec<HExpr> = this.into_iter().collect();
        for (arg, (pname, pty)) in args.iter().zip(params) {
            let h = match (pty, &arg.kind) {
                // A call that returns a borrow is already one: `ver(contas.get(0))`.
                (pt, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) if is_lent(*pt) && self.expr(arg, Some(*pt)).is_ok_and(|h| is_lent(h.ty)) => {
                    let h = self.expr(arg, Some(*pt))?;
                    HExpr { span: arg.span, ..self.coerce(h, *pt)? }
                }
                (Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_), ExprKind::Give(x)) => {
                    let lid = self.lookup(&x.name).ok_or_else(|| self.unknown(&x.name, x.span))?;
                    let h = self.give(lid, arg.span)?;
                    self.coerce(h, *pty)?
                }
                // `&x` for a value parameter (`a: &T` that became `a: int`) copies it.
                (pt, ExprKind::Lend(x)) if !is_lent(*pt) && !matches!(pt, Type::Own(_) | Type::Class(_) | Type::Iface(_)) && !pt.is_opt() || matches!(pt, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_)) && matches!(arg.kind, ExprKind::Lend(_)) => {
                    let h = self.read_in_place(x)?;
                    self.coerce(h, *pt)?
                }
                (Type::Own(_), _) => {
                    return Err(err(format!("parameter `{pname}` takes ownership of a buffer"), arg.span)
                        .with_note("pass it with `give b`"));
                }
                (Type::Lent(el), ExprKind::Lend(x)) => {
                    let h = self.lent_arg(x)?;
                    match h.ty {
                        Type::Own(e2) | Type::Lent(e2) if e2 == *el => HExpr { ty: *pty, span: arg.span, ..h },
                        t => {
                            let d = err(format!("expected {}, found {t}", Type::Lent(*el)), arg.span);
                            return Err(match t {
                                Type::Own(b) | Type::Lent(b) if b.elem == el.elem => d.with_note(
                                    "a `sym` array (indices -k..+k) and an array from 0 cannot be swapped: the same index means different cells",
                                ),
                                _ => d,
                            });
                        }
                    }
                }
                (Type::Lent(_), _) => {
                    return Err(err(format!("parameter `{pname}` borrows a buffer"), arg.span).with_note("pass it with `&b`"));
                }
                (Type::LentClass(c), ExprKind::Lend(x)) => {
                    let h = self.lent_arg(x)?;
                    match h.ty {
                        Type::Class(c2) | Type::LentClass(c2) if c2 == *c => HExpr { ty: *pty, span: arg.span, ..h },
                        t if t.is_opt() => return Err(self.maybe_empty(t, arg.span)),
                        t => return Err(err(format!("expected {pty}, found {t}"), arg.span)),
                    }
                }
                (Type::LentClass(_), _) => {
                    return Err(err(format!("parameter `{pname}` borrows an object"), arg.span).with_note("pass it with `&c`"));
                }
                (Type::LentOptClass(c), ExprKind::Lend(x)) => {
                    let h = self.lent_arg(x)?;
                    match h.ty {
                        Type::Class(c2) | Type::LentClass(c2) | Type::OptClass(c2) | Type::LentOptClass(c2) if c2 == *c => {
                            HExpr { ty: *pty, span: arg.span, ..h }
                        }
                        t => return Err(err(format!("expected {pty}, found {t}"), arg.span)),
                    }
                }
                (Type::LentOptIface(i), ExprKind::Lend(x)) => {
                    let h = self.lent_arg(x)?;
                    match h.ty {
                        Type::Iface(j) | Type::LentIface(j) | Type::OptIface(j) | Type::LentOptIface(j) if j == *i => {
                            HExpr { ty: *pty, span: arg.span, ..h }
                        }
                        Type::Class(c) | Type::LentClass(c) | Type::OptClass(c) | Type::LentOptClass(c) if self.ifaces[*i].impls.contains(&c) => {
                            hx(HExprKind::ToIface(Box::new(h), self.class_tags[c]), *pty, arg.span)
                        }
                        t => return Err(err(format!("expected {pty}, found {t}"), arg.span)),
                    }
                }
                (Type::LentOptClass(_) | Type::LentOptIface(_), _) => {
                    return Err(err(format!("parameter `{pname}` borrows an object"), arg.span).with_note("pass it with `&x`"));
                }
                (Type::LentIface(i), ExprKind::Lend(x)) => {
                    let h = self.lent_arg(x)?;
                    match h.ty {
                        Type::LentIface(j) | Type::Iface(j) if j == *i => HExpr { ty: *pty, span: arg.span, ..h },
                        Type::Class(c) | Type::LentClass(c) if self.ifaces[*i].impls.contains(&c) => {
                            let h = HExpr { ty: Type::LentClass(c), ..h };
                            hx(HExprKind::ToIface(Box::new(h), self.class_tags[c]), *pty, arg.span)
                        }
                        Type::Class(c) | Type::LentClass(c) => {
                            return Err(err(format!("`{c}` does not implement `{i}`"), arg.span)
                                .with_note(format!("add `@impl({i})` to class `{c}`")));
                        }
                        t => return Err(err(format!("expected {pty}, found {t}"), arg.span)),
                    }
                }
                (Type::LentIface(_), _) => {
                    return Err(err(format!("parameter `{pname}` borrows an object"), arg.span).with_note("pass it with `&x`"));
                }
                _ => self.expect(arg, *pty)?,
            };
            hargs.push(h);
        }
        // What the callee borrows, it may change: a `T?` field under it is
        // not proved any more (the callee could have emptied it).
        for h in &hargs {
            let inner = match &h.kind {
                HExprKind::ToIface(x, _) => x,
                _ => h,
            };
            if matches!(h.ty, Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_))
                && let Some(p) = path_of(inner)
            {
                self.invalidate(&p, false);
            }
        }
        Ok(hargs)
    }

    /// What `&x` / `&a.b` passes: a local array or object, or one held in a field.
    fn lent_arg(&mut self, x: &Expr) -> R<HExpr> {
        if let ExprKind::Var(name) = &x.kind {
            let lid = self.lookup(name).ok_or_else(|| self.unknown(name, x.span))?;
            let ty = self.f.locals[lid].ty;
            if !matches!(ty, Type::Own(_) | Type::Lent(_) | Type::Class(_) | Type::LentClass(_) | Type::Iface(_) | Type::LentIface(_)) && !ty.is_opt() {
                return Err(err(format!("only arrays and objects are borrowed; `{name}` is {ty}"), x.span)
                    .with_note("numbers, texts and structs are copied: pass them without `&`"));
            }
            return Ok(hx(HExprKind::Lend(lid), ty, x.span));
        }
        let h = self.expr(x, None)?;
        if !matches!(h.ty, Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_)) {
            return Err(err(format!("only arrays and objects are borrowed; this is {}", h.ty), x.span));
        }
        Ok(h)
    }
}

/// The target of an assignment: slots of a local (a struct field is an
/// offset), or cells of an array element.
// Built once per assignment and consumed right away: the size does not matter.
#[allow(clippy::large_enum_variant)]
enum LVal {
    Local { id: LocalId, off: usize, ty: Type },
    Elem { arr: HExpr, idx: HExpr, off: usize, ty: Type },
}

impl LVal {
    fn ty(&self) -> Type {
        match self {
            LVal::Local { ty, .. } | LVal::Elem { ty, .. } => *ty,
        }
    }
}

/// The targets of the assignments inside `s` (nested statements included).
fn assigned_in<'a>(s: &'a ast::Stmt, out: &mut Vec<&'a Expr>) {
    let block = |b: &'a ast::Block, out: &mut Vec<&'a Expr>| b.stmts.iter().for_each(|s| assigned_in(s, out));
    match &s.kind {
        StmtKind::Assign { target, .. } => out.push(target),
        StmtKind::Let { on_nat: Some(b), .. } => block(b, out),
        StmtKind::Tri { neg, zero, pos, .. } => [neg, zero, pos].into_iter().for_each(|b| block(b, out)),
        StmtKind::If { then, otherwise, .. } => {
            block(then, out);
            if let Some(b) = otherwise {
                block(b, out);
            }
        }
        StmtKind::Walk { body, .. } | StmtKind::While { body, .. } => block(body, out),
        StmtKind::For { init, step, body, .. } => {
            assigned_in(init, out);
            assigned_in(step, out);
            block(body, out);
        }
        StmtKind::Match { arms, .. } => arms.iter().for_each(|a| block(&a.body, out)),
        _ => {}
    }
}

/// `n.proximo` as written, for messages.
fn path_text(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Var(n) => n.clone(),
        ExprKind::Member(b, f) => format!("{}.{}", path_text(b), f.name),
        _ => "this".into(),
    }
}

/// Calls `f` on every expression in `b`, nested ones included.
fn visit_block(b: &ast::Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        visit_stmt(s, f);
    }
}

fn visit_stmt(s: &ast::Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Let { value, on_nat, .. } => {
            if let Some(v) = value {
                visit_expr(v, f);
            }
            if let Some(b) = on_nat {
                visit_block(b, f);
            }
        }
        StmtKind::Assign { target, value } => {
            visit_expr(target, f);
            visit_expr(value, f);
        }
        StmtKind::Return(Some(e)) | StmtKind::Expr(e) | StmtKind::Assert { cond: e, .. } => visit_expr(e, f),
        StmtKind::Return(None) | StmtKind::Burn(_) | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Tri { head, neg, zero, pos } => {
            visit_expr(head, f);
            for b in [neg, zero, pos] {
                visit_block(b, f);
            }
        }
        StmtKind::If { cond, then, otherwise } => {
            visit_expr(cond, f);
            visit_block(then, f);
            if let Some(b) = otherwise {
                visit_block(b, f);
            }
        }
        StmtKind::Walk { range, body, .. } => {
            match range {
                Range::Between(a, b) => {
                    visit_expr(a, f);
                    visit_expr(b, f);
                }
                Range::Span(k) => visit_expr(k, f),
            }
            visit_block(body, f);
        }
        StmtKind::Match { scrut, arms } => {
            visit_expr(scrut, f);
            for arm in arms {
                arm.pats.iter().for_each(|p| visit_expr(p, f));
                visit_block(&arm.body, f);
            }
        }
        StmtKind::While { cond, body } => {
            visit_expr(cond, f);
            visit_block(body, f);
        }
        StmtKind::For { init, cond, step, body } => {
            visit_stmt(init, f);
            visit_expr(cond, f);
            visit_stmt(step, f);
            visit_block(body, f);
        }
    }
}

fn visit_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Unary(_, a) | ExprKind::Cast(a, _) | ExprKind::Claim(a) | ExprKind::Member(a, _) => visit_expr(a, f),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) | ExprKind::Range(a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
        ExprKind::ArrayLit(items) | ExprKind::Call { args: items, .. } => items.iter().for_each(|a| visit_expr(a, f)),
        ExprKind::StructLit { fields, .. } => fields.iter().for_each(|(_, a)| visit_expr(a, f)),
        ExprKind::MethodCall { recv, args, .. } => {
            visit_expr(recv, f);
            args.iter().for_each(|a| visit_expr(a, f));
        }
        ExprKind::Lend(a) | ExprKind::GiveField(a) | ExprKind::Is(a, _) => visit_expr(a, f),
        ExprKind::Safe { recv, rest, .. } => {
            visit_expr(recv, f);
            visit_expr(rest, f);
        }
        ExprKind::Elvis(a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
        ExprKind::Num(_) | ExprKind::Real(_) | ExprKind::Char(_) | ExprKind::Str(_) | ExprKind::NaT | ExprKind::Var(_) | ExprKind::Bool(_) | ExprKind::Give(_) => {}
    }
}

/// Collects the operands of a `a + b + c` chain, left to right.
fn add_chain<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &e.kind {
        ExprKind::Binary(BinOp::Add, a, b) => {
            add_chain(a, out);
            add_chain(b, out);
        }
        _ => out.push(e),
    }
}

/// Checks one `printf` value against its format letter (`' '` for a bare `{x}`).
fn piece(spec: char, label: &str, h: HExpr) -> R<(Fmt, HExpr)> {
    let ty = h.ty;
    let buffer = matches!(ty, Type::Lent(_));
    // `{x}` and `%s` of a `T?` value print `null` or the value.
    let value_opt = matches!(ty, Type::OptScalar(_) | Type::OptBool | Type::OptText | Type::OptEnum(_));
    let ok = match spec {
        'd' | 'i' | 't' | 'v' => ty.is_int() || buffer,
        'f' | 'r' => ty == Type::Scalar(Scalar::Real),
        's' => matches!(ty, Type::Text | Type::Enum(_) | Type::Struct(_) | Type::Bool | Type::Class(_) | Type::LentClass(_) | Type::LentIface(_)) || value_opt,
        'c' => matches!(ty, Type::Scalar(Scalar::Tryte | Scalar::Trit) | Type::Lent(ast::Buf { elem: Elem::Scalar(Scalar::Tryte), .. })),
        _ => matches!(ty, Type::Scalar(_) | Type::Text | Type::Enum(_) | Type::Struct(_) | Type::Bool | Type::Class(_) | Type::LentClass(_) | Type::LentIface(_)) || buffer || value_opt,
    };
    if !ok {
        let wants = match spec {
            'd' | 'i' | 't' | 'v' => "an integer",
            'f' | 'r' => "a real",
            's' => "text",
            'c' => "a tryte",
            _ => "a number or text",
        };
        let found = match ty {
            Type::Lent(e) => format!("an array of {}", e.ty()),
            t => t.to_string(),
        };
        let d = err(format!("`{label}` prints {wants}, found {found}"), h.span);
        return Err(match spec {
            'c' if ty.is_int() => d.with_note("cast it: `(tryte) x`"),
            'd' | 'i' | 't' | 'v' if matches!(ty, Type::Enum(_) | Type::Bool) => d.with_note("cast it to read the number: `(int) x`"),
            'd' | 'i' | 't' | 'v' | 'f' | 'r' | 'c' if matches!(ty, Type::Struct(_)) => {
                d.with_note("print the whole struct with `%s` or `{x}`, or format one field: `p.x`")
            }
            'd' | 'i' if ty == Type::Scalar(Scalar::Real) => d.with_note("use `%f` for reals, or cast: `(int) x`"),
            'f' | 'r' if ty.is_int() => d.with_note("use `%d` for integers, or cast: `(real) x`"),
            _ => d,
        });
    }
    let fmt = match spec {
        't' => Fmt::Trits,
        'v' => Fmt::Hepta,
        'c' => Fmt::Char,
        _ => Fmt::Plain,
    };
    Ok((fmt, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::lex, parser::parse};

    fn check_src(src: &str) -> R<Hir> {
        check(&parse(lex(src).expect("lexes")).expect("parses"))
    }

    /// Checks `body` inside `proc main() { ... }` and returns the error message.
    fn err_in(body: &str) -> String {
        err_of(&format!("proc main() {{ {body} }}"))
    }

    fn err_of(src: &str) -> String {
        match check_src(src) {
            Ok(_) => panic!("expected an error for:\n{src}"),
            Err(d) => format!("{}{}", d.message, d.note.map(|n| format!(" | {n}")).unwrap_or_default()),
        }
    }

    fn ok(body: &str) -> Hir {
        check_src(&format!("proc main() {{ {body} }}")).unwrap_or_else(|d| panic!("{}: {body}", d.message))
    }

    const SPEC_EXAMPLE: &str = r#"
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int = sum(&b, 2) on NaT { burn b; return -1; };
    s = s << 1;
    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    return 0;
}
"#;

    #[test]
    fn spec_example_checks() {
        let hir = check_src(SPEC_EXAMPLE).unwrap();
        assert_eq!(hir.entry, 1);
        let sum = &hir.procs[0];
        assert_eq!(sum.params, 2);
        assert_eq!(sum.locals[2].name, "acc");
        // `acc += v[i]` became `acc = acc + v[i]`.
        let HStmtKind::Walk { body, .. } = &sum.body.stmts[1].kind else { panic!() };
        let HStmtKind::Assign { local: 2, index: None, value, .. } = &body.stmts[0].kind else { panic!() };
        assert!(matches!(value.kind, HExprKind::Binary(BinOp::Add, ..)));
    }

    #[test]
    fn literal_types() {
        ok("t: tryte = 364; u: trit = -; w: int = t; r: real = 2.5; c: tryte = 'A';");
        assert_eq!(err_in("t: tryte = 400;"), "`400` does not fit in a tryte (-364..=364)");
        assert_eq!(err_in("t: trit = 2;"), "`2` does not fit in a trit (-1..=1)");
        assert!(err_in("r: real = 5;").contains("write it with a point"));
    }

    #[test]
    fn mixing_and_narrowing() {
        assert!(err_in("x: int = 1; r: real = 2.0; y: int = x + r;").contains("cannot mix int and real"));
        assert!(err_in("w: int = 5; t: tryte = w;").contains("narrow explicitly with `as tryte`"));
        ok("w: int = 5; t: tryte = (tryte) w; r: real = (real) w; back: int = (int) r;");
        assert!(err_in("r: real = 2.0 % 1.0;").contains("`%` works on integers"));
        assert!(err_in("r: real = 2.0 << 1;").contains("`<<` works on integers"));
        ok("r: real = 2.0 ** 3; t: tryte = 3; u: tryte = t * 2;");
    }

    #[test]
    fn logic_and_tri() {
        assert!(err_in("x: int = 1; y: bool = x && x;").contains("`&&` works on bools and trits, found int"));
        ok("a: trit = +; b: trit = 0; c: trit = a && b || -;");
        assert!(err_in("x: int = 5; tri x { - => return; 0 => return; + => return; }").contains("`tri` needs a trit"));
        ok("x: int = 5; tri x <=> 0 { - => return; 0 => return; + => return; }");
    }

    #[test]
    fn names_and_scopes() {
        assert_eq!(err_in("y = 1;"), "unknown name `y`");
        // An inner `x` shadows the outer one only inside its block.
        let hir = ok("x: int = 1; t: trit = +; tri t { - => { x: int = 2; } 0 => return; + => return; } x = 3;");
        assert_eq!(hir.procs[0].locals.len(), 3);
        let HStmtKind::Assign { local, .. } = &hir.procs[0].body.stmts[3].kind else { panic!() };
        assert_eq!(*local, 0);
        assert!(err_in("walk i in 0..3 { i = 1; }").contains("read-only"));
        assert!(err_of("const L: int = 3; proc main() { L = 4; }").contains("cannot assign to const"));
        assert!(err_in("break;").contains("outside of a loop"));
    }

    #[test]
    fn buffers() {
        assert!(err_in("b: int = claim 5;").contains("needs to know the array type"));
        assert!(err_in("sym b[]: int = claim 5; c: int = b;").contains("cannot be used as a value"));
        assert!(err_in("x: int = 1; burn x;").contains("only an array or object you own can be burned"));
        assert!(err_of("proc f(sym &v[]: int) { burn v; } proc main() {}").contains("cannot burn a borrowed array"));
        assert!(err_of("proc f(sym &v[]: int) { v = v; } proc main() {}").contains("cannot replace something borrowed"));
        assert!(err_in("sym b[]: int = claim 3; x: int = &b;").contains("only works in a call argument"));
        assert!(err_in("b: int = {1, 2};").contains("needs to know the array type"));
        assert!(err_in("sym b[]: int = {1, 2}; burn b;").contains("odd number of values"));
        ok("a[]: int = {1, 2, 3, 4}; n: int = len(a); z[]: tryte = claim 0; burn a; burn z;");
        // literal indices are checked against arrays of known size
        assert!(err_in("sym a[]: int = {1, 2, 3}; x: int = a[2]; burn a;").contains("valid indices of `a` are -1..+1"));
        assert!(err_in("a[]: int = claim 4; a[4] = 1; burn a;").contains("are 0..3"));
        assert!(err_in("a[]: int = {1}; x: int = a[-1]; burn a;").contains("out of bounds"));
        assert!(err_in("a[]: int = claim 0; x: int = a[0]; burn a;").contains("it is empty"));
        ok("sym a[]: int = {1, 2, 3}; x: int = a[1] + a[-1]; i: int = 5; burn a; a = claim 11; y: int = a[5]; burn a;");
        ok("n: int = 3; a[]: int = claim n; x: int = a[7]; burn a;"); // size unknown here: checked at run time
        assert!(err_in("s: text = \"hi\"; s[0] = 'A';").contains("text is read-only"));
        ok("s: text = \"hi\"; c: tryte = s[1]; n: int = len(s); sym b[]: tryte = claim 3; b[0] = c; burn b;");
    }

    #[test]
    fn calls() {
        let defs = "proc take(sym b[]: int) { burn b; } proc peek(sym &v[]: int) -> int { return v[0]; }";
        assert!(err_of(&format!("{defs} proc main() {{ sym b[]: int = claim 3; take(b); }}")).contains("pass it with `give b`"));
        assert!(err_of(&format!("{defs} proc main() {{ sym b[]: int = claim 3; x: int = peek(b); }}")).contains("pass it with `&b`"));
        assert!(err_of(&format!("{defs} proc main() {{ x: int = peek(); }}")).contains("takes 1 argument, found 0"));
        assert!(err_of("proc mk() -> sym int[] { sym b[]: int = claim 1; return b; } proc main() { mk(); }").contains("would be lost"));
        assert!(err_in("x: int = 1; x + 1;").contains("does nothing"));
        assert!(err_in("x: int = print(\"a\");").contains("no value"));
        assert!(err_in("emit(1000);").contains("does not fit in a tryte"));
        ok("emit('A'); print(1.5); printl(\"hi\"); printl(); print(1 + 2); x: int? = input(); dump(\"here\");");
    }

    #[test]
    fn printf_checks() {
        ok("n: int = 1; t: tryte = 'A'; printf(\"{n} {n:t} {n:v} {t:c} %d %i %f %r %s %c\", n + 1, n, 2.5, 0.5, \"x\", 'b');");
        assert!(err_in("printf(\"%d\", 1.5);").contains("prints an integer, found real"));
        assert!(err_in("x: int = 1; printf(\"%r\", x);").contains("prints a real, found int"));
        assert!(err_in("x: int = 70; printf(\"%c\", x);").contains("prints a tryte, found int"));
        assert!(err_in("printf(\"%d %d\", 1);").contains("has no argument"));
        assert!(err_in("printf(\"oi\", 1);").contains("has no `%`"));
        assert!(err_in("printf(\"{}\");").contains("must hold a value"));
        assert!(err_in("printf(\"{a:q}\");").contains("unknown format `:q`"));
        assert!(err_in("printf(\"{a\");").contains("never closed"));
        assert!(err_in("printf(\"%x\", 1);").contains("unknown format `%x`"));
        ok("print(\"{x} 50% 'a' \\\"b\\\" ação\");");
        assert!(err_in("f: text = \"x\"; printf(f, 1);").contains("has no `%`"));
        ok("f: text = \"t\"; n: int = 1; printf(\"a \" + f + \" = %t {n}\" + \"!\", n);");
        assert!(err_in("n: int = 1; printf(\"a\" + n);").contains("joins text only, found int"));
    }

    #[test]
    fn enum_checks() {
        let e = |src: &str| err_of(src);
        assert!(e("enum A { X, Y } proc main() { a: A = A.Z; }").contains("has no value `Z`"));
        assert!(e("enum A { X, Y = 0 } proc main() {}").contains("both have the value 0"));
        assert!(e("enum A { X } enum A { Y } proc main() {}").contains("defined twice"));
        assert!(e("proc main() { c: Cor = 1; }").contains("unknown type `Cor`"));
        assert!(e("enum A { X } proc main() { a: A = 0; }").contains("expected A, found int"));
        assert!(e("enum A { X } enum B { Y } proc main() { t: trit = A.X == B.Y; }").contains("cannot compare A with B"));
        assert!(e("enum A { X } proc main() { printf(\"%d\", A.X); }").contains("cast it"));
        assert!(e("proc main() { x: int = 1; y: int = x.z; }").contains("int has no fields"));
        assert!(e("proc main() { match 5 { 1 => print(1); } }").contains("needs an `else` arm"));
        assert!(e("proc main() { match 5 { else => print(1); 1 => print(2); } }").contains("must be the last"));
        assert!(e("proc main() { match 5 { 1 | 1 => print(1); else => {} } }").contains("already appears"));
        assert!(e("proc main() { x: int = 2; match 5 { x => print(1); else => {} } }").contains("must be a literal or an enum value"));
        assert!(check_src("enum A { X, Y } proc f(a: A) -> A { return a; } proc main() { b: A = f(A.Y); print(b); }").is_ok());
    }

    #[test]
    fn if_checks() {
        ok("x: int = 1; if x > 0 { print(x); } else if x == 0 { } else { print(-x); }");
        assert!(err_in("x: int = 1; if x { }").contains("`if` needs a bool, found int"));
        assert!(err_in("x: int = 1; y: int = !x;").contains("`!` works on bools and trits"));
        assert!(err_in("x: int = 1; while x { }").contains("`while` needs a bool, found int"));
        ok("i: int = 0; while i < 3 { i = i + 1; }");
        assert!(err_in("x: int = 1; t: trit = x == \"a\";").contains("compared with text, found int"));
        assert!(err_in("x: real = 1.5; t: trit = x < 2;").contains("integer literal where a real is expected"));
    }

    #[test]
    fn print_buffers() {
        ok("sym b[]: int = claim 3; sym t[]: tryte = claim 1; printl(b); printf(\"{b} {b:t} %v {t:c} %c\", b, t); s: text = toText(b, 'v'); burn b; burn t;");
        assert!(err_in("sym b[]: int = claim 3; printf(\"{b:c}\"); burn b;").contains("found an array of int"));
        assert!(err_in("sym b[]: int = claim 3; printf(\"%f\", b); burn b;").contains("prints a real"));
    }

    #[test]
    fn final_locals() {
        ok("final x: int = input() on NaT { return; }; final y: int = x + 1; print(y);");
        ok("final sym b[]: int = claim 3; b[0] = 5; burn b;");
        ok("final x: int = 1; tri x <=> 0 { - => {} 0 => {} + => { x: int = 2; print(x); } }");
        assert!(err_in("final x: int = 1; x = 2;").contains("`x` is final and cannot change"));
        assert!(err_in("final x: int = 1; x += 2;").contains("is final"));
        assert!(err_in("final x: int = 1; x: int = 2;").contains("cannot be declared again"));
    }

    #[test]
    fn to_text_checks() {
        ok("n: int = 5; a: text = toText(n); b: text = toText(n, 't'); c: text = toText(1.5); print(\"x\" + toText(n, 'v'));");
        assert!(err_in("x: text = toText(1.5, 't');").contains("takes an integer, found real"));
        assert!(err_in("x: text = toText(\"a\");").contains("takes a number, found text"));
        assert!(err_in("x: text = toText(5, 'x');").contains("must be 't' (ternary) or 'v'"));
        assert!(err_in("x: text = toText();").contains("1 or 2 arguments"));
    }

    #[test]
    fn text_concat() {
        ok("name: text = \"Ana\"; print(\"hi \" + name + \"!\\n\");");
        assert!(err_in("x: int = 5; print(\"x = \" + x);").contains("joins text with text only, found int"));
        ok("t: text = \"a\" + \"b\"; u: text = t + t; same: trit = t == u;");
        assert!(err_in("print(\"a\" + 1.5);").contains("found real"));
    }

    #[test]
    fn structure() {
        assert!(err_of("proc f() {}").contains("no `proc main`"));
        assert!(err_of("proc main(x: int) {}").contains("cannot take parameters"));
        assert!(err_of("proc f() {} proc f() {} proc main() {}").contains("defined twice"));
        assert!(err_of("proc print() {} proc main() {}").contains("built-in"));
        assert!(err_of("proc f() -> int { x: int = 1; } proc main() {}").contains("must end with `return`"));
        assert!(err_in("return; x: int = 1;").contains("unreachable code"));
        assert!(err_in("x: int = input() on NaT { print(\"bad\"); };").contains("must end with `return`"));
        ok("x: int = input() on NaT { return; }; print(x);");
    }

    #[test]
    fn consts() {
        let hir = check_src("const LIMIT: tryte = -100; const NAME: text = \"Kestrel\"; proc main() { x: tryte = LIMIT; print(NAME); }").unwrap();
        assert_eq!(hir.procs[0].locals[0].ty, Type::Scalar(Scalar::Tryte));
        assert!(err_of("const X: int = 1 + 2; proc main() {}").contains("must be a literal"));
    }
}
