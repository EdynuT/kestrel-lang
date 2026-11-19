//! Name resolution and type checking: AST -> [`Hir`] (SPEC §1-§8).
//!
//! Ownership rules (use after give, leaks, double burn, tri symmetry) are
//! checked afterwards by the linear checker on the HIR.

use std::collections::HashMap;

use kestrel_core::charset::char_to_tryte;
use kestrel_core::word::{TRIT_MAX, TRYTE_MAX};
use kestrel_core::Word;

use crate::ast::{self, Expr, ExprKind, Item, Place, Program, Range, StmtKind, UnOp};
use crate::diag::{Diagnostic, Span};
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

const BUILTINS: [&str; 8] = ["print", "printl", "printf", "to_text", "emit", "input", "len", "dump"];

pub fn check(program: &Program) -> R<Hir> {
    let mut cx = Checker {
        sigs: Vec::new(),
        proc_ids: HashMap::new(),
        consts: HashMap::new(),
        enums: HashMap::new(),
        f: FnCx::new(Type::Void),
    };
    // Enums first, so types like `c: Cor` resolve anywhere. Each enum gets a
    // hidden proc that turns a value into its name, for `print` and `to_text`.
    let mut name_procs = Vec::new();
    for item in &program.items {
        if let Item::Enum(e) = item {
            name_procs.push(cx.declare_enum(e)?);
        }
    }
    for item in &program.items {
        match item {
            Item::Const(c) => cx.declare_const(c)?,
            Item::Proc(p) => cx.declare_proc(p)?,
            Item::Enum(_) => {}
        }
    }
    for p in &name_procs {
        cx.declare_proc(p)?;
    }
    let entry = *cx
        .proc_ids
        .get("main")
        .ok_or_else(|| Diagnostic::new("the program has no `proc main`", Span::default()))?;
    let main = &cx.sigs[entry];
    if !main.params.is_empty() {
        return Err(Diagnostic::new("`main` cannot take parameters", main.span));
    }
    if !matches!(main.ret, Type::Void | Type::Scalar(Scalar::Int)) {
        return Err(Diagnostic::new(format!("`main` must yield an int or nothing, not {}", main.ret), main.span));
    }
    let mut procs = Vec::new();
    for item in &program.items {
        if let Item::Proc(p) = item {
            procs.push(cx.check_proc(p)?);
        }
    }
    for p in &name_procs {
        procs.push(cx.check_proc(p)?);
    }
    Ok(Hir { procs, entry })
}

#[derive(Clone)]
struct Sig {
    params: Vec<(String, Type)>,
    ret: Type,
    span: Span,
}

struct FnCx {
    locals: Vec<Local>,
    scopes: Vec<Vec<(String, LocalId)>>,
    ret: Type,
    loops: usize,
}

impl FnCx {
    fn new(ret: Type) -> FnCx {
        FnCx { locals: Vec::new(), scopes: vec![Vec::new()], ret, loops: 0 }
    }
}

struct Checker {
    sigs: Vec<Sig>,
    proc_ids: HashMap<String, ProcId>,
    consts: HashMap<String, HExpr>,
    enums: HashMap<String, EnumInfo>,
    f: FnCx,
}

struct EnumInfo {
    name: &'static str,
    variants: Vec<(String, i64)>,
    /// Hidden proc `$name.E(v: int) -> text`.
    name_proc: String,
}

fn err(msg: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(msg, span)
}

