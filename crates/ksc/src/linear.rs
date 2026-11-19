//! Ownership checker (SPEC §6): every `own` buffer is burned or given away
//! exactly once on every path.
//!
//! The checker walks each proc tracking every own local as alive, burned or
//! given. Leaving a block or yielding with a buffer still alive is a leak;
//! touching a burned or given buffer is a use-after-free; the arms of a `tri`
//! that fall through must agree; a loop body (`walk`, `while`) must leave outer buffers as it
//! found them.

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
}

impl St {
    /// Burned and given are both "consumed": arms may differ in how.
    fn agrees(self, other: St) -> bool {
        self == other || (self.consumed() && other.consumed())
    }

    fn consumed(self) -> bool {
        matches!(self, St::Burned | St::Given)
    }

    fn describe(self) -> &'static str {
        match self {
            St::Untracked => "not declared",
            St::Alive => "still owned",
            St::Burned => "burned",
            St::Given => "given away",
        }
    }
}

type State = Vec<St>;

struct LoopCx {
    entry: State,
    /// Own locals declared inside this walk's body.
    inner: Vec<LocalId>,
}

pub fn check(hir: &Hir) -> R<()> {
    hir.procs.iter().try_for_each(check_proc)
}

fn check_proc(p: &HProc) -> R<()> {
    let mut st = vec![St::Untracked; p.locals.len()];
    for (i, s) in st.iter_mut().enumerate().take(p.params) {
        if is_own(p.locals[i].ty) {
            *s = St::Alive;
        }
    }
    let mut ck = Checker { p, loops: Vec::new() };
    if let Some(end) = ck.block(&p.body, st)? {
        // A proc without `-> T` may fall off its end: that is an implicit `yield;`.
        let end_span = Span::new(p.body.span.end - 1, p.body.span.end);
        ck.no_alive(&end, end_span, "the proc ends")?;
    }
    Ok(())
}

fn is_own(t: Type) -> bool {
    matches!(t, Type::Own(_))
}

struct Checker<'h> {
    p: &'h HProc,
    loops: Vec<LoopCx>,
}

