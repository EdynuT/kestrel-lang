//! Code generator: checked [`Hir`] -> `.ksb` [`Module`].
//!
//! The VM is a stack machine, so expressions compile to postfix order.
//! Locals become frame slots (slot 0..8 is shown as R-4..R+4): parameters
//! first, then everything else.

use std::collections::HashMap;

use kestrel_core::isa::{Addr, Op, Slot};
use kestrel_core::limits::{MAX_LOCALS, MAX_PARAMS};
use kestrel_core::ksb::{Function, Module};
use kestrel_core::{Real, Word, Word27};

use crate::diag::Diagnostic;
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

pub fn generate(hir: &Hir) -> R<Module> {
    let mut pools = Pools::default();
    let mut functions = Vec::new();
    for p in &hir.procs {
        functions.push(ProcGen::new(p, &mut pools)?.run()?);
    }
    let m = Module { entry: hir.entry as u32, consts: pools.consts, strings: pools.strings, functions };
    debug_assert_eq!(m.verify(), Ok(()));
    Ok(m)
}

#[derive(Default)]
struct Pools {
    consts: Vec<Word27>,
    const_ids: HashMap<u64, u32>,
    strings: Vec<String>,
    string_ids: HashMap<String, u32>,
}

impl Pools {
    fn constant(&mut self, w: Word27) -> u32 {
        *self.const_ids.entry(w.bits()).or_insert_with(|| {
            self.consts.push(w);
            (self.consts.len() - 1) as u32
        })
    }

    fn string(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.string_ids.get(s) {
            return id;
        }
        self.strings.push(s.to_string());
        let id = (self.strings.len() - 1) as u32;
        self.string_ids.insert(s.to_string(), id);
        id
    }
}

/// Jump sites waiting for their target address.
struct LoopLabels {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

struct ProcGen<'a> {
    p: &'a HProc,
    pools: &'a mut Pools,
    slots: Vec<Slot>,
    names: Vec<String>,
    code: Vec<u8>,
    loops: Vec<LoopLabels>,
}

impl<'a> ProcGen<'a> {
    fn new(p: &'a HProc, pools: &'a mut Pools) -> R<Self> {
        let n = p.locals.len();
        if n > MAX_LOCALS {
            return Err(Diagnostic::new(
                format!("`{}` has {n} variables; a proc can have at most {MAX_LOCALS}", p.name),
                p.body.span,
            )
            .with_note("split it into smaller procs"));
        }
        let mut slots = vec![Slot::MAX; n];
        let mut taken: HashMap<Slot, usize> = HashMap::new();
        for (i, slot) in slots.iter_mut().enumerate().take(p.params) {
            *slot = i as Slot;
            taken.insert(i as Slot, i);
        }
        // User variables get the low registers; walk variables and compiler
        // temporaries go last so `dump` shows what the programmer wrote first.
        let mut order: Vec<usize> = (0..n).filter(|&i| slots[i] == Slot::MAX).collect();
        order.sort_by_key(|&i| match p.locals[i].kind {
            LocalKind::Param | LocalKind::Let => 0,
            LocalKind::WalkVar => 1,
            LocalKind::Hidden => 2,
        });
        let mut next: Slot = 0;
        for i in order {
            while taken.contains_key(&next) {
                next += 1;
            }
            slots[i] = next;
            taken.insert(next, i);
            next += 1;
        }
        let count = taken.keys().max().map_or(0, |m| usize::from(*m) + 1);
        let mut names = vec![String::new(); count];
        for (i, l) in p.locals.iter().enumerate() {
            names[usize::from(slots[i])] = l.name.clone();
        }
        if p.params > MAX_PARAMS {
            return Err(Diagnostic::new(format!("a proc can take at most {MAX_PARAMS} parameters"), p.body.span));
        }
        Ok(ProcGen { p, pools, slots, names, code: Vec::new(), loops: Vec::new() })
    }

