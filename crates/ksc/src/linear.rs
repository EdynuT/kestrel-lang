//! Ownership checker (SPEC §6): every `own` buffer is burned or given away
//! exactly once on every path.
//!
//! The checker walks each proc tracking every own local as alive, burned or
//! given. Leaving a block or returning with a buffer still alive is a leak;
//! touching a burned or given buffer is a use-after-free; the arms of a `tri`
//! that fall through must agree; a loop body (`walk`, `while`) must leave outer buffers as it
//! found them.

use std::collections::HashMap;

use crate::diag::{Diagnostic, Span};
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    /// Not an own buffer, or not declared yet on this path.
    Untracked,
    Alive,
    Burned,
    Given,
    /// Declared without a value (`c: Conta;`): owns nothing yet.
    Empty,
}

impl St {
    /// Burned and given are both "consumed": arms may differ in how.
    fn agrees(self, other: St) -> bool {
        self == other || (self.consumed() && other.consumed())
    }

    fn consumed(self) -> bool {
        matches!(self, St::Burned | St::Given | St::Empty)
    }

    fn describe(self) -> &'static str {
        match self {
            St::Untracked => "not declared",
            St::Alive => "still owned",
            St::Burned => "burned",
            St::Given => "given away",
            St::Empty => "without a value",
        }
    }
}

type State = Vec<St>;

struct LoopCx {
    entry: State,
    /// Own locals declared inside this walk's body.
    inner: Vec<LocalId>,
}

thread_local! {
    /// [`Hir::borrows`] of the program being checked: which argument the
    /// result of a `-> &T` call borrows from.
    static BORROWS: std::cell::RefCell<Vec<Option<usize>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The argument of a call whose borrow its result is (`contas.get(0)`
/// borrows from `contas`), if the proc returns `-> &T`.
fn borrowed_arg(e: &HExpr) -> Option<&HExpr> {
    let (pid, args) = match &e.kind {
        HExprKind::Call(pid, args) => (*pid, args),
        _ => return None,
    };
    let k = BORROWS.with(|b| b.borrow().get(pid).copied().flatten())?;
    args.get(k)
}

pub fn check(hir: &Hir) -> R<()> {
    BORROWS.with(|b| *b.borrow_mut() = hir.borrows.clone());
    // Procs of other files were checked there; the linker writes dispatches.
    hir.procs.iter().filter(|p| matches!(p.link, Link::Own | Link::Shared)).try_for_each(check_proc)
}

fn check_proc(p: &HProc) -> R<()> {
    let mut st = vec![St::Untracked; p.locals.len()];
    for (i, s) in st.iter_mut().enumerate().take(p.params) {
        if is_own(p.locals[i].ty) {
            *s = St::Alive;
        }
    }
    let mut ck = Checker { p, loops: Vec::new(), alias: HashMap::new(), borrowers: Vec::new() };
    if let Some(end) = ck.block(&p.body, st)? {
        // A proc without `-> T` may fall off its end: that is an implicit `return;`.
        let end_span = Span::new(p.body.span.end - 1, p.body.span.end);
        ck.no_alive(&end, end_span, "the proc ends")?;
    }
    Ok(())
}

fn is_own(t: Type) -> bool {
    matches!(t, Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_))
}

struct Checker<'h> {
    p: &'h HProc,
    loops: Vec<LoopCx>,
    /// What each `lent` variable (and hidden `?.` variable) borrows from.
    alias: HashMap<LocalId, LocalId>,
    /// The `lent` variables in scope: while one borrows from `x`, `x` cannot
    /// be burned, given away, emptied of an element or lent to a call.
    borrowers: Vec<LocalId>,
}

/// Whether `s` mentions local `id` anywhere inside.
fn stmt_uses(s: &HStmt, id: LocalId) -> bool {
    let e = |x: &HExpr| expr_uses(x, id);
    let b = |x: &HBlock| x.stmts.iter().any(|s| stmt_uses(s, id));
    match &s.kind {
        HStmtKind::Let { local, value, on_nat } => *local == id || e(value) || on_nat.as_ref().is_some_and(b),
        HStmtKind::Declare(l) | HStmtKind::Burn(l) => *l == id,
        HStmtKind::Assign { local, index, value, .. } => *local == id || index.as_ref().is_some_and(e) || e(value),
        HStmtKind::SetElem { arr, index, value, .. } => e(arr) || e(index) || e(value),
        HStmtKind::Return(v) => v.as_ref().is_some_and(e),
        HStmtKind::Break | HStmtKind::Continue | HStmtKind::Trap(_) | HStmtKind::Dump(_) => false,
        HStmtKind::Tri { head, arms, .. } => e(head) || arms.iter().any(b),
        HStmtKind::Block(x) => b(x),
        HStmtKind::While { cond, body, step } => e(cond) || b(body) || step.as_ref().is_some_and(b),
        HStmtKind::Exit(x) | HStmtKind::Expr(x) => e(x),
        HStmtKind::Walk { var, span_tmp, start, end, body, .. } => {
            *var == id || span_tmp.as_ref().is_some_and(|(_, k)| e(k)) || e(start) || e(end) || b(body)
        }
        HStmtKind::Throw { value, .. } => e(value),
        HStmtKind::Try { body, catches } => b(body) || catches.iter().any(|c| c.local == id || b(&c.body)),
    }
}

