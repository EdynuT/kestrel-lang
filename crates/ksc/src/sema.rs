//! Name resolution and type checking: AST -> [`Hir`] (SPEC §1-§8).
//!
//! Ownership rules (use after give, leaks, double burn, tri symmetry) are
//! checked afterwards by the linear checker on the HIR.

use std::collections::HashMap;

use kestrel_core::word::{TRIT_MAX, TRYTE_MAX};

use crate::ast::{self, Expr, ExprKind, InstrOp, Item, Place, Program, Range, StmtKind, UnOp};
use crate::diag::{Diagnostic, Span};
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

const BUILTINS: [&str; 6] = ["print", "show", "emit", "input", "len", "dump"];

pub fn check(program: &Program) -> R<Hir> {
    let mut cx = Checker { sigs: Vec::new(), proc_ids: HashMap::new(), consts: HashMap::new(), f: FnCx::new(Type::Void) };
    for item in &program.items {
        match item {
            Item::Const(c) => cx.declare_const(c)?,
            Item::Proc(p) => cx.declare_proc(p)?,
        }
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
    regs: HashMap<i8, Span>,
}

impl FnCx {
    fn new(ret: Type) -> FnCx {
        FnCx { locals: Vec::new(), scopes: vec![Vec::new()], ret, loops: 0, regs: HashMap::new() }
    }
}

struct Checker {
    sigs: Vec<Sig>,
    proc_ids: HashMap<String, ProcId>,
    consts: HashMap<String, HExpr>,
    f: FnCx,
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
            params.push((param.name.name.clone(), param.ty.ty));
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
            self.new_local(&param.name.name, *ty, None, LocalKind::Param, param.name.span);
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

    fn new_local(&mut self, name: &str, ty: Type, reg: Option<i8>, kind: LocalKind, span: Span) -> LocalId {
        let id = self.f.locals.len();
        self.f.locals.push(Local { name: name.to_string(), ty, reg, kind, span });
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
            StmtKind::Let { name, ty, reg, value, on_nat } => {
                let (value, lty) = match ty {
                    Some(t) => {
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
                if let Some((r, rspan)) = reg {
                    if !lty.is_numeric() {
                        return Err(err("only numbers can be pinned to a register", *rspan));
                    }
                    if self.f.regs.insert(*r, *rspan).is_some() {
                        return Err(err(
                            format!("register @{} is already pinned to another variable", crate::lexer::reg_name(*r)),
                            *rspan,
                        ));
                    }
                }
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
                let local = self.new_local(&name.name, lty, reg.map(|(r, _)| r), LocalKind::Let, name.span);
                HStmtKind::Let { local, value, on_nat }
            }
            StmtKind::Assign { place, value } => self.assign(place, value)?,
            StmtKind::Instr { op, dest, args, .. } => {
                let place_expr = match &dest.index {
                    None => Expr { kind: ExprKind::Var(dest.name.name.clone()), span: dest.span },
                    Some(i) => Expr {
                        kind: ExprKind::Index(
                            Box::new(Expr { kind: ExprKind::Var(dest.name.name.clone()), span: dest.name.span }),
                            Box::new(i.clone()),
                        ),
                        span: dest.span,
                    },
                };
                let value = match (op, args.as_slice()) {
                    (InstrOp::Mov, [src]) => src.clone(),
                    (InstrOp::Neg, []) => Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(place_expr)), span: s.span },
                    (op, args) => {
                        let bin = op.bin_op().expect("arithmetic instruction");
                        let (l, r) = match args {
                            [src] => (place_expr, src.clone()),
                            [a, b] => (a.clone(), b.clone()),
                            _ => unreachable!("operand count checked by the parser"),
                        };
                        Expr { kind: ExprKind::Binary(bin, Box::new(l), Box::new(r)), span: s.span }
                    }
                };
                self.assign(dest, &value)?
            }
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
                    return Err(err(format!("`{word}` outside of a `walk`"), s.span));
                }
                if s.kind == StmtKind::Break { HStmtKind::Break } else { HStmtKind::Continue }
            }
            StmtKind::Tri { head, neg, zero, pos } => {
                let h = self.expr(head, Some(Type::Scalar(Scalar::Trit)))?;
                if h.ty != Type::Scalar(Scalar::Trit) {
                    return Err(err(format!("`tri` needs a trit, found {}", h.ty), head.span)
                        .with_note("compare first, e.g. `tri x <=> 0 { ... }`"));
                }
                let arms = [self.block(neg)?, self.block(zero)?, self.block(pos)?];
                HStmtKind::Tri { head: h, arms: Box::new(arms) }
            }
            StmtKind::Walk { var, range, body } => {
                let word = Type::Scalar(Scalar::Int);
                let (span_tmp, start, end) = match range {
                    Range::Between(a, b) => (None, self.expect(a, word)?, self.expect(b, word)?),
                    Range::Span(k) => {
                        let hk = self.expect(k, word)?;
                        let tmp = self.new_local("$span", word, None, LocalKind::Hidden, k.span);
                        let local = hx(HExprKind::Local(tmp), word, k.span);
                        let start = hx(HExprKind::Neg(Box::new(local.clone())), word, k.span);
                        (Some((tmp, hk)), start, local)
                    }
                };
                let end_tmp = self.new_local("$end", word, None, LocalKind::Hidden, var.span);
                self.f.scopes.push(Vec::new());
                let v = self.new_local(&var.name, word, None, LocalKind::WalkVar, var.span);
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
                if !matches!(h.kind, HExprKind::Call(..) | HExprKind::Builtin(..)) {
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
                    .with_note("convert one side with `as real` or `as int`"));
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
            ExprKind::Unary(_, inner) => {
                let h = self.expr(inner, want.filter(|t| t.is_numeric()))?;
                self.numeric(&h)?;
                let ty = h.ty;
                match h.kind {
                    HExprKind::Word(w) => hx(HExprKind::Word(-w), ty, span),
                    HExprKind::Real(r) => hx(HExprKind::Real(-r), ty, span),
                    _ => hx(HExprKind::Neg(Box::new(h)), ty, span),
                }
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, span)?,
            ExprKind::Cast(inner, s) => {
                let h = self.expr(inner, None)?;
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
            BinOp::Cmp3 => {
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
                let (ha, hb, ty) = self.numeric_pair(a, b, want)?;
                if op == BinOp::Mod && ty == Type::Scalar(Scalar::Real) {
                    return Err(err("`%` works on integers, not reals", span));
                }
                (ha, hb, ty)
            }
        };
        Ok(hx(HExprKind::Binary(op, Box::new(ha), Box::new(hb)), ty, span))
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
            "print" => {
                arity(1)?;
                let a = self.expect(&args[0], text)?;
                return Ok(builtin(Builtin::Print, vec![a], Type::Void));
            }
            "show" => {
                arity(1)?;
                let a = self.expr(&args[0], None)?;
                if !matches!(a.ty, Type::Scalar(_) | Type::Text) {
                    return Err(err(format!("`show` prints numbers and text, found {}", a.ty), a.span));
                }
                return Ok(builtin(Builtin::Show, vec![a], Type::Void));
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
    acc: int @R0 = 0;
    walk i in span k {
        add acc, v[i];
    }
    yield acc;
}

proc main() -> int {
    b: own [int] = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int @R+1 = sum(lend b, 2) on NaT { burn b; yield -1; };
    shl3 s, 1;
    tri cmp3 s, 10 {
        - => print("less"),
        0 => print("equal"),
        + => print("greater"),
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
        assert_eq!(sum.locals[2].reg, Some(0));
        // `add acc, v[i]` became `acc = acc + v[i]`.
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
        assert!(err_in("x := 5; tri x { - => yield, 0 => yield, + => yield, }").contains("`tri` needs a trit"));
        ok("x := 5; tri x <=> 0 { - => yield, 0 => yield, + => yield, }");
    }

    #[test]
    fn names_and_scopes() {
        assert_eq!(err_in("y = 1;"), "unknown name `y`");
        // An inner `x` shadows the outer one only inside its block.
        let hir = ok("x := 1; t: trit = +; tri t { - => { x := 2; } 0 => yield, + => yield, } x = 3;");
        assert_eq!(hir.procs[0].locals.len(), 3);
        let HStmtKind::Assign { local, .. } = &hir.procs[0].body.stmts[3].kind else { panic!() };
        assert_eq!(*local, 0);
        assert!(err_in("walk i in 0..3 { i = 1; }").contains("read-only"));
        assert!(err_of("const L: int = 3; proc main() { L = 4; }").contains("cannot assign to const"));
        assert!(err_in("break;").contains("outside of a `walk`"));
    }

    #[test]
    fn registers() {
        assert!(err_in("a: int @R0 = 1; b: int @R0 = 2;").contains("@R0 is already pinned"));
        assert!(err_in("b: own [int] @R+1 = claim 3; burn b;").contains("only numbers"));
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
        ok("emit('A'); show(1.5); show(\"hi\"); x := input(); dump(\"here\");");
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
        ok("x := input() on NaT { yield; }; show(x);");
    }

    #[test]
    fn consts() {
        let hir = check_src("const LIMIT: tryte = -100; const NAME: text = \"Kestrel\"; proc main() { x := LIMIT; print(NAME); }").unwrap();
        assert_eq!(hir.procs[0].locals[0].ty, Type::Scalar(Scalar::Tryte));
        assert!(err_of("const X: int = 1 + 2; proc main() {}").contains("must be a literal"));
    }
}