    fn run(mut self) -> R<Function> {
        self.block(&self.p.body)?;
        if !self.p.body.diverges() {
            // A proc without `-> T` that falls off its end yields nothing.
            self.push_word(Word::ZERO);
            self.emit(Op::Ret);
        }
        Ok(Function {
            name: self.p.name.clone(),
            params: self.p.params as u8,
            locals: self.names.len() as u16,
            code: self.code,
            local_names: self.names,
        })
    }

    // ---- emission helpers ----

    fn here(&self) -> Addr {
        self.code.len() as Addr
    }

    fn emit(&mut self, op: Op) -> usize {
        let at = self.code.len();
        op.encode(&mut self.code);
        at
    }

    /// Patches the `n`-th u32 operand of the instruction at `at`.
    fn patch(&mut self, at: usize, n: usize, target: Addr) {
        let pos = at + 1 + 4 * n;
        self.code[pos..pos + 4].copy_from_slice(&target.to_le_bytes());
    }

    fn slot(&self, id: LocalId) -> Slot {
        self.slots[id]
    }

    /// `a && b` / `a || b`, evaluating `b` only when it can change the result
    /// (Kleene: `- && x == -`, `+ || x == +`; NaT on the left stays NaT).
    /// ```text
    ///       a; jnat NAT; br3 NEG, ZERO, POS
    /// &&:   NEG: push -; jmp END   ZERO: push 0; b; kand; jmp END   POS: b; jmp END
    /// ||:   NEG: b; jmp END        ZERO: push 0; b; kor; jmp END    POS: push +; jmp END
    ///       NAT: push NaT
    /// END:
    /// ```
    fn short_circuit(&mut self, op: BinOp, a: &HExpr, b: &HExpr) -> R<()> {
        let and = op == BinOp::And;
        self.expr(a)?;
        let jnat = self.emit(Op::JNat(0));
        let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
        let mut exits = Vec::new();
        for (arm, decided) in [(0, and), (1, false), (2, !and)] {
            let start = self.here();
            self.patch(br, arm, start);
            if decided {
                self.push_word(if and { Word::NEG_ONE } else { Word::ONE });
            } else if arm == 1 {
                self.push_word(Word::ZERO);
                self.expr(b)?;
                self.emit(if and { Op::KAnd } else { Op::KOr });
            } else {
                self.expr(b)?;
            }
            exits.push(self.emit(Op::Jmp(0)));
        }
        let nat = self.here();
        self.patch(jnat, 0, nat);
        self.push_word(Word::NAT);
        let end = self.here();
        for at in exits {
            self.patch(at, 0, end);
        }
        Ok(())
    }

    fn push_word(&mut self, w: Word) {
        let k = self.pools.constant(Word27::from(w));
        self.emit(Op::PushC(k));
    }

    fn push_real(&mut self, r: Real) {
        let k = self.pools.constant(Word27::from(r));
        self.emit(Op::PushR(k));
    }

    /// Integer results narrower than a word overflow into NaT at their width.
    fn narrow(&mut self, ty: Type) {
        match ty {
            Type::Scalar(Scalar::Tryte) => {
                self.emit(Op::Narrow6);
            }
            Type::Scalar(Scalar::Trit) => {
                self.emit(Op::Narrow1);
            }
            _ => {}
        }
    }

    // ---- statements ----

    fn block(&mut self, b: &HBlock) -> R<()> {
        b.stmts.iter().try_for_each(|s| self.stmt(s))
    }