fn expr_uses(x: &HExpr, id: LocalId) -> bool {
    let e = |y: &HExpr| expr_uses(y, id);
    match &x.kind {
        HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_) => false,
        HExprKind::Local(l) | HExprKind::Give(l) | HExprKind::Lend(l) | HExprKind::Field(l, _) => *l == id,
        HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) | HExprKind::ToIface(a, _) | HExprKind::TakeField(a, _) => e(a),
        HExprKind::IsClass(a, ..) => e(a),
        HExprKind::Binary(_, a, c) | HExprKind::Index(a, c) | HExprKind::ElemField(a, c, _) => e(a) || e(c),
        HExprKind::Bind(l, a, c) => *l == id || e(a) || e(c),
        HExprKind::Call(_, xs) | HExprKind::Builtin(_, xs) | HExprKind::Pack(xs) | HExprKind::StructLit(xs) | HExprKind::NewObj(xs) => {
            xs.iter().any(e)
        }
        HExprKind::Print(ps) => ps.iter().any(|(_, y)| e(y)),
        HExprKind::IfNull { value, then, null, .. } => e(value) || e(then) || null.as_deref().is_some_and(e),
    }
}

/// A borrowed type (`&Conta`, `&No?`, `&v[]`).
fn is_lent_ty(t: Type) -> bool {
    matches!(t, Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_))
}

/// A `lent` variable (not a parameter).
fn is_lent_var(l: &Local) -> bool {
    l.kind == LocalKind::Let
        && matches!(l.ty.unopt(), Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_))
}

/// The local an array or object expression reaches (`a`, `a.b[i].c`), lent or not.
fn base_local(e: &HExpr) -> Option<LocalId> {
    let mut e = e;
    loop {
        match &e.kind {
            HExprKind::Lend(id) | HExprKind::Local(id) | HExprKind::Give(id) => return Some(*id),
            HExprKind::ElemField(arr, _, _) | HExprKind::ToIface(arr, _) | HExprKind::TakeField(arr, _) => e = arr,
            _ => e = borrowed_arg(e)?,
        }
    }
}