impl Checker<'_> {
    fn name(&self, id: LocalId) -> &str {
        &self.p.locals[id].name
    }

    fn block(&mut self, b: &HBlock, mut st: State) -> R<Option<State>> {
        for s in &b.stmts {
            match self.stmt(s, st)? {
                Some(next) => st = next,
                None => return Ok(None),
            }
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

    /// Every buffer still alive at a yield (or at the end of a proc) leaks.
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
        }
    }

    fn stmt(&mut self, s: &HStmt, mut st: State) -> R<Option<State>> {
        match &s.kind {
            HStmtKind::Let { local, value, on_nat } => {
                self.expr(value, &mut st)?;
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
            HStmtKind::Assign { local, index, value } => {
                if let Some(i) = index {
                    self.expr(i, &mut st)?;
                }
                self.expr(value, &mut st)?;
                if index.is_some() {
                    // Checked after the operands so `b[0] = f(give b)` is caught.
                    self.require_alive(*local, &st, s.span)?;
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
                        St::Burned | St::Untracked => st[*local] = St::Alive,
                    }
                }
            }
            HStmtKind::Burn(id) => {
                let name = self.name(*id);
                match st[*id] {
                    St::Alive => st[*id] = St::Burned,
                    St::Burned => {
                        return Err(Diagnostic::new(format!("`{name}` is burned twice"), s.span)
                            .with_note("a buffer can be burned only once"));
                    }
                    St::Given | St::Untracked => {
                        return Err(Diagnostic::new(format!("`{name}` was given away and cannot be burned here"), s.span)
                            .with_note("whoever received it is now responsible for burning it"));
                    }
                }
            }
            HStmtKind::Yield(v) => {
                if let Some(e) = v {
                    self.expr(e, &mut st)?;
                }
                self.no_alive(&st, s.span, "the proc yields")?;
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
                let Some((_, mut first)) = ends.first().cloned() else {
                    return Ok(None); // every arm diverges
                };
                for (_, other) in &ends[1..] {
                    if let Some(id) = (0..first.len()).find(|&i| !first[i].agrees(other[i])) {
                        let detail: Vec<String> =
                            ends.iter().map(|(label, s)| format!("{label}: {}", s[id].describe())).collect();
                        let what = match origin {
                            Branch::Tri => "the arms of this `tri`",
                            Branch::If => "the branches of this `if`",
                            Branch::Match => "the arms of this `match`",
                        };
                        return Err(Diagnostic::new(
                            format!("{what} leave `{}` in different states", self.name(id)),
                            s.span,
                        )
                        .with_note(format!("{} (every path that continues must agree)", detail.join(", "))));
                    }
                    // If any arm gave the buffer away, its name cannot be reused afterwards.
                    for (f, o) in first.iter_mut().zip(other) {
                        if *o == St::Given {
                            *f = St::Given;
                        }
                    }
                }
                st = first;
            }
            HStmtKind::While { cond, body, step } => {
                self.expr(cond, &mut st)?;
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

    fn expr(&self, e: &HExpr, st: &mut State) -> R<()> {
        match &e.kind {
            HExprKind::Word(_) | HExprKind::Real(_) | HExprKind::Text(_) | HExprKind::Local(_) => {}
            HExprKind::Give(id) => {
                self.require_alive(*id, st, e.span)?;
                st[*id] = St::Given;
            }
            HExprKind::Lend(id) => self.require_alive(*id, st, e.span)?,
            HExprKind::Neg(a) | HExprKind::Convert(a) | HExprKind::Claim(a) => self.expr(a, st)?,
            HExprKind::Pack(items) => items.iter().try_for_each(|a| self.expr(a, st))?,
            HExprKind::Binary(_, a, b) => {
                self.expr(a, st)?;
                self.expr(b, st)?;
            }
            HExprKind::Index(base, idx) => {
                self.expr(idx, st)?;
                match base.kind {
                    HExprKind::Local(id) => self.require_alive(id, st, base.span)?,
                    _ => self.expr(base, st)?,
                }
            }
            HExprKind::Call(_, args) => {
                let lent: Vec<LocalId> =
                    args.iter().filter_map(|a| if let HExprKind::Lend(id) = a.kind { Some(id) } else { None }).collect();
                for (i, a) in args.iter().enumerate() {
                    if let HExprKind::Give(id) = a.kind
                        && lent.contains(&id)
                    {
                        return Err(Diagnostic::new(format!("`{}` is lent and given away in the same call", self.name(id)), a.span)
                            .with_note("the callee would borrow an array it also owns"));
                    }
                    if let HExprKind::Lend(id) = a.kind
                        && args[..i].iter().any(|b| b.kind == HExprKind::Lend(id))
                    {
                        return Err(Diagnostic::new(format!("`{}` is lent twice in the same call", self.name(id)), a.span)
                            .with_note("the proc would see one array under two names: a write through one would change the other"));
                    }
                    self.expr(a, st)?;
                }
            }
            HExprKind::Builtin(_, args) => args.iter().try_for_each(|a| self.expr(a, st))?,
            HExprKind::Print(pieces) => pieces.iter().try_for_each(|(_, a)| self.expr(a, st))?,
        }
        Ok(())
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
        proc take(own sym b[]: int) { burn b; }
        proc peek(lent sym v[]: int) -> int { yield v[0]; }
        proc both(lent sym v[]: int, own sym b[]: int) { burn b; }
        proc mk() -> sym int[] { sym b[]: int = claim 1; yield b; }
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
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k { acc += v[i]; }
    yield acc;
}
proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 { b[i] = i * i; }
    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
    s = s << 1;
    tri s <=> 10 { - => print("less"); 0 => print("equal"); + => print("greater"); }
    burn b;
    yield 0;
}"#;
        run(src).unwrap();
    }

    #[test]
    fn basic_lifecycle() {
        ok("sym b[]: int = claim 3; b[0] = 1; x := peek(lend b); burn b;");
        ok("sym b[]: int = claim 3; take(give b);");
        ok("sym b[]: int = mk(); sym c[]: int = give b; burn c;");
        assert!(err("sym b[]: int = claim 3;").contains("`b` is never burned or given away"));
        assert!(err("sym b[]: int = claim 3; burn b; burn b;").contains("burned twice"));
        assert!(err("sym b[]: int = claim 3; burn b; b[0] = 1;").contains("used after it was burned"));
        assert!(err("sym b[]: int = claim 3; take(give b); x := b[0];").contains("used after it was given away"));
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
        assert!(err("sym b[]: int = claim 3; both(lend b, give b);").contains("lent and given away in the same call"));
        assert!(err("sym b[]: int = claim 3; b[0] = peek(lend b) + 0; take(give b); b[1] = 2;").contains("given away"));
        assert!(err("sym b[]: int = claim 3; yield;").contains("still owns a buffer when the proc yields"));
        assert!(run(&format!("{DEFS} proc keep(own sym b[]: int) {{ }} proc main() {{}}")).unwrap_err().message.contains("when the proc ends"));
        run(&format!("{DEFS} proc pass(own sym b[]: int) -> sym int[] {{ yield b; }} proc main() {{}}")).unwrap();
    }

    #[test]
    fn on_nat_must_clean_up() {
        ok("sym b[]: int = claim 3; s := peek(lend b) on NaT { burn b; yield; }; burn b;");
        assert!(err("sym b[]: int = claim 3; s := peek(lend b) on NaT { yield; }; burn b;").contains("when the proc yields"));
    }

    #[test]
    fn tri_symmetry() {
        ok("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => burn b; + => take(give b); }");
        assert!(err("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => burn b; + => take(give b); } b = claim 1; burn b;")
            .contains("only be reused after `burn`"));
        ok("sym b[]: int = claim 3; t: trit = +; tri t { - => { burn b; yield; } 0 => {} + => {} } burn b;");
        let e = err("sym b[]: int = claim 3; t: trit = +; tri t { - => burn b; 0 => {} + => burn b; }");
        assert!(e.contains("leave `b` in different states"), "{e}");
        assert!(e.contains("`-` arm: burned, `0` arm: still owned, `+` arm: burned"), "{e}");
        assert!(err("sym b[]: int = claim 3; t: trit = +; tri t { - => yield; 0 => burn b; + => burn b; }").contains("when the proc yields"));
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