    fn stmt(&mut self, s: &HStmt) -> R<()> {
        match &s.kind {
            HStmtKind::Let { local, value, on_nat } => {
                self.expr(value)?;
                match on_nat {
                    None => {
                        self.emit(Op::Store(self.slot(*local)));
                    }
                    Some(handler) => {
                        // value; jnat H; store x; jmp END; H: handler (diverges); END:
                        let jnat = self.emit(Op::JNat(0));
                        self.emit(Op::Store(self.slot(*local)));
                        let jmp = self.emit(Op::Jmp(0));
                        let h = self.here();
                        self.patch(jnat, 0, h);
                        self.block(handler)?;
                        let end = self.here();
                        self.patch(jmp, 0, end);
                    }
                }
            }
            HStmtKind::Assign { local, index: None, value } => {
                self.expr(value)?;
                self.emit(Op::Store(self.slot(*local)));
            }
            HStmtKind::Assign { local, index: Some(idx), value } => {
                self.emit(Op::Load(self.slot(*local)));
                self.expr(idx)?;
                self.expr(value)?;
                self.emit(Op::StIdx);
            }
            HStmtKind::Burn(id) => {
                self.emit(Op::Purge(self.slot(*id)));
            }
            HStmtKind::Yield(v) => {
                match v {
                    Some(e) => self.expr(e)?,
                    None => self.push_word(Word::ZERO),
                }
                self.emit(Op::Ret);
            }
            HStmtKind::Break => {
                let at = self.emit(Op::Jmp(0));
                self.loops.last_mut().expect("break inside walk").breaks.push(at);
            }
            HStmtKind::Continue => {
                let at = self.emit(Op::Jmp(0));
                self.loops.last_mut().expect("continue inside walk").continues.push(at);
            }
            HStmtKind::Block(b) => self.block(b)?,
            HStmtKind::Trap(reason) => {
                let k = self.pools.string(reason);
                self.emit(Op::Trap(k));
            }
            HStmtKind::Tri { head, arms, .. } => {
                self.expr(head)?;
                let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                let mut exits = Vec::new();
                for (n, arm) in arms.iter().enumerate() {
                    let start = self.here();
                    self.patch(br, n, start);
                    self.block(arm)?;
                    if !arm.diverges() {
                        exits.push(self.emit(Op::Jmp(0)));
                    }
                }
                let end = self.here();
                for at in exits {
                    self.patch(at, 0, end);
                }
            }
            HStmtKind::While { cond, body, step } => self.while_loop(cond, body, step.as_ref())?,
            HStmtKind::Walk { var, span_tmp, start, end, end_tmp, body } => self.walk(*var, span_tmp, start, end, *end_tmp, body)?,
            HStmtKind::Expr(e) => {
                self.expr(e)?;
                if leaves_value(e) {
                    self.emit(Op::Pop);
                }
            }
            HStmtKind::Dump(label) => {
                let k = self.pools.string(label);
                self.emit(Op::Dump(k));
            }
        }
        Ok(())
    }