impl Checker<'_> {
    fn name(&self, id: LocalId) -> &str {
        &self.p.locals[id].name
    }

    /// The owner `id` borrows from in the end (itself if it borrows nothing).
    fn root(&self, id: LocalId) -> LocalId {
        let mut r = id;
        for _ in 0..self.p.locals.len() {
            match self.alias.get(&r) {
                Some(&n) if n != r => r = n,
                _ => break,
            }
        }
        r
    }

    /// `what` would happen to something reached through `direct`: an error if
    /// a `lent` variable (other than `direct` itself) borrows from the same owner.
    fn guard(&self, direct: LocalId, what: &str, span: Span) -> R<()> {
        let via = self.borrowers.contains(&direct).then_some(direct);
        let r = self.root(direct);
        if let Some(&b) = self.borrowers.iter().find(|&&b| Some(b) != via && b != r && self.root(b) == r) {
            let (owner, by) = (self.name(r), self.name(b));
            return Err(Diagnostic::new(format!("`{owner}` cannot be {what} while `{by}` borrows from it"), span)
                .with_note(format!("`{by}` could be left pointing at nothing; do this after the last use of `{by}`")));
        }
        Ok(())
    }

    /// `local` (a `lent` variable) now borrows from what `value` reaches.
    fn borrow(&mut self, local: LocalId, value: &HExpr) {
        match base_local(value) {
            Some(from) => {
                let r = self.root(from);
                self.alias.insert(local, r);
            }
            None => {
                self.alias.remove(&local);
            }
        }
        if !self.borrowers.contains(&local) {
            self.borrowers.push(local);
        }
    }

    fn block(&mut self, b: &HBlock, st: State) -> R<Option<State>> {
        let r = self.block_here(b, st);
        // `lent` variables end with their block.
        self.borrowers.retain(|id| !b.scope.contains(id));
        r
    }

    fn block_here(&mut self, b: &HBlock, mut st: State) -> R<Option<State>> {
        // A `lent` variable of this block borrows until the last statement
        // that uses it, not until the `}` (so the owner can be burned after).
        let last_use: Vec<(LocalId, usize)> = b
            .scope
            .iter()
            .filter(|id| is_lent_var(&self.p.locals[**id]))
            .map(|&id| (id, b.stmts.iter().rposition(|s| stmt_uses(s, id)).unwrap_or(0)))
            .collect();
        for (k, s) in b.stmts.iter().enumerate() {
            match self.stmt(s, st)? {
                Some(next) => st = next,
                None => return Ok(None),
            }
            self.borrowers.retain(|id| !last_use.contains(&(*id, k)));
        }
        for &id in &b.scope {
            if st[id] == St::Alive {
                let l = &self.p.locals[id];
                return Err(Diagnostic::new(format!("`{}` is never burned or given away", l.name), l.span)
                    .with_note(format!("burn it (`burn {};`) or pass it on (`give {}`) before its block ends", l.name, l.name)));
            }
            st[id] = St::Untracked;
        }
        Ok(Some(st))
    }

    /// Every buffer still alive at a return (or at the end of a proc) leaks.
    fn no_alive(&self, st: &State, span: Span, when: &str) -> R<()> {
        if let Some(id) = st.iter().position(|s| *s == St::Alive) {
            let name = self.name(id);
            return Err(Diagnostic::new(format!("`{name}` still owns a buffer when {when}"), span)
                .with_note(format!("burn it (`burn {name};`) or give it away first")));
        }
        Ok(())
    }

    fn require_alive(&self, id: LocalId, st: &State, span: Span) -> R<()> {
        match st[id] {
            St::Alive | St::Untracked => Ok(()),
            St::Burned => Err(Diagnostic::new(format!("`{}` is used after it was burned", self.name(id)), span)),
            St::Given => Err(Diagnostic::new(format!("`{}` is used after it was given away", self.name(id)), span)),
            St::Empty => Err(Diagnostic::new(format!("`{}` has no value yet", self.name(id)), span)
                .with_note(format!("give it one first: `{} = ...;`", self.name(id)))),
        }
    }

    fn stmt(&mut self, s: &HStmt, mut st: State) -> R<Option<State>> {
        match &s.kind {
            HStmtKind::Let { local, value, on_nat } => {
                self.expr(value, &mut st)?;
                if is_lent_var(&self.p.locals[*local]) {
                    self.borrow(*local, value);
                }
                if let Some(b) = on_nat {
                    self.block(b, st.clone())?; // always diverges (checked by sema)
                }
                if is_own(self.p.locals[*local].ty) {
                    st[*local] = St::Alive;
                    if let Some(l) = self.loops.last_mut() {
                        l.inner.push(*local);
                    }
                }
            }
            HStmtKind::Assign { local, index, value, .. } => {
                if let Some(i) = index {
                    self.expr(i, &mut st)?;
                }
                self.expr(value, &mut st)?;
                if index.is_none() && is_lent_var(&self.p.locals[*local]) {
                    self.borrow(*local, value);
                }
                if index.is_some() {
                    // Checked after the operands so `b[0] = f(give b)` is caught.
                    self.require_alive(*local, &st, s.span)?;
                } else if is_own(self.p.locals[*local].ty) && self.p.locals[*local].ty.is_opt() {
                    // A `T?` may be assigned in any state: sema checks it is empty when it runs.
                    st[*local] = St::Alive;
                } else if is_own(self.p.locals[*local].ty) {
                    let name = self.name(*local);
                    match st[*local] {
                        St::Alive => {
                            return Err(Diagnostic::new(format!("`{name}` still owns a buffer; assigning to it would leak it"), s.span)
                                .with_note(format!("burn it first: `burn {name};`")));
                        }
                        St::Given => {
                            return Err(Diagnostic::new(format!("`{name}` was given away; its name can only be reused after `burn`"), s.span));
                        }
                        St::Burned | St::Untracked | St::Empty => st[*local] = St::Alive,
                    }
                }
            }
            HStmtKind::Declare(local) if is_lent_var(&self.p.locals[*local]) => {
                self.borrowers.push(*local);
            }
            HStmtKind::Declare(local) => {
                if is_own(self.p.locals[*local].ty) {
                    // `x: No?;` holds a valid (empty) value; `x: No;` has none yet.
                    st[*local] = if self.p.locals[*local].ty.is_opt() { St::Alive } else { St::Empty };
                    if let Some(l) = self.loops.last_mut() {
                        l.inner.push(*local);
                    }
                }
            }
            HStmtKind::SetElem { arr, index, value, .. } => {
                self.expr(index, &mut st)?;
                self.expr(value, &mut st)?;
                match arr.kind {
                    HExprKind::Local(id) => self.require_alive(id, &st, s.span)?,
                    _ => self.expr(arr, &mut st)?,
                }
            }
            HStmtKind::Burn(id) => {
                self.guard(*id, "burned", s.span)?;
                let name = self.name(*id);
                match st[*id] {
                    St::Alive => st[*id] = St::Burned,
                    St::Burned => {
                        return Err(Diagnostic::new(format!("`{name}` is burned twice"), s.span)
                            .with_note("a buffer can be burned only once"));
                    }
                    St::Empty => {
                        return Err(Diagnostic::new(format!("`{name}` has no value to burn"), s.span)
                            .with_note(format!("it was declared without one; give it a value first: `{name} = ...;`")));
                    }
                    St::Given | St::Untracked => {
                        return Err(Diagnostic::new(format!("`{name}` was given away and cannot be burned here"), s.span)
                            .with_note("whoever received it is now responsible for burning it"));
                    }
                }
            }
            HStmtKind::Return(v) => {
                if let Some(e) = v {
                    self.expr(e, &mut st)?;
                    // `-> &T`: the result must come from the first `&` parameter.
                    let ps = &self.p.locals[..self.p.params];
                    if let Some(k) = ps.iter().position(|l| is_lent_ty(l.ty))
                        && is_lent_ty(self.p.ret)
                    {
                        let from = lent_root(e).map(|d| self.root(d));
                        // `return null;` (from `-> &T?`) lends nothing.
                        if from != Some(k) && !is_null(e) {
                            let src = &ps[k].name;
                            return Err(Diagnostic::new(format!("the result must be borrowed from `{src}`"), e.span).with_note(
                                format!("a `-> &T` proc lends part of its first `&` parameter (`{src}`), so the caller knows what stays borrowed"),
                            ));
                        }
                    }
                }
                self.no_alive(&st, s.span, "the proc returns")?;
                return Ok(None);
            }
            // A trap zeroes the heap, so buffers still alive here are fine.
            HStmtKind::Trap(_) => return Ok(None),
            // The program ends: arrays still alive are freed with it.
            HStmtKind::Exit(code) => {
                self.expr(code, &mut st)?;
                return Ok(None);
            }
            HStmtKind::Break | HStmtKind::Continue => {
                let word = if s.kind == HStmtKind::Break { "break" } else { "continue" };
                let l = self.loops.last().expect("sema rejects break outside walk");
                if let Some(&id) = l.inner.iter().find(|id| st[**id] == St::Alive) {
                    return Err(Diagnostic::new(format!("`{}` still owns a buffer at this `{word}`", self.name(id)), s.span)
                        .with_note("burn it before leaving the iteration"));
                }
                self.outer_unchanged(&l.entry, &st, &l.inner, s.span)?;
                return Ok(None);
            }
            HStmtKind::Block(b) => return self.block(b, st),
            HStmtKind::Tri { head, arms, origin } => {
                self.expr(head, &mut st)?;
                let mut ends: Vec<(&str, State)> = Vec::new();
                let labels = match origin {
                    Branch::Tri => ["`-` arm", "`0` arm", "`+` arm"],
                    Branch::If => ["`else` branch", "", "`if` branch"],
                    Branch::Match => ["a later arm", "", "this arm"],
                };
                for (label, arm) in labels.into_iter().zip(arms.iter()) {
                    if let Some(end) = self.block(arm, st.clone())? {
                        ends.push((label, end));
                    }
                }
                let what = match origin {
                    Branch::Tri => "the arms of this `tri`",
                    Branch::If => "the branches of this `if`",
                    Branch::Match => "the arms of this `match`",
                };
                match self.merge(&ends, what, s.span)? {
                    Some(m) => st = m,
                    None => return Ok(None), // every arm diverges
                }
            }
            HStmtKind::Throw { value, .. } => {
                // What is still owned here is freed when the error leaves the block.
                self.expr(value, &mut st)?;
                return Ok(None);
            }
            HStmtKind::Try { body, catches } => {
                let entry = st.clone();
                let mut ends: Vec<(&str, State)> = Vec::new();
                if let Some(end) = self.block(body, st)? {
                    ends.push(("the `try` block", end));
                }
                // An error may leave the body anywhere: what the body changed
                // is freed then, so each catch starts with it gone (a `T?`
                // with it empty, which is a value).
                let mut changed = Vec::new();
                changed_in(body, &mut changed);
                let mut start = entry.clone();
                for id in changed {
                    start[id] = match entry[id] {
                        St::Untracked => St::Untracked,
                        _ if self.p.locals[id].ty.is_opt() => St::Alive,
                        St::Empty => St::Empty,
                        _ => St::Burned,
                    };
                }
                for c in catches {
                    let mut at = start.clone();
                    at[c.local] = St::Alive;
                    if let Some(end) = self.block(&c.body, at)? {
                        ends.push(("a `catch`", end));
                    }
                }
                match self.merge(&ends, "the `try` and its `catch` blocks", s.span)? {
                    Some(m) => st = m,
                    None => return Ok(None),
                }
            }
            HStmtKind::While { cond, body, step } => {
                // The condition runs before every turn: it cannot consume anything.
                let before = st.clone();
                self.expr(cond, &mut st)?;
                if let Some(id) = (0..st.len()).find(|&i| st[i] != before[i]) {
                    return Err(Diagnostic::new(format!("the loop condition gives away `{}`", self.name(id)), cond.span)
                        .with_note("the condition runs before every turn, so it would give it away again"));
                }
                self.loops.push(LoopCx { entry: st.clone(), inner: Vec::new() });
                let after = match (self.block(body, st.clone()), step) {
                    (Ok(Some(after)), Some(step)) => self.block(step, after),
                    (r, _) => r,
                };
                let l = self.loops.pop().expect("pushed above");
                if let Some(after) = after? {
                    self.outer_unchanged(&l.entry, &after, &l.inner, s.span)?;
                }
            }
            HStmtKind::Walk { span_tmp, start, end, body, .. } => {
                if let Some((_, k)) = span_tmp {
                    self.expr(k, &mut st)?;
                }
                self.expr(start, &mut st)?;
                self.expr(end, &mut st)?;
                self.loops.push(LoopCx { entry: st.clone(), inner: Vec::new() });
                let after = self.block(body, st.clone());
                let l = self.loops.pop().expect("pushed above");
                if let Some(after) = after? {
                    self.outer_unchanged(&l.entry, &after, &l.inner, s.span)?;
                }
                // The body may run zero times, so the state is the entry state.
            }
            HStmtKind::Expr(e) => self.expr(e, &mut st)?,
            HStmtKind::Dump(_) => {}
        }
        Ok(Some(st))
    }

    /// The state after paths that join (arms of a `tri`, a `try` and its
    /// catches): they must agree on every buffer. `None` if none continues.
    fn merge(&self, ends: &[(&str, State)], what: &str, span: Span) -> R<Option<State>> {
        let Some((_, mut first)) = ends.first().cloned() else {
            return Ok(None);
        };
        for (_, other) in &ends[1..] {
            if let Some(id) = (0..first.len()).find(|&i| !first[i].agrees(other[i])) {
                let detail: Vec<String> = ends.iter().map(|(label, s)| format!("{label}: {}", s[id].describe())).collect();
                return Err(Diagnostic::new(format!("{what} leave `{}` in different states", self.name(id)), span)
                    .with_note(format!("{} (every path that continues must agree)", detail.join(", "))));
            }
            // If any path gave the buffer away, its name cannot be reused afterwards.
            for (f, o) in first.iter_mut().zip(other) {
                if *o == St::Given {
                    *f = St::Given;
                }
            }
        }
        Ok(Some(first))
    }

    /// A walk body may not consume (or create) buffers that live outside it.
    fn outer_unchanged(&self, entry: &State, now: &State, inner: &[LocalId], span: Span) -> R<()> {
        for id in 0..entry.len() {
            if !inner.contains(&id) && entry[id] != now[id] && is_own(self.p.locals[id].ty) {
                return Err(Diagnostic::new(
                    format!("a loop body cannot change `{}`, which lives outside the loop", self.name(id)),
                    span,
                )
                .with_note(format!("it was {} before the loop and is {} here", entry[id].describe(), now[id].describe())));
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &HExpr, st: &mut State) -> R<()> {
        match &e.kind {
            HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_) | HExprKind::Local(_) => {}
            HExprKind::Give(id) => {
                self.guard(*id, "given away", e.span)?;
                self.require_alive(*id, st, e.span)?;
                st[*id] = St::Given;
            }
            HExprKind::Lend(id) => self.require_alive(*id, st, e.span)?,
            HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) => self.expr(a, st)?,
            HExprKind::Pack(items) => items.iter().try_for_each(|a| self.expr(a, st))?,
            HExprKind::Binary(op, a, b) => {
                self.expr(a, st)?;
                // `a && b` / `a || b` may skip `b`: a `give` there might never happen.
                if matches!(op, BinOp::And | BinOp::Or)
                    && let Some(at) = (0..st.len()).find_map(|id| gives(b, id))
                {
                    return Err(Diagnostic::new(format!("`give` on the right of `{}` may never run", op.symbol()), at)
                        .with_note("the right side is skipped when the left side decides; give the array away before the test"));
                }
                self.expr(b, st)?;
            }
            HExprKind::Index(base, idx) => {
                self.expr(idx, st)?;
                match base.kind {
                    HExprKind::Local(id) => self.require_alive(id, st, base.span)?,
                    _ => self.expr(base, st)?,
                }
            }
            HExprKind::ToIface(inner, _) => self.expr(inner, st)?,
            HExprKind::Call(_, args) => {
                // What each lent argument borrows from: a local, maybe through
                // fields (`&c` and `&c.items` both borrow from `c`).
                let lent: Vec<Option<LocalId>> = args.iter().map(lent_root).collect();
                for (a, l) in args.iter().zip(&lent) {
                    if let Some(d) = l {
                        self.guard(*d, "borrowed by a call", a.span)?;
                    }
                }
                for (i, a) in args.iter().enumerate() {
                    // Also nested: `f(&v, g(give v))`.
                    if let Some((id, at)) = lent.iter().flatten().find_map(|&id| gives(a, id).map(|s| (id, s))) {
                        return Err(Diagnostic::new(format!("`{}` is borrowed and given away in the same call", self.name(id)), at)
                            .with_note("the callee would borrow something it also owns"));
                    }
                    // `f(&cs[0], remove(cs, 0))`: the same object lent and owned.
                    if let Some((id, at)) = lent.iter().flatten().find_map(|&id| takes_out(a, id).map(|s| (id, s))) {
                        return Err(Diagnostic::new(format!("`{}` is borrowed and has an element taken out in the same call", self.name(id)), at)
                            .with_note("the callee could borrow the very element it gets: take it out before the call"));
                    }
                    if let Some(id) = lent[i]
                        && lent[..i].contains(&Some(id))
                    {
                        return Err(Diagnostic::new(format!("`{}` is borrowed twice in the same call", self.name(id)), a.span)
                            .with_note("the proc would see the same data under two names: a write through one would change the other"));
                    }
                    self.expr(a, st)?;
                }
            }
            HExprKind::Builtin(Builtin::Pop | Builtin::Remove, args) if base_local(&args[0]).is_some_and(|d| self.guard(d, "x", e.span).is_err()) => {
                return self.guard(base_local(&args[0]).expect("checked"), "emptied of an element", e.span);
            }
            HExprKind::Builtin(_, args) => {
                // `append(v, f(give v))`: the array would be gone before the append.
                for a in args {
                    if let HExprKind::Lend(id) = a.kind
                        && let Some(g) = args.iter().find_map(|b| gives(b, id))
                    {
                        return Err(Diagnostic::new(format!("`{}` is given away inside a call that uses it", self.name(id)), g)
                            .with_note("the array would be gone before the call uses it"));
                    }
                }
                args.iter().try_for_each(|a| self.expr(a, st))?
            }
            HExprKind::Print(pieces) => pieces.iter().try_for_each(|(_, a)| self.expr(a, st))?,
            HExprKind::StructLit(items) | HExprKind::NewObj(items) => items.iter().try_for_each(|a| self.expr(a, st))?,
            HExprKind::Field(..) => {}
            HExprKind::ElemField(arr, idx, _) => {
                self.expr(idx, st)?;
                match arr.kind {
                    HExprKind::Local(id) => self.require_alive(id, st, e.span)?,
                    _ => self.expr(arr, st)?,
                }
            }
            HExprKind::IfNull { value, tmp, then, null } => {
                self.expr(value, st)?;
                if let Some(from) = base_local(value) {
                    let r = self.root(from);
                    self.alias.insert(*tmp, r);
                }
                self.expr(then, st)?;
                if let Some(n) = null {
                    self.expr(n, st)?;
                }
            }
            HExprKind::IsClass(x, ..) => match x.kind {
                HExprKind::Local(id) => self.require_alive(id, st, e.span)?,
                _ => self.expr(x, st)?,
            },
            HExprKind::TakeField(obj, _) if base_local(obj).is_some_and(|d| self.guard(d, "emptied", e.span).is_err()) => {
                return self.guard(base_local(obj).expect("checked"), "emptied", e.span);
            }
            HExprKind::TakeField(obj, _) => match obj.kind {
                HExprKind::Local(id) => self.require_alive(id, st, e.span)?,
                _ => self.expr(obj, st)?,
            },
            HExprKind::Bind(_, value, body) => {
                self.expr(value, st)?;
                self.expr(body, st)?;
            }
        }
        Ok(())
    }
}

