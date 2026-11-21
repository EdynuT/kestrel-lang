//! Checked errors, as in Java: an error a proc may throw (a `throw`, or a
//! call to a proc with `throws`) is caught by a `try` around it or listed
//! in the proc's own `throws`. `Trap` (a failure of the VM) needs neither.
//!
//! Runs on the [`Hir`] after the checker, before the ownership checker.

use crate::diag::{Diagnostic, Span};
use crate::hir::*;
use crate::object::ERROR;

type R<T> = Result<T, Diagnostic>;

pub fn check(hir: &Hir) -> R<()> {
    for p in hir.procs.iter().filter(|p| matches!(p.link, Link::Own | Link::Shared)) {
        Checker { hir, proc: p, tries: Vec::new() }.block(&p.body)?;
    }
    Ok(())
}

struct Try<'h> {
    catches: &'h [HCatch],
    /// What the body may throw, as far as this `try`.
    thrown: Vec<&'static str>,
}

struct Checker<'h> {
    hir: &'h Hir,
    proc: &'h HProc,
    tries: Vec<Try<'h>>,
}

fn takes(c: &HCatch, error: &str) -> bool {
    match c.class {
        None => true,
        Some((n, _)) => n == error,
    }
}

impl<'h> Checker<'h> {
    /// Error `error` (a class name, or `Error`) may be thrown at `span`.
    fn raise(&mut self, error: &'static str, span: Span) -> R<()> {
        for t in self.tries.iter_mut().rev() {
            if !t.thrown.contains(&error) {
                t.thrown.push(error);
            }
            if t.catches.iter().any(|c| takes(c, error)) {
                return Ok(());
            }
        }
        if self.proc.throws.contains(&error) || self.proc.throws.contains(&ERROR) {
            return Ok(());
        }
        let name = &self.proc.name;
        let what = if error == ERROR { "an error".to_string() } else { format!("`{error}`") };
        Err(Diagnostic::new(format!("this may throw {what}, which `{name}` neither catches nor declares"), span)
            .with_note(format!("catch it with `try {{ ... }} catch e: {error} {{ ... }}`, or add `throws {error}` to `{name}`")))
    }

    fn block(&mut self, b: &'h HBlock) -> R<()> {
        b.stmts.iter().try_for_each(|s| self.stmt(s))
    }

    fn stmt(&mut self, s: &'h HStmt) -> R<()> {
        match &s.kind {
            HStmtKind::Let { value, on_nat, .. } => {
                self.expr(value)?;
                if let Some(b) = on_nat {
                    self.block(b)?;
                }
            }
            HStmtKind::Declare(_) | HStmtKind::Burn(_) | HStmtKind::Break | HStmtKind::Continue | HStmtKind::Trap(_) | HStmtKind::Dump(_) => {}
            HStmtKind::Assign { index, value, .. } => {
                if let Some(i) = index {
                    self.expr(i)?;
                }
                self.expr(value)?;
            }
            HStmtKind::SetElem { arr, index, value, .. } => {
                self.expr(arr)?;
                self.expr(index)?;
                self.expr(value)?;
            }
            HStmtKind::Return(v) => {
                if let Some(e) = v {
                    self.expr(e)?;
                }
            }
            HStmtKind::Tri { head, arms, .. } => {
                self.expr(head)?;
                arms.iter().try_for_each(|a| self.block(a))?;
            }
            HStmtKind::Block(b) => self.block(b)?,
            HStmtKind::While { cond, body, step } => {
                self.expr(cond)?;
                self.block(body)?;
                if let Some(b) = step {
                    self.block(b)?;
                }
            }
            HStmtKind::Exit(e) | HStmtKind::Expr(e) => self.expr(e)?,
            HStmtKind::Walk { span_tmp, start, end, body, .. } => {
                if let Some((_, k)) = span_tmp {
                    self.expr(k)?;
                }
                self.expr(start)?;
                self.expr(end)?;
                self.block(body)?;
            }
            HStmtKind::Throw { value, class } => {
                self.expr(value)?;
                self.raise(class.unwrap_or(ERROR), s.span)?;
            }
            HStmtKind::Try { body, catches } => {
                for (k, c) in catches.iter().enumerate() {
                    if let Some(prev) = catches[..k].iter().find(|p| c.class.map_or(p.class.is_none(), |(n, _)| takes(p, n))) {
                        let msg = match prev.class {
                            None => "this `catch` never runs: the `catch` of `Error` above takes every error".to_string(),
                            Some((n, _)) => format!("`{n}` is caught twice"),
                        };
                        return Err(Diagnostic::new(msg, c.span).with_note("put `catch e: Error` last, after the specific errors"));
                    }
                }
                self.tries.push(Try { catches, thrown: Vec::new() });
                let r = self.block(body);
                let t = self.tries.pop().expect("pushed above");
                r?;
                // Java's rule: catching an error the body never throws is a mistake.
                for c in catches {
                    if let Some((n, _)) = c.class
                        && n != kestrel_core::TRAP_CLASS
                        && !t.thrown.contains(&n)
                        && !t.thrown.contains(&ERROR)
                    {
                        return Err(Diagnostic::new(format!("nothing in this `try` throws `{n}`"), c.span)
                            .with_note("remove this `catch`, or check which proc you meant to call"));
                    }
                }
                catches.iter().try_for_each(|c| self.block(&c.body))?;
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &'h HExpr) -> R<()> {
        match &e.kind {
            HExprKind::Word(_)
            | HExprKind::Real(_)
            | HExprKind::Text(_)
            | HExprKind::Local(_)
            | HExprKind::Give(_)
            | HExprKind::Lend(_)
            | HExprKind::Field(..) => {}
            HExprKind::Neg(a)
            | HExprKind::Convert(a)
            | HExprKind::Claim(a)
            | HExprKind::ToIface(a, _)
            | HExprKind::TakeField(a, _)
            | HExprKind::IsClass(a, ..) => self.expr(a)?,
            HExprKind::Binary(_, a, b) | HExprKind::Index(a, b) | HExprKind::ElemField(a, b, _) | HExprKind::Bind(_, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
            }
            HExprKind::Call(pid, args) => {
                args.iter().try_for_each(|a| self.expr(a))?;
                for &t in &self.hir.procs[*pid].throws {
                    self.raise(t, e.span)?;
                }
            }
            HExprKind::Builtin(_, xs) | HExprKind::Pack(xs) | HExprKind::StructLit(xs) | HExprKind::NewObj(xs) => {
                xs.iter().try_for_each(|a| self.expr(a))?
            }
            HExprKind::Print(ps) => ps.iter().try_for_each(|(_, a)| self.expr(a))?,
            HExprKind::IfNull { value, then, null, .. } => {
                self.expr(value)?;
                self.expr(then)?;
                if let Some(n) = null {
                    self.expr(n)?;
                }
            }
        }
        Ok(())
    }
}