    /// ```text
    ///       [k -> span_tmp]  start -> var  end -> end_tmp
    ///       cmp3 var, end_tmp; br3 BODY, BODY, EXIT
    /// BODY: body
    /// CONT: cmp3 var, end_tmp; br3 INC, EXIT, EXIT     (compare before +1: no overflow at the top)
    /// INC:  var = var + 1; jmp BODY
    /// EXIT:
    /// ```
    fn walk(&mut self, var: LocalId, span_tmp: &Option<(LocalId, HExpr)>, start: &HExpr, end: &HExpr, end_tmp: LocalId, body: &HBlock) -> R<()> {
        if let Some((tmp, k)) = span_tmp {
            self.expr(k)?;
            self.emit(Op::Store(self.slot(*tmp)));
        }
        let (v, e) = (self.slot(var), self.slot(end_tmp));
        self.expr(start)?;
        self.emit(Op::Store(v));
        self.expr(end)?;
        self.emit(Op::Store(e));

        self.emit(Op::Load(v));
        self.emit(Op::Load(e));
        self.emit(Op::Cmp3);
        let enter = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });

        let body_at = self.here();
        self.loops.push(LoopLabels { breaks: Vec::new(), continues: Vec::new() });
        self.block(body)?;
        let labels = self.loops.pop().expect("pushed above");

        let cont = self.here();
        self.emit(Op::Load(v));
        self.emit(Op::Load(e));
        self.emit(Op::Cmp3);
        let again = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
        let inc = self.here();
        self.emit(Op::Load(v));
        self.push_word(Word::ONE);
        self.emit(Op::Add);
        self.emit(Op::Store(v));
        self.emit(Op::Jmp(body_at));
        let exit = self.here();

        self.patch(enter, 0, body_at);
        self.patch(enter, 1, body_at);
        self.patch(enter, 2, exit);
        self.patch(again, 0, inc);
        self.patch(again, 1, exit);
        self.patch(again, 2, exit);
        for at in labels.breaks {
            self.patch(at, 0, exit);
        }
        for at in labels.continues {
            self.patch(at, 0, cont);
        }
        Ok(())
    }

    /// ```text
    /// TOP:  cond; br3 EXIT, UNKNOWN, BODY     (NaT traps in br3)
    /// UNKNOWN: trap "loop condition is 0 (unknown)"
    /// BODY: body
    /// CONT: step; jmp TOP                      (continue -> CONT, break -> EXIT)
    /// EXIT:
    /// ```
    fn while_loop(&mut self, cond: &HExpr, body: &HBlock, step: Option<&HBlock>) -> R<()> {
        let top = self.here();
        self.expr(cond)?;
        let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
        let unknown = self.here();
        let k = self.pools.string("loop condition is 0 (unknown)");
        self.emit(Op::Trap(k));
        let body_at = self.here();
        self.loops.push(LoopLabels { breaks: Vec::new(), continues: Vec::new() });
        self.block(body)?;
        let labels = self.loops.pop().expect("pushed above");
        let cont = self.here();
        if let Some(step) = step {
            self.block(step)?;
        }
        self.emit(Op::Jmp(top));
        let exit = self.here();
        self.patch(br, 0, exit);
        self.patch(br, 1, unknown);
        self.patch(br, 2, body_at);
        for at in labels.breaks {
            self.patch(at, 0, exit);
        }
        for at in labels.continues {
            self.patch(at, 0, cont);
        }
        Ok(())
    }

    // ---- expressions ----

    fn expr(&mut self, e: &HExpr) -> R<()> {
        let real = |t: Type| t == Type::Scalar(Scalar::Real);
        match &e.kind {
            HExprKind::Word(w) => self.push_word(*w),
            HExprKind::Real(r) => self.push_real(*r),
            HExprKind::Text(s) => {
                let k = self.pools.string(s);
                self.emit(Op::PushS(k));
            }
            HExprKind::Local(id) | HExprKind::Lend(id) => {
                self.emit(Op::Load(self.slot(*id)));
            }
            HExprKind::Give(id) => {
                self.emit(Op::Move(self.slot(*id)));
            }
            HExprKind::Neg(a) => {
                self.expr(a)?;
                self.emit(if real(a.ty) { Op::FNeg } else { Op::Neg });
            }
            HExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => self.short_circuit(*op, a, b)?,
            HExprKind::Binary(op, a, b) if op.is_comparison() => {
                self.expr(a)?;
                self.expr(b)?;
                self.emit(if real(a.ty) { Op::FCmp3 } else { Op::Cmp3 });
                // Answer (+ = true) for cmp3 results -, 0, + as bits 0, 1, 2.
                let mask = match op {
                    BinOp::Eq => 0b010,
                    BinOp::Ne => 0b101,
                    BinOp::Lt => 0b001,
                    BinOp::Le => 0b011,
                    BinOp::Gt => 0b100,
                    _ => 0b110, // Ge
                };
                self.emit(Op::Test(mask));
            }
            HExprKind::Binary(op, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
                let is_real = real(a.ty);
                let code = match (op, is_real) {
                    (BinOp::Cmp3, false) => Op::Cmp3,
                    (BinOp::Cmp3, true) => Op::FCmp3,
                    (BinOp::Add, false) => Op::Add,
                    (BinOp::Sub, false) => Op::Sub,
                    (BinOp::Mul, false) => Op::Mul,
                    (BinOp::Div, false) => Op::Div,
                    (BinOp::Mod, false) => Op::Mod,
                    (BinOp::Pow, false) => Op::Pow,
                    (BinOp::Shl, false) => Op::Shl3,
                    (BinOp::Shr, false) => Op::Shr3,
                    (BinOp::Add, true) => Op::FAdd,
                    (BinOp::Sub, true) => Op::FSub,
                    (BinOp::Mul, true) => Op::FMul,
                    (BinOp::Div, true) => Op::FDiv,
                    (BinOp::Pow, true) => Op::FPow,
                    (op, true) => unreachable!("sema rejects `{}` on reals", op.symbol()),
                    (op, false) => unreachable!("`{}` is handled above", op.symbol()),
                };
                self.emit(code);
                if !is_real {
                    self.narrow(e.ty);
                }
            }
            HExprKind::Convert(a) => {
                self.expr(a)?;
                match (a.ty, e.ty) {
                    (from, to) if from.is_int() && to.is_int() => {
                        let (Type::Scalar(f), Type::Scalar(t)) = (from, to) else { unreachable!() };
                        if t.rank() < f.rank() {
                            self.narrow(to);
                        }
                    }
                    (from, Type::Scalar(Scalar::Real)) if from.is_int() => {
                        self.emit(Op::IToF);
                    }
                    (Type::Scalar(Scalar::Real), to) => {
                        self.emit(Op::FToI);
                        self.narrow(to);
                    }
                    (from, to) => unreachable!("sema rejects converting {from} to {to}"),
                }
            }
            HExprKind::Index(base, idx) => {
                if base.ty == Type::Text {
                    self.expr(base)?;
                    self.expr(idx)?;
                    self.emit(Op::TIdx);
                } else {
                    let HExprKind::Local(id) = base.kind else { unreachable!("buffer index base is a local") };
                    self.emit(Op::Load(self.slot(id)));
                    self.expr(idx)?;
                    self.emit(Op::LdIdx);
                }
            }
            HExprKind::Call(pid, args) => {
                for a in args {
                    self.expr(a)?;
                }
                self.emit(Op::Call { f: *pid as u32, argc: args.len() as u8 });
            }
            HExprKind::Print(pieces) => {
                for (fmt, p) in pieces {
                    self.expr(p)?;
                    self.emit(match fmt {
                        Fmt::Plain if p.ty == Type::Text => Op::Print,
                        Fmt::Plain => Op::Show,
                        Fmt::Trits => Op::ShowT,
                        Fmt::Hepta => Op::ShowV,
                        Fmt::Char => Op::Emit,
                    });
                }
            }
            HExprKind::Builtin(b, args) => {
                for a in args {
                    self.expr(a)?;
                }
                self.emit(match b {
                    Builtin::Emit => Op::Emit,
                    Builtin::Input => Op::Input,
                    Builtin::Len => Op::TLen,
                    Builtin::ToText(Fmt::Trits) => Op::TStrT,
                    Builtin::ToText(Fmt::Hepta) => Op::TStrV,
                    Builtin::ToText(_) => Op::TStr,
                });
            }
            HExprKind::Claim(n) => {
                self.expr(n)?;
                self.emit(if is_sym(e.ty) { Op::Alloc } else { Op::AllocZ });
            }
            HExprKind::Pack(items) => {
                for item in items {
                    self.expr(item)?;
                }
                self.emit(Op::Pack { n: items.len() as u32, sym: is_sym(e.ty) });
            }
        }
        Ok(())
    }
}

fn is_sym(t: Type) -> bool {
    matches!(t, Type::Own(b) | Type::Lent(b) if b.sym)
}

/// Whether an expression statement leaves a value to pop. Every proc call
/// returns one (a dummy 0 for procs without `-> T`); built-ins without a
/// result push nothing.
fn leaves_value(e: &HExpr) -> bool {
    match e.kind {
        HExprKind::Builtin(..) => e.ty != Type::Void,
        HExprKind::Print(_) => false,
        _ => true,
    }
}