/// The local a lent argument borrows from (`&c`, or `c.items` read
/// through an object), if it is one.
/// The `null` literal (NaT; for an interface value, both of its parts).
fn is_null(e: &HExpr) -> bool {
    match &e.kind {
        HExprKind::Word(w) => w.is_nat(),
        HExprKind::StructLit(parts) => !parts.is_empty() && parts.iter().all(is_null),
        _ => false,
    }
}

fn lent_root(e: &HExpr) -> Option<LocalId> {
    if !matches!(e.ty, Type::Lent(_) | Type::LentClass(_) | Type::LentIface(_) | Type::LentOptClass(_) | Type::LentOptIface(_)) {
        return None;
    }
    let mut e = e;
    loop {
        match &e.kind {
            HExprKind::Lend(id) | HExprKind::Local(id) => return Some(*id),
            HExprKind::ElemField(arr, _, _) | HExprKind::ToIface(arr, _) => e = arr,
            _ => e = borrowed_arg(e)?,
        }
    }
}

/// The span of a `give id` inside `e`, if there is one.
fn gives(e: &HExpr, id: LocalId) -> Option<crate::diag::Span> {
    let sub = |x: &HExpr| gives(x, id);
    match &e.kind {
        HExprKind::Give(g) if *g == id => Some(e.span),
        HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) | HExprKind::ToIface(a, _) => sub(a),
        HExprKind::Binary(_, a, b) | HExprKind::Index(a, b) | HExprKind::Bind(_, a, b) => sub(a).or_else(|| sub(b)),
        HExprKind::ElemField(a, b, _) => sub(a).or_else(|| sub(b)),
        HExprKind::TakeField(a, _) | HExprKind::IsClass(a, ..) => sub(a),
        HExprKind::IfNull { value, then, .. } => sub(value).or_else(|| sub(then)),
        HExprKind::Call(_, xs) | HExprKind::Builtin(_, xs) | HExprKind::Pack(xs) | HExprKind::StructLit(xs) => xs.iter().find_map(sub),
        HExprKind::Print(ps) => ps.iter().find_map(|(_, x)| sub(x)),
        _ => None,
    }
}