fn fits(from: Type, to: Type) -> bool {
    match (from, to) {
        (Type::Scalar(a), Type::Scalar(b)) if a.is_int() && b.is_int() => a.rank() <= b.rank(),
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
        if !matches!(c.ty.ty, Type::Scalar(_) | Type::Text) {
            return Err(err("a const must be a number or text", c.ty.span));
        }
        let value = self.expect(&c.value, c.ty.ty)?;
        if !matches!(value.kind, HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_)) {
            return Err(err("a const value must be a literal", c.value.span));
        }
        // The const takes its declared type, so `const T: tryte = 5;` is a tryte.
        let value = HExpr { ty: c.ty.ty, ..value };
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
        // proc $name.E(v: int) -> text { if v == k0 { yield "A"; } ... yield "Last"; }
        let sp = e.name.span;
        let ex = |kind| Expr { kind, span: sp };
        let st = |kind| ast::Stmt { kind, span: sp };
        let (last, rest) = variants.split_last().expect("checked non-empty");
        let mut stmts = Vec::new();
        for (n, k) in rest {
            let v = ex(ExprKind::Var("v".into()));
            let cond = ex(ExprKind::Binary(BinOp::Eq, Box::new(v), Box::new(ex(ExprKind::Num(Word::new(*k))))));
            let then = ast::Block { stmts: vec![st(StmtKind::Yield(Some(ex(ExprKind::Str(n.clone())))))], span: sp };
            stmts.push(st(StmtKind::If { cond, then, otherwise: None }));
        }
        stmts.push(st(StmtKind::Yield(Some(ex(ExprKind::Str(last.0.clone()))))));
        let name_proc = format!("$name.{name}");
        let int = ast::TypeExpr { ty: Type::Scalar(Scalar::Int), span: sp };
        let decl = ast::ProcDecl {
            name: ast::Ident { name: name_proc.clone(), span: sp },
            params: vec![ast::Param { name: ast::Ident { name: "v".into(), span: sp }, ty: int }],
            ret: Some(ast::TypeExpr { ty: Type::Text, span: sp }),
            body: ast::Block { stmts, span: sp },
        };
        self.enums.insert(name.clone(), EnumInfo { name: ast::intern(name), variants, name_proc });
        Ok(decl)
    }

    /// Rejects enum types that were never declared.
    fn known(&self, t: &ast::TypeExpr) -> R<()> {
        match t.ty {
            Type::Enum(n) if !self.enums.contains_key(n) => Err(err(format!("unknown type `{n}`"), t.span)
                .with_note("types are trit, tryte, int, real, text, own/lent [T] or a declared enum")),
            _ => Ok(()),
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
        for param in &p.params {
            if params.iter().any(|(n, _)| *n == param.name.name) {
                return Err(err(format!("parameter `{}` appears twice", param.name.name), param.name.span));
            }
            self.known(&param.ty)?;
            params.push((param.name.name.clone(), param.ty.ty));
        }
        if let Some(t) = &p.ret {
            self.known(t)?;
        }
        let ret = p.ret.map_or(Type::Void, |t| t.ty);
        if let (Type::Lent(_), Some(t)) = (ret, p.ret) {
            return Err(err("a proc cannot yield a lent buffer", t.span).with_note("lent buffers only live as long as the call"));
        }
        self.proc_ids.insert(name.clone(), self.sigs.len());
        self.sigs.push(Sig { params, ret, span: p.name.span });
        Ok(())
    }

    fn check_proc(&mut self, p: &ast::ProcDecl) -> R<HProc> {
        let sig = self.sigs[self.proc_ids[&p.name.name]].clone();
        self.f = FnCx::new(sig.ret);
        for (param, (_, ty)) in p.params.iter().zip(&sig.params) {
            self.new_local(&param.name.name, *ty, LocalKind::Param, param.name.span);
        }
        let body = self.block(&p.body)?;
        if sig.ret != Type::Void && !body.diverges() {
            let end = Span::new(p.body.span.end - 1, p.body.span.end);
            return Err(err(format!("`{}` must end with `yield` (it returns {})", p.name.name, sig.ret), end));
        }
        let f = std::mem::replace(&mut self.f, FnCx::new(Type::Void));
        Ok(HProc { name: p.name.name.clone(), params: p.params.len(), ret: sig.ret, locals: f.locals, body })
    }

    // ---- scopes ----

    fn new_local(&mut self, name: &str, ty: Type, kind: LocalKind, span: Span) -> LocalId {
        let id = self.f.locals.len();
        self.f.locals.push(Local { name: name.to_string(), ty, kind, span, is_final: false });
        if kind != LocalKind::Hidden {
            self.f.scopes.last_mut().expect("a scope is open").push((name.to_string(), id));
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.f.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name).map(|(_, id)| *id)
    }

    fn unknown(&self, name: &str, span: Span) -> Diagnostic {
        if self.proc_ids.contains_key(name) {
            err(format!("`{name}` is a proc; call it with `{name}(...)`"), span)
        } else {
            err(format!("unknown name `{name}`"), span)
        }
    }

    fn block(&mut self, b: &ast::Block) -> R<HBlock> {
        self.f.scopes.push(Vec::new());
        let mut stmts: Vec<HStmt> = Vec::new();
        for s in &b.stmts {
            if let Some(prev) = stmts.last() {
                let diverged = HBlock { stmts: vec![prev.clone()], scope: vec![], span: prev.span }.diverges();
                if diverged {
                    return Err(err("unreachable code", s.span).with_note("nothing runs after `yield`, `break` or `continue`"));
                }
            }
            stmts.push(self.stmt(s)?);
        }
        let scope = self.f.scopes.pop().expect("scope pushed above").into_iter().map(|(_, id)| id).collect();
        Ok(HBlock { stmts, scope, span: b.span })
    }

    // ---- statements ----

    fn stmt(&mut self, s: &ast::Stmt) -> R<HStmt> {
        let kind = match &s.kind {
            StmtKind::Let { is_final, name, ty, value, on_nat } => {
                let (value, lty) = match ty {
                    Some(t) => {
                        self.known(t)?;
                        if let Type::Lent(_) = t.ty {
                            return Err(err("a variable cannot hold a lent buffer", t.span)
                                .with_note("lent buffers exist only as proc parameters"));
                        }
                        (self.expect(value, t.ty)?, t.ty)
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
                        if !value.ty.is_numeric() {
                            return Err(err(format!("`on NaT` needs a number, found {}", value.ty), value.span));
                        }
                        let hb = self.block(b)?;
                        if !hb.diverges() {
                            return Err(err("the `on NaT` block must end with `yield`, `break` or `continue`", b.span)
                                .with_note("after it, the variable is guaranteed not to be NaT"));
                        }
                        Some(hb)
                    }
                    None => None,
                };
                let in_scope = self.f.scopes.last().and_then(|s| s.iter().rev().find(|(n, _)| *n == name.name));
                if let Some(&(_, prev)) = in_scope
                    && self.f.locals[prev].is_final
                {
                    return Err(err(format!("`{}` is final and cannot be declared again", name.name), name.span)
                        .with_note("pick another name, or declare the first one without `final`"));
                }
                let local = self.new_local(&name.name, lty, LocalKind::Let, name.span);
                self.f.locals[local].is_final = *is_final;
                HStmtKind::Let { local, value, on_nat }
            }
            StmtKind::Assign { place, value } => self.assign(place, value)?,
            StmtKind::Burn(name) => {
                let id = self.lookup(&name.name).ok_or_else(|| self.unknown(&name.name, name.span))?;
                match self.f.locals[id].ty {
                    Type::Own(_) => HStmtKind::Burn(id),
                    Type::Lent(_) => {
                        return Err(err("cannot burn a lent buffer", name.span).with_note("only its owner can burn it"));
                    }
                    t => return Err(err(format!("only an own buffer can be burned, `{}` is {t}", name.name), name.span)),
                }
            }
            StmtKind::Yield(v) => match (self.f.ret, v) {
                (Type::Void, None) => HStmtKind::Yield(None),
                (Type::Void, Some(e)) => {
                    return Err(err("this proc has no return type, so it yields nothing", e.span).with_note("write `yield;`"));
                }
                (t, None) => return Err(err(format!("this proc must yield a {t}"), s.span)),
                (Type::Own(e), Some(Expr { kind: ExprKind::Var(name), span })) => {
                    // `yield b` hands ownership back to the caller.
                    let id = self.lookup(name).ok_or_else(|| self.unknown(name, *span))?;
                    let h = self.give(id, *span)?;
                    HStmtKind::Yield(Some(self.coerce(h, Type::Own(e))?))
                }
                (t, Some(e)) => HStmtKind::Yield(Some(self.expect(e, t)?)),
            },
            StmtKind::Break | StmtKind::Continue => {
                if self.f.loops == 0 {
                    let word = if s.kind == StmtKind::Break { "break" } else { "continue" };
                    return Err(err(format!("`{word}` outside of a loop"), s.span)
                        .with_note("`break` and `continue` work inside `walk` and `while`"));
                }
                if s.kind == StmtKind::Break { HStmtKind::Break } else { HStmtKind::Continue }
            }
            StmtKind::If { cond, then, otherwise } => {
                let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`if` needs a trit, found {}", h.ty), cond.span)
                        .with_note("compare first, e.g. `if x > 0 { ... }`"));
                }
                // `if` is a `tri` whose `0` arm traps: an unknown condition is an error.
                let then = self.block(then)?;
                let otherwise = match otherwise {
                    Some(b) => self.block(b)?,
                    None => HBlock { stmts: vec![], scope: vec![], span: s.span },
                };
                let trap = HStmt { kind: HStmtKind::Trap("if condition is 0 (unknown)".into()), span: cond.span };
                let unknown = HBlock { stmts: vec![trap], scope: vec![], span: cond.span };
                HStmtKind::Tri { head: h, arms: Box::new([otherwise, unknown, then]), origin: Branch::If }
            }
            StmtKind::Tri { head, neg, zero, pos } => {
                let h = self.expr(head, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`tri` needs a trit, found {}", h.ty), head.span)
                        .with_note("compare first, e.g. `tri x <=> 0 { ... }`"));
                }
                let arms = [self.block(neg)?, self.block(zero)?, self.block(pos)?];
                HStmtKind::Tri { head: h, arms: Box::new(arms), origin: Branch::Tri }
            }
            StmtKind::Match { scrut, arms } => self.match_stmt(scrut, arms, s.span)?,
            StmtKind::While { cond, body } => {
                let h = self.expr(cond, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`while` needs a trit, found {}", h.ty), cond.span)
                        .with_note("compare first, e.g. `while i < 10 { ... }`"));
                }
                self.f.loops += 1;
                let body = self.block(body);
                self.f.loops -= 1;
                HStmtKind::While { cond: h, body: body? }
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
                self.f.scopes.push(Vec::new());
                let v = self.new_local(&var.name, word, LocalKind::WalkVar, var.span);
                self.f.loops += 1;
                let body = self.block(body);
                self.f.loops -= 1;
                self.f.scopes.pop();
                HStmtKind::Walk { var: v, span_tmp, start, end, end_tmp, body: body? }
            }
            StmtKind::Expr(e) => {
                if let ExprKind::Call { name, args } = &e.kind
                    && name.name == "dump"
                {
                    return match args.as_slice() {
                        [Expr { kind: ExprKind::Str(label), .. }] => Ok(HStmt { kind: HStmtKind::Dump(label.clone()), span: s.span }),
                        _ => Err(err("`dump` takes one string literal: `dump(\"label\");`", e.span)),
                    };
                }
                let h = self.expr(e, None)?;
                if !matches!(h.kind, HExprKind::Call(..) | HExprKind::Builtin(..) | HExprKind::Print(_)) {
                    return Err(err("this expression does nothing", e.span).with_note("only calls can be used as statements"));
                }
                if let Type::Own(_) = h.ty {
                    return Err(err("the buffer returned by this call would be lost", e.span)
                        .with_note("store it, e.g. `b: own [int] = f(...);`, then burn it"));
                }
                HStmtKind::Expr(h)
            }
        };
        Ok(HStmt { kind, span: s.span })
    }

    fn assign(&mut self, place: &Place, value: &Expr) -> R<HStmtKind> {
        let name = &place.name.name;
        let Some(id) = self.lookup(name) else {
            if self.consts.contains_key(name) {
                return Err(err(format!("cannot assign to const `{name}`"), place.name.span));
            }
            return Err(self.unknown(name, place.name.span));
        };
        let local = &self.f.locals[id];
        if local.kind == LocalKind::WalkVar {
            return Err(err(format!("`{name}` is the walk variable and is read-only"), place.name.span));
        }
        if local.is_final && place.index.is_none() {
            return Err(err(format!("`{name}` is final and cannot change"), place.name.span)
                .with_note("declare it without `final` if it needs to change"));
        }
        match (&place.index, local.ty) {
            (Some(idx), Type::Own(e) | Type::Lent(e)) => {
                let hidx = self.index(idx)?;
                let hv = self.expect(value, Type::Scalar(e.scalar()))?;
                Ok(HStmtKind::Assign { local: id, index: Some(hidx), value: hv })
            }
            (Some(_), Type::Text) => Err(err("text is read-only", place.span)),
            (Some(_), t) => Err(err(format!("cannot index a {t}"), place.name.span)),
            (None, Type::Lent(_)) => Err(err("cannot replace a lent buffer", place.span)
                .with_note("assign to its cells instead: `v[i] = ...`")),
            (None, t) => {
                let hv = self.expect(value, t)?;
                Ok(HStmtKind::Assign { local: id, index: None, value: hv })
            }
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
        let d = err(format!("expected {ty}, found {}", h.ty), h.span);
        Err(match (h.ty, ty) {
            (Type::Scalar(Scalar::Real), t) | (t, Type::Scalar(Scalar::Real)) if t.is_int() => {
                d.with_note(format!("convert explicitly with `as {ty}`"))
            }
            (from, to) if from.is_int() && to.is_int() => {
                d.with_note(format!("narrow explicitly with `as {to}` (values out of range become NaT)"))
            }
            _ => d,
        })
    }

    fn index(&mut self, idx: &Expr) -> R<HExpr> {
        let h = self.expr(idx, Some(Type::Scalar(Scalar::Int)))?;
        if !h.ty.is_int() {
            return Err(err(format!("an index must be an integer, found {}", h.ty), idx.span));
        }
        Ok(h)
    }

    fn give(&self, id: LocalId, span: Span) -> R<HExpr> {
        match self.f.locals[id].ty {
            Type::Own(e) => Ok(hx(HExprKind::Give(id), Type::Own(e), span)),
            Type::Lent(_) => Err(err("cannot give away a lent buffer", span).with_note("only its owner can give it")),
            t => Err(err(format!("only an own buffer can be given, this is {t}"), span)),
        }
    }

    fn numeric(&self, h: &HExpr) -> R<()> {
        if h.ty.is_numeric() { Ok(()) } else { Err(err(format!("expected a number, found {}", h.ty), h.span)) }
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
            ExprKind::NaT => match want {
                Some(Type::Scalar(Scalar::Real)) => hx(HExprKind::Real(kestrel_core::Real::NAT), Type::Scalar(Scalar::Real), span),
                Some(t @ Type::Scalar(_)) => hx(HExprKind::Word(kestrel_core::Word::NAT), t, span),
                _ => hx(HExprKind::Word(kestrel_core::Word::NAT), Type::Scalar(Scalar::Int), span),
            },
            ExprKind::Var(name) => {
                if let Some(id) = self.lookup(name) {
                    let ty = self.f.locals[id].ty;
                    if let Type::Own(_) | Type::Lent(_) = ty {
                        return Err(err(format!("buffer `{name}` cannot be used as a value"), span)
                            .with_note(format!("index it (`{name}[i]`), pass it with `lend {name}` or `give {name}`, or burn it")));
                    }
                    hx(HExprKind::Local(id), ty, span)
                } else if let Some(c) = self.consts.get(name) {
                    HExpr { span, ..c.clone() }
                } else {
                    return Err(self.unknown(name, span));
                }
            }
            ExprKind::Unary(op, inner) => {
                let h = self.expr(inner, want.filter(|t| t.is_numeric()))?;
                self.numeric(&h)?;
                let ty = h.ty;
                if *op == UnOp::Not && ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`!` works on trits, found {ty}"), h.span)
                        .with_note("to flip the sign of a number use `-x`"));
                }
                match h.kind {
                    HExprKind::Word(w) => hx(HExprKind::Word(-w), ty, span),
                    HExprKind::Real(r) => hx(HExprKind::Real(-r), ty, span),
                    _ => hx(HExprKind::Neg(Box::new(h)), ty, span),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, span)?,
            ExprKind::Member(base, field) => {
                let note = "`.` only works on enum names for now, like `Cor.Verde`";
                let ExprKind::Var(tname) = &base.kind else {
                    return Err(err("cannot use `.` here", span).with_note(note));
                };
                let Some(info) = self.enums.get(tname) else {
                    return Err(err(format!("`{tname}` is not an enum"), base.span).with_note(note));
                };
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
                if !h.ty.is_numeric() {
                    return Err(err(format!("cannot cast {}", h.ty), inner.span));
                }
                let to = Type::Scalar(*s);
                if h.ty == to { h } else { hx(HExprKind::Convert(Box::new(h)), to, span) }
            }
            ExprKind::Index(base, idx) => {
                if let ExprKind::Var(name) = &base.kind
                    && let Some(id) = self.lookup(name)
                    && let ty @ (Type::Own(el) | Type::Lent(el)) = self.f.locals[id].ty
                {
                    let hidx = self.index(idx)?;
                    let hb = hx(HExprKind::Local(id), ty, base.span);
                    return Ok(hx(HExprKind::Index(Box::new(hb), Box::new(hidx)), Type::Scalar(el.scalar()), span));
                }
                let hb = self.expr(base, None)?;
                if hb.ty != Type::Text {
                    return Err(err(format!("cannot index {}", hb.ty), base.span));
                }
                let hidx = self.index(idx)?;
                hx(HExprKind::Index(Box::new(hb), Box::new(hidx)), Type::Scalar(Scalar::Tryte), span)
            }
            ExprKind::Call { name, args } => self.call(name, args, span)?,
            ExprKind::Claim(size) => match want {
                Some(Type::Own(el)) => {
                    let n = self.index(size)?;
                    hx(HExprKind::Claim(Box::new(n)), Type::Own(el), span)
                }
                _ => {
                    return Err(err("`claim` needs to know the buffer type", span)
                        .with_note("declare it, e.g. `b: own [int] = claim 5;`"));
                }
            },
            ExprKind::Give(name) => {
                let id = self.lookup(&name.name).ok_or_else(|| self.unknown(&name.name, name.span))?;
                self.give(id, span)?
            }
            ExprKind::Lend(_) => {
                return Err(err("`lend` is only allowed as a call argument", span));
            }
        })
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, want: Option<Type>, span: Span) -> R<HExpr> {
        let trit = Type::Scalar(Scalar::Trit);
        let (ha, hb, ty) = match op {
            BinOp::And | BinOp::Or => {
                let ha = self.expr(a, Some(trit))?;
                let hb = self.expr(b, Some(trit))?;
                for h in [&ha, &hb] {
                    if h.ty != trit {
                        return Err(err(format!("`{}` works on trits, found {}", op.symbol(), h.ty), h.span)
                            .with_note("to compare numbers use `<=>`"));
                    }
                }
                (ha, hb, trit)
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
                (ha, hb, trit)
            }
            BinOp::Cmp3 | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let (ha, hb, _) = self.numeric_pair(a, b, None)?;
                (ha, hb, trit)
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
                let (ha, hb, ty) = self.numeric_pair(a, b, want).map_err(|d| {
                    if op == BinOp::Add && d.message.ends_with("found text") {
                        d.with_note("text can be joined with `+` only inside `print`")
                    } else {
                        d
                    }
                })?;
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
                            hx(HExprKind::Binary(BinOp::Eq, Box::new(local()), Box::new(k)), trit, scrut.span)
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

    /// The enum type of `e`, if it has one.
    fn enum_type(&mut self, e: &Expr) -> Option<Type> {
        self.expr(e, None).ok().map(|h| h.ty).filter(|t| matches!(t, Type::Enum(_)))
    }

    /// Like `expr`, but a buffer name is lent so `print`/`printf`/`to_text`
    /// can show its cells.
    fn shown(&mut self, e: &Expr, want: Option<Type>) -> R<HExpr> {
        if let ExprKind::Var(name) = &e.kind
            && let Some(id) = self.lookup(name)
            && let Type::Own(el) | Type::Lent(el) = self.f.locals[id].ty
        {
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
                    let (name, spec) = inner.split_once(':').unwrap_or((&inner, ""));
                    let spec = match spec {
                        "" => ' ',
                        "t" | "v" | "c" => spec.chars().next().unwrap_or(' '),
                        _ => {
                            return Err(err(format!("unknown format `:{spec}` in `{label}`"), pspan)
                                .with_note("use `:t` (ternary), `:v` (heptavintimal) or `:c` (character)"));
                        }
                    };
                    let ident = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if !ident {
                        return Err(err(format!("`{label}` must hold a variable name"), pspan)
                            .with_note("for expressions use `%` and pass them as arguments: `printf(\"%d\", a + b)`"));
                    }
                    let h = self.shown(&Expr { kind: ExprKind::Var(name.to_string()), span: pspan }, None)?;
                    let (f, h) = piece(spec, &label, h)?;
                    out.push((f, self.named(h)));
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
                    out.push((f, self.named(h)));
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
                let mut pieces = Vec::new();
                if !(name.name == "printl" && args.is_empty()) {
                    arity(1)?;
                    if self.is_text_chain(&args[0]) {
                        let mut texts = Vec::new();
                        self.text_pieces(&args[0], &mut texts)?;
                        pieces.extend(texts.into_iter().map(|h| (Fmt::Plain, h)));
                    } else {
                        let a = self.shown(&args[0], None)?;
                        if !matches!(a.ty, Type::Scalar(_) | Type::Text | Type::Lent(_) | Type::Enum(_)) {
                            return Err(err(format!("`{}` prints numbers and text, found {}", name.name, a.ty), a.span));
                        }
                        pieces.push((Fmt::Plain, self.named(a)));
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
            "to_text" => {
                let (x, base) = match args {
                    [x] => (x, None),
                    [x, b] => (x, Some(b)),
                    _ => return Err(err(format!("`to_text` takes 1 or 2 arguments, found {}", args.len()), span)),
                };
                let fmt = match base.map(|b| &b.kind) {
                    None => Fmt::Plain,
                    Some(ExprKind::Char(c)) if char_to_tryte('t') == Some(*c) => Fmt::Trits,
                    Some(ExprKind::Char(c)) if char_to_tryte('v') == Some(*c) => Fmt::Hepta,
                    Some(_) => {
                        return Err(err("the base of `to_text` must be 't' (ternary) or 'v' (heptavintimal)", base.map_or(span, |b| b.span))
                            .with_note("leave it out for decimal: `to_text(x)`"));
                    }
                };
                let a = self.shown(x, None)?;
                if fmt == Fmt::Plain && matches!(a.ty, Type::Enum(_)) {
                    return Ok(self.named(a));
                }
                let ok = match fmt {
                    Fmt::Plain => matches!(a.ty, Type::Scalar(_) | Type::Lent(_)),
                    _ => a.ty.is_int() || matches!(a.ty, Type::Lent(_)),
                };
                if !ok {
                    let wants = if fmt == Fmt::Plain { "a number" } else { "an integer" };
                    return Err(err(format!("`to_text` takes {wants}, found {}", a.ty), a.span));
                }
                return Ok(builtin(Builtin::ToText(fmt), vec![a], text));
            }
            "emit" => {
                arity(1)?;
                let a = self.expect(&args[0], Type::Scalar(Scalar::Tryte))?;
                return Ok(builtin(Builtin::Emit, vec![a], Type::Void));
            }
            "input" => {
                arity(0)?;
                return Ok(builtin(Builtin::Input, vec![], Type::Scalar(Scalar::Int)));
            }
            "len" => {
                arity(1)?;
                let a = self.expect(&args[0], text)?;
                return Ok(builtin(Builtin::Len, vec![a], Type::Scalar(Scalar::Int)));
            }
            "dump" => return Err(err("`dump(...)` is a statement, not a value", span)),
            _ => {}
        }
        let Some(&id) = self.proc_ids.get(&name.name) else {
            return Err(if self.lookup(&name.name).is_some() {
                err(format!("`{}` is a variable, not a proc", name.name), name.span)
            } else {
                err(format!("unknown proc `{}`", name.name), name.span)
            });
        };
        let sig = self.sigs[id].clone();
        arity(sig.params.len())?;
        let mut hargs = Vec::new();
        for (arg, (pname, pty)) in args.iter().zip(&sig.params) {
            let h = match (pty, &arg.kind) {
                (Type::Own(_), ExprKind::Give(x)) => {
                    let lid = self.lookup(&x.name).ok_or_else(|| self.unknown(&x.name, x.span))?;
                    let h = self.give(lid, arg.span)?;
                    self.coerce(h, *pty)?
                }
                (Type::Own(_), _) => {
                    return Err(err(format!("parameter `{pname}` takes ownership of a buffer"), arg.span)
                        .with_note("pass it with `give b`"));
                }
                (Type::Lent(el), ExprKind::Lend(x)) => {
                    let lid = self.lookup(&x.name).ok_or_else(|| self.unknown(&x.name, x.span))?;
                    match self.f.locals[lid].ty {
                        Type::Own(e2) | Type::Lent(e2) if e2 == *el => hx(HExprKind::Lend(lid), *pty, arg.span),
                        t => return Err(err(format!("expected a buffer of {}, found {t}", el.scalar()), arg.span)),
                    }
                }
                (Type::Lent(_), _) => {
                    return Err(err(format!("parameter `{pname}` borrows a buffer"), arg.span).with_note("pass it with `lend b`"));
                }
                _ => self.expect(arg, *pty)?,
            };
            hargs.push(h);
        }
        Ok(hx(HExprKind::Call(id, hargs), sig.ret, span))
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
    let ok = match spec {
        'd' | 'i' | 't' | 'v' => ty.is_int() || buffer,
        'f' | 'r' => ty == Type::Scalar(Scalar::Real),
        's' => matches!(ty, Type::Text | Type::Enum(_)),
        'c' => matches!(ty, Type::Scalar(Scalar::Tryte | Scalar::Trit) | Type::Lent(Elem::Tryte)),
        _ => matches!(ty, Type::Scalar(_) | Type::Text | Type::Enum(_)) || buffer,
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
            Type::Lent(e) => format!("a buffer of {}", e.scalar()),
            t => t.to_string(),
        };
        let d = err(format!("`{label}` prints {wants}, found {found}"), h.span);
        return Err(match spec {
            'c' if ty.is_int() => d.with_note("cast it: `(tryte) x`"),
            'd' | 'i' | 't' | 'v' if matches!(ty, Type::Enum(_)) => d.with_note("cast it to read the number: `(int) x`"),
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
proc sum(v: lent [int], k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    yield acc;
}

proc main() -> int {
    b: own [int] = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
    s = s << 1;
    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    yield 0;
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
        let HStmtKind::Assign { local: 2, index: None, value } = &body.stmts[0].kind else { panic!() };
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
        assert!(err_in("x := 1; r := 2.0; y := x + r;").contains("cannot mix int and real"));
        assert!(err_in("w: int = 5; t: tryte = w;").contains("narrow explicitly with `as tryte`"));
        ok("w: int = 5; t: tryte = (tryte) w; r := (real) w; back := (int) r;");
        assert!(err_in("r := 2.0 % 1.0;").contains("`%` works on integers"));
        assert!(err_in("r := 2.0 << 1;").contains("`<<` works on integers"));
        ok("r := 2.0 ** 3; t: tryte = 3; u := t * 2;");
    }

    #[test]
    fn logic_and_tri() {
        assert!(err_in("x := 1; y := x && x;").contains("`&&` works on trits, found int"));
        ok("a: trit = +; b: trit = 0; c := a && b || -;");
        assert!(err_in("x := 5; tri x { - => yield; 0 => yield; + => yield; }").contains("`tri` needs a trit"));
        ok("x := 5; tri x <=> 0 { - => yield; 0 => yield; + => yield; }");
    }

    #[test]
    fn names_and_scopes() {
        assert_eq!(err_in("y = 1;"), "unknown name `y`");
        // An inner `x` shadows the outer one only inside its block.
        let hir = ok("x := 1; t: trit = +; tri t { - => { x := 2; } 0 => yield; + => yield; } x = 3;");
        assert_eq!(hir.procs[0].locals.len(), 3);
        let HStmtKind::Assign { local, .. } = &hir.procs[0].body.stmts[3].kind else { panic!() };
        assert_eq!(*local, 0);
        assert!(err_in("walk i in 0..3 { i = 1; }").contains("read-only"));
        assert!(err_of("const L: int = 3; proc main() { L = 4; }").contains("cannot assign to const"));
        assert!(err_in("break;").contains("outside of a loop"));
    }

    #[test]
    fn registers() {
    }

    #[test]
    fn buffers() {
        assert!(err_in("b := claim 5;").contains("needs to know the buffer type"));
        assert!(err_in("b: own [int] = claim 5; c := b;").contains("cannot be used as a value"));
        assert!(err_in("x := 1; burn x;").contains("only an own buffer can be burned"));
        assert!(err_of("proc f(v: lent [int]) { burn v; } proc main() {}").contains("cannot burn a lent buffer"));
        assert!(err_of("proc f(v: lent [int]) { v = v; } proc main() {}").contains("cannot replace a lent buffer"));
        assert!(err_in("b: own [int] = claim 3; x := lend b;").contains("only allowed as a call argument"));
        assert!(err_in("x: lent [int] = 1;").contains("cannot hold a lent buffer"));
        assert!(err_in("s := \"hi\"; s[0] = 'A';").contains("text is read-only"));
        ok("s := \"hi\"; c := s[1]; n := len(s); b: own [tryte] = claim 3; b[0] = c; burn b;");
    }

    #[test]
    fn calls() {
        let defs = "proc take(b: own [int]) { burn b; } proc peek(v: lent [int]) -> int { yield v[0]; }";
        assert!(err_of(&format!("{defs} proc main() {{ b: own [int] = claim 3; take(b); }}")).contains("pass it with `give b`"));
        assert!(err_of(&format!("{defs} proc main() {{ b: own [int] = claim 3; x := peek(b); }}")).contains("pass it with `lend b`"));
        assert!(err_of(&format!("{defs} proc main() {{ x := peek(); }}")).contains("takes 1 argument, found 0"));
        assert!(err_of("proc mk() -> own [int] { b: own [int] = claim 1; yield b; } proc main() { mk(); }").contains("would be lost"));
        assert!(err_in("x := 1; x + 1;").contains("does nothing"));
        assert!(err_in("x := print(\"a\");").contains("has no value"));
        assert!(err_in("emit(1000);").contains("does not fit in a tryte"));
        ok("emit('A'); print(1.5); printl(\"hi\"); printl(); print(1 + 2); x := input(); dump(\"here\");");
    }

    #[test]
    fn printf_checks() {
        ok("n: int = 1; t: tryte = 'A'; printf(\"{n} {n:t} {n:v} {t:c} %d %i %f %r %s %c\", n + 1, n, 2.5, 0.5, \"x\", 'b');");
        assert!(err_in("printf(\"%d\", 1.5);").contains("prints an integer, found real"));
        assert!(err_in("x: int = 1; printf(\"%r\", x);").contains("prints a real, found int"));
        assert!(err_in("x: int = 70; printf(\"%c\", x);").contains("prints a tryte, found int"));
        assert!(err_in("printf(\"%d %d\", 1);").contains("has no argument"));
        assert!(err_in("printf(\"oi\", 1);").contains("has no `%`"));
        assert!(err_in("printf(\"{1+2}\");").contains("must hold a variable name"));
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
        assert!(e("enum A { X, Y } proc main() { a := A.Z; }").contains("has no value `Z`"));
        assert!(e("enum A { X, Y = 0 } proc main() {}").contains("both have the value 0"));
        assert!(e("enum A { X } enum A { Y } proc main() {}").contains("defined twice"));
        assert!(e("proc main() { c: Cor = 1; }").contains("unknown type `Cor`"));
        assert!(e("enum A { X } proc main() { a: A = 0; }").contains("expected A, found int"));
        assert!(e("enum A { X } enum B { Y } proc main() { t: trit = A.X == B.Y; }").contains("cannot compare A with B"));
        assert!(e("enum A { X } proc main() { printf(\"%d\", A.X); }").contains("cast it"));
        assert!(e("proc main() { x := 1; y := x.z; }").contains("`x` is not an enum"));
        assert!(e("proc main() { match 5 { 1 => print(1); } }").contains("needs an `else` arm"));
        assert!(e("proc main() { match 5 { else => print(1); 1 => print(2); } }").contains("must be the last"));
        assert!(e("proc main() { match 5 { 1 | 1 => print(1); else => {} } }").contains("already appears"));
        assert!(e("proc main() { x := 2; match 5 { x => print(1); else => {} } }").contains("must be a literal or an enum value"));
        assert!(check_src("enum A { X, Y } proc f(a: A) -> A { yield a; } proc main() { b := f(A.Y); print(b); }").is_ok());
    }

    #[test]
    fn if_checks() {
        ok("x := 1; if x > 0 { print(x); } else if x == 0 { } else { print(-x); }");
        assert!(err_in("x := 1; if x { }").contains("`if` needs a trit, found int"));
        assert!(err_in("x := 1; y := !x;").contains("`!` works on trits"));
        assert!(err_in("x := 1; while x { }").contains("`while` needs a trit, found int"));
        ok("i := 0; while i < 3 { i = i + 1; }");
        assert!(err_in("x := 1; t: trit = x == \"a\";").contains("found text"));
        assert!(err_in("x := 1.5; t: trit = x < 2;").contains("integer literal where a real is expected"));
    }

    #[test]
    fn print_buffers() {
        ok("b: own [int] = claim 3; t: own [tryte] = claim 1; printl(b); printf(\"{b} {b:t} %v {t:c} %c\", b, t); s := to_text(b, 'v'); burn b; burn t;");
        assert!(err_in("b: own [int] = claim 3; printf(\"{b:c}\"); burn b;").contains("found a buffer of int"));
        assert!(err_in("b: own [int] = claim 3; printf(\"%f\", b); burn b;").contains("prints a real"));
    }

    #[test]
    fn final_locals() {
        ok("final x: int = input(); final y := x + 1; print(y);");
        ok("final b: own [int] = claim 3; b[0] = 5; burn b;");
        ok("final x := 1; tri x <=> 0 { - => {} 0 => {} + => { x := 2; print(x); } }");
        assert!(err_in("final x := 1; x = 2;").contains("`x` is final and cannot change"));
        assert!(err_in("final x := 1; x += 2;").contains("is final"));
        assert!(err_in("final x := 1; x := 2;").contains("cannot be declared again"));
    }

    #[test]
    fn to_text_checks() {
        ok("n: int = 5; a := to_text(n); b: text = to_text(n, 't'); c := to_text(1.5); print(\"x\" + to_text(n, 'v'));");
        assert!(err_in("x := to_text(1.5, 't');").contains("takes an integer, found real"));
        assert!(err_in("x := to_text(\"a\");").contains("takes a number, found text"));
        assert!(err_in("x := to_text(5, 'x');").contains("must be 't' (ternary) or 'v'"));
        assert!(err_in("x := to_text();").contains("1 or 2 arguments"));
    }

    #[test]
    fn text_concat() {
        ok("name: text = \"Ana\"; print(\"hi \" + name + \"!\\n\");");
        assert!(err_in("x := 5; print(\"x = \" + x);").contains("joins text with text only, found int"));
        assert!(err_in("t := \"a\" + \"b\";").contains("only inside `print`"));
        assert!(err_in("print(\"a\" + 1.5);").contains("found real"));
    }

    #[test]
    fn structure() {
        assert!(err_of("proc f() {}").contains("no `proc main`"));
        assert!(err_of("proc main(x: int) {}").contains("cannot take parameters"));
        assert!(err_of("proc f() {} proc f() {} proc main() {}").contains("defined twice"));
        assert!(err_of("proc print() {} proc main() {}").contains("built-in"));
        assert!(err_of("proc f() -> int { x := 1; } proc main() {}").contains("must end with `yield`"));
        assert!(err_in("yield; x := 1;").contains("unreachable code"));
        assert!(err_in("x := input() on NaT { print(\"bad\"); };").contains("must end with `yield`"));
        assert!(err_of("proc f() -> lent [int] { } proc main() {}").contains("cannot yield a lent buffer"));
        ok("x := input() on NaT { yield; }; print(x);");
    }

    #[test]
    fn consts() {
        let hir = check_src("const LIMIT: tryte = -100; const NAME: text = \"Kestrel\"; proc main() { x := LIMIT; print(NAME); }").unwrap();
        assert_eq!(hir.procs[0].locals[0].ty, Type::Scalar(Scalar::Tryte));
        assert!(err_of("const X: int = 1 + 2; proc main() {}").contains("must be a literal"));
    }
}