/// The span of a `pop(id)` or `remove(id, i)` inside `e`, if there is one.
fn takes_out(e: &HExpr, id: LocalId) -> Option<crate::diag::Span> {
    let sub = |x: &HExpr| takes_out(x, id);
    match &e.kind {
        HExprKind::Builtin(Builtin::Pop | Builtin::Remove, xs) if lent_root(&xs[0]) == Some(id) => Some(e.span),
        HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) | HExprKind::ToIface(a, _) => sub(a),
        HExprKind::Binary(_, a, b) | HExprKind::Index(a, b) | HExprKind::Bind(_, a, b) => sub(a).or_else(|| sub(b)),
        HExprKind::ElemField(a, b, _) => sub(a).or_else(|| sub(b)),
        HExprKind::TakeField(a, _) | HExprKind::IsClass(a, ..) => sub(a),
        HExprKind::IfNull { value, then, .. } => sub(value).or_else(|| sub(then)),
        HExprKind::Call(_, xs) | HExprKind::Builtin(_, xs) | HExprKind::Pack(xs) | HExprKind::StructLit(xs) => xs.iter().find_map(sub),
        HExprKind::Print(ps) => ps.iter().find_map(|(_, x)| sub(x)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::lex, parser::parse, sema};

    fn run(src: &str) -> R<()> {
        let hir = sema::check(&parse(lex(src).expect("lexes")).expect("parses")).unwrap_or_else(|d| panic!("sema: {}", d.message));
        check(&hir)
    }

    const DEFS: &str = "
        proc take(sym b[]: int) { burn b; }
        proc peek(sym &v[]: int) -> int { return v[0]; }
        proc both(sym &v[]: int, sym b[]: int) { burn b; }
        proc mk() -> sym int[] { sym b[]: int = claim 1; return b; }
    ";

    fn ok(body: &str) {
        let src = format!("{DEFS} proc main() {{ {body} }}");
        if let Err(d) = run(&src) {
            panic!("unexpected error `{}` in: {body}", d.message);
        }
    }

    fn err(body: &str) -> String {
        let src = format!("{DEFS} proc main() {{ {body} }}");
        match run(&src) {
            Ok(()) => panic!("expected an ownership error in: {body}"),
            Err(d) => format!("{}{}", d.message, d.note.map(|n| format!(" | {n}")).unwrap_or_default()),
        }
    }

    #[test]
    fn spec_example_is_sound() {
        let src = r#"
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k { acc += v[i]; }
    return acc;
}
proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 { b[i] = i * i; }
    s: int = sum(&b, 2) on NaT { burn b; return -1; };
    s = s << 1;
    tri s <=> 10 { - => print("less"); 0 => print("equal"); + => print("greater"); }
    burn b;
    return 0;
}"#;
        run(src).unwrap();
    }

    #[test]
    fn basic_lifecycle() {
        ok("sym b[]: int = claim 3; b[0] = 1; x: int = peek(&b); burn b;");
        ok("sym b[]: int = claim 3; take(give b);");
        ok("sym b[]: int = mk(); sym c[]: int = give b; burn c;");
        assert!(err("sym b[]: int = claim 3;").contains("`b` is never burned or given away"));
        assert!(err("sym b[]: int = claim 3; burn b; burn b;").contains("burned twice"));
        assert!(err("sym b[]: int = claim 3; burn b; b[0] = 1;").contains("used after it was burned"));
        assert!(err("sym b[]: int = claim 3; take(give b); x: int = b[0];").contains("used after it was given away"));
        assert!(err("sym b[]: int = claim 3; take(give b); burn b;").contains("given away and cannot be burned"));
        assert!(err("sym b[]: int = claim 3; take(give b); take(give b);").contains("used after it was given away"));
    }

    #[test]
    fn reuse_only_after_burn() {
        ok("sym b[]: int = claim 3; burn b; b = claim 5; burn b;");
        assert!(err("sym b[]: int = claim 3; b = claim 5; burn b;").contains("assigning to it would leak"));
        assert!(err("sym b[]: int = claim 3; take(give b); b = claim 5; burn b;").contains("only be reused after `burn`"));
    }

    #[test]
    fn calls_and_yield() {
        assert!(err("sym b[]: int = claim 3; both(&b, give b);").contains("borrowed and given away in the same call"));
        assert!(err("sym b[]: int = claim 3; b[0] = peek(&b) + 0; take(give b); b[1] = 2;").contains("given away"));
        assert!(err("sym b[]: int = claim 3; return;").contains("still owns a buffer when the proc returns"));
        assert!(run(&format!("{DEFS} proc keep(sym b[]: int) {{ }} proc main() {{}}")).unwrap_err().message.contains("when the proc ends"));
        run(&format!("{DEFS} proc pass(sym b[]: int) -> sym int[] {{ return b; }} proc main() {{}}")).unwrap();
    }

    #[test]
    fn on_nat_must_clean_up() {
        ok("sym b[]: int = claim 3; s: int = peek(&b) on NaT { burn b; return; }; burn b;");
        assert!(err("sym b[]: int = claim 3; s: int = peek(&b) on NaT { return; }; burn b;").contains("when the proc returns"));
    }

    #[test]
    fn tri_symmetry() {
        ok("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => burn b; + => take(give b); }");
        assert!(err("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => burn b; + => take(give b); } b = claim 1; burn b;")
            .contains("only be reused after `burn`"));
        ok("sym b[]: int = claim 3; t: trit = +; tri t { - => { burn b; return; } 0 => {} + => {} } burn b;");
        let e = err("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => {} + => burn b; }");
        assert!(e.contains("leave `b` in different states"), "{e}");
        assert!(e.contains("`-` arm: burned, `0` arm: still owned, `+` arm: burned"), "{e}");
        assert!(err("sym b[]: int = claim 3; t: trit = +; tri t { - => return; 0 => burn b; + => burn b; }").contains("when the proc returns"));
    }

    #[test]
    fn walk_rules() {
        ok("walk i in 0..2 { sym b[]: int = claim 3; b[0] = i; burn b; }");
        ok("sym b[]: int = claim 3; walk i in span 1 { b[i] = i; } burn b;");
        assert!(err("sym b[]: int = claim 3; walk i in 0..2 { burn b; }").contains("cannot change `b`, which lives outside the loop"));
        assert!(err("walk i in 0..2 { sym b[]: int = claim 3; break; }").contains("still owns a buffer at this `break`"));
        ok("walk i in 0..2 { sym b[]: int = claim 3; burn b; continue; }");
        assert!(err("sym b[]: int = claim 3; walk i in 0..2 { burn b; break; }").contains("cannot change `b`"));
        assert!(err("sym c[]: int = claim 1; burn c; walk i in 0..2 { c = claim 3; }").contains("cannot change `c`"));
    }
}
