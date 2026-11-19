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
        functions.push(ProcGen::new(p, &hir.structs, &mut pools)?.run()?);
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
    structs: &'a Structs,
    pools: &'a mut Pools,
    slots: Vec<Slot>,
    names: Vec<String>,
    code: Vec<u8>,
    loops: Vec<LoopLabels>,
    /// Slots taken by the parameters (a struct parameter takes several).
    param_slots: usize,
}

impl<'a> ProcGen<'a> {
    fn new(p: &'a HProc, structs: &'a Structs, pools: &'a mut Pools) -> R<Self> {
        // Each local takes as many slots as its type (a struct: one per
        // field). Parameters come first, in order, as the caller pushes them.
        // User variables get the low registers next; walk variables and
        // compiler temporaries go last so `dump` shows what the programmer wrote first.
        let n = p.locals.len();
        let mut order: Vec<usize> = (p.params..n).collect();
        order.sort_by_key(|&i| match p.locals[i].kind {
            LocalKind::Param | LocalKind::Let => 0,
            LocalKind::WalkVar => 1,
            LocalKind::Hidden => 2,
        });
        let mut slots = vec![0; n];
        let mut names = Vec::new();
        let mut param_slots = 0;
        for (k, i) in (0..p.params).chain(order).enumerate() {
            if names.len() + width(structs, p.locals[i].ty) > MAX_LOCALS {
                return Err(Diagnostic::new(
                    format!("`{}` has too many variables; a proc can have at most {MAX_LOCALS} slots", p.name),
                    p.body.span,
                )
                .with_note("split it into smaller procs (each struct field takes a slot)"));
            }
            slots[i] = names.len() as Slot;
            leaf_names(structs, p.locals[i].ty, &p.locals[i].name, &mut names);
            if k < p.params {
                param_slots = names.len();
            }
        }
        if param_slots > MAX_PARAMS {
            return Err(Diagnostic::new(
                format!("`{}` takes {param_slots} values; a proc can take at most {MAX_PARAMS}", p.name),
                p.body.span,
            )
            .with_note("each struct field counts as one value"));
        }
        Ok(ProcGen { p, structs, pools, slots, names, code: Vec::new(), loops: Vec::new(), param_slots })
    }

    fn width(&self, ty: Type) -> usize {
        width(self.structs, ty)
    }

    /// How many values an expression statement leaves to pop. Every proc
    /// call returns something (a dummy 0 for procs without `-> T`); built-ins
    /// without a result and prints push nothing.
    fn leftover(&self, e: &HExpr) -> usize {
        match &e.kind {
            HExprKind::Builtin(..) => usize::from(e.ty != Type::Void),
            HExprKind::Print(_) => 0,
            HExprKind::Bind(_, _, body) => self.leftover(body),
            HExprKind::Call(..) if e.ty == Type::Void => 1,
            _ => self.width(e.ty),
        }
    }

    /// The zero value of a type, as `claim` fills new arrays.
    fn push_zero(&mut self, ty: Type) {
        match ty {
            Type::Scalar(Scalar::Real) => self.push_real(Real::ZERO),
            Type::Text => {
                let k = self.pools.string("");
                self.emit(Op::PushS(k));
            }
            Type::Struct(n) => {
                for (_, t) in self.structs[n].clone() {
                    self.push_zero(t);
                }
            }
            _ => self.push_word(Word::ZERO),
        }
    }

    /// Loads `n` slots starting at `first`.
    fn load(&mut self, first: Slot, n: usize) {
        for k in 0..n {
            self.emit(Op::Load(first + k as Slot));
        }
    }

    /// Stores the top `n` stack values into the slots starting at `first`.
    fn store(&mut self, first: Slot, n: usize) {
        for k in (0..n).rev() {
            self.emit(Op::Store(first + k as Slot));
        }
    }

    fn run(mut self) -> R<Function> {
        self.block(&self.p.body)?;
        if !self.p.body.diverges() {
            // A proc without `-> T` that falls off its end returns nothing.
            self.push_word(Word::ZERO);
            self.emit(Op::Ret);
        }
        Ok(Function {
            name: self.p.name.clone(),
            params: self.param_slots as u8,
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
                // With `on NaT`, failed arithmetic on this line gives NaT for
                // the handler instead of trapping.
                if on_nat.is_some() {
                    self.emit(Op::Soft);
                }
                self.expr(value)?;
                if on_nat.is_some() {
                    self.emit(Op::Hard);
                }
                match on_nat {
                    None => {
                        self.store(self.slot(*local), self.width(value.ty));
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
            HStmtKind::Assign { local, index: None, field, value } => {
                self.expr(value)?;
                self.store(self.slot(*local) + *field as Slot, self.width(value.ty));
            }
            HStmtKind::Assign { local, index: Some(idx), field, value } => {
                self.emit(Op::Load(self.slot(*local)));
                self.expr(idx)?;
                self.expr(value)?;
                let n = self.width(value.ty);
                if *field == 0 && n == 1 {
                    self.emit(Op::StIdx);
                } else {
                    self.emit(Op::StElem { off: *field as u8, n: n as u8 });
                }
            }
            HStmtKind::Burn(id) => {
                self.emit(Op::Purge(self.slot(*id)));
            }
            HStmtKind::Return(v) => {
                match v {
                    Some(e) => self.expr(e)?,
                    None => self.push_word(Word::ZERO),
                }
                match self.p.ret {
                    Type::Struct(_) => self.emit(Op::RetN(self.width(self.p.ret) as u8)),
                    _ => self.emit(Op::Ret),
                };
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
            HStmtKind::Exit(code) => {
                self.expr(code)?;
                self.emit(Op::Exit);
            }
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
                for _ in 0..self.leftover(e) {
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
            HExprKind::Local(id) => self.load(self.slot(*id), self.width(e.ty)),
            HExprKind::Lend(id) => {
                self.emit(Op::Load(self.slot(*id)));
            }
            HExprKind::Field(id, off) => self.load(self.slot(*id) + *off as Slot, self.width(e.ty)),
            HExprKind::StructLit(items) => {
                for item in items {
                    self.expr(item)?;
                }
            }
            HExprKind::Bind(tmp, value, body) => {
                self.expr(value)?;
                self.store(self.slot(*tmp), self.width(value.ty));
                self.expr(body)?;
            }
            HExprKind::Give(id) => {
                self.emit(Op::Move(self.slot(*id)));
            }
            HExprKind::Neg(a) => {
                self.expr(a)?;
                self.emit(if real(a.ty) { Op::FNeg } else { Op::Neg });
            }
            HExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => self.short_circuit(*op, a, b)?,
            HExprKind::Binary(op, a, b) if a.ty == Type::Text => {
                self.expr(a)?;
                self.expr(b)?;
                match op {
                    BinOp::Add => self.emit(Op::TCat),
                    BinOp::Eq => self.emit(Op::TEq),
                    _ => {
                        self.emit(Op::TEq);
                        self.emit(Op::Neg) // !=
                    }
                };
            }
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
                    match self.width(e.ty) {
                        1 => self.emit(Op::LdIdx),
                        w => self.emit(Op::LdElem { off: 0, n: w as u8 }),
                    };
                }
            }
            HExprKind::Call(pid, args) => {
                for a in args {
                    self.expr(a)?;
                }
                let argc: usize = args.iter().map(|a| self.width(a.ty)).sum();
                self.emit(Op::Call { f: *pid as u32, argc: argc as u8 });
            }
            HExprKind::Print(pieces) => {
                for (fmt, p) in pieces {
                    self.expr(p)?;
                    if p.ty == Type::Void {
                        continue; // a struct, already printed field by field
                    }
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
                    Builtin::Sub => Op::TSub,
                    Builtin::ToInt => Op::TToI,
                    Builtin::ToReal => Op::TToF,
                    Builtin::CharIs(k) => Op::CharIs(*k),
                    Builtin::ReadFile => Op::ReadFile,
                    Builtin::FileExists => Op::FileExists,
                    Builtin::WriteFile => Op::WriteFile,
                    Builtin::WriteBytes => Op::WriteBytes,
                    Builtin::ReadLine => Op::ReadLine,
                    Builtin::Eof => Op::Eof,
                    Builtin::ArgCount => Op::ArgCount,
                    Builtin::Arg => Op::Arg,
                    Builtin::ToText(Fmt::Trits) => Op::TStrT,
                    Builtin::ToText(Fmt::Hepta) => Op::TStrV,
                    Builtin::ToText(_) => Op::TStr,
                });
            }
            HExprKind::Claim(n) => {
                self.expr(n)?;
                let elem = elem_ty(e.ty);
                if elem.is_int() || matches!(elem, Type::Enum(_)) {
                    self.emit(if is_sym(e.ty) { Op::Alloc } else { Op::AllocZ });
                } else {
                    // Other elements start from a typed zero: 0.0, "", or a struct of zeros.
                    self.push_zero(elem);
                    self.emit(Op::AllocT { w: self.width(elem) as u8, sym: is_sym(e.ty) });
                }
            }
            HExprKind::Pack(items) => {
                for item in items {
                    self.expr(item)?;
                }
                let (n, sym) = (items.len() as u32, is_sym(e.ty));
                match self.width(elem_ty(e.ty)) {
                    1 => self.emit(Op::Pack { n, sym }),
                    w => self.emit(Op::PackT { n, w: w as u8, sym }),
                };
            }
            HExprKind::ElemField(arr, idx, off) => {
                self.emit(Op::Load(self.slot(*arr)));
                self.expr(idx)?;
                self.emit(Op::LdElem { off: *off as u8, n: self.width(e.ty) as u8 });
            }
        }
        Ok(())
    }
}

/// The element type of an array type.
fn elem_ty(t: Type) -> Type {
    match t {
        Type::Own(b) | Type::Lent(b) => b.ty(),
        other => other,
    }
}

fn is_sym(t: Type) -> bool {
    matches!(t, Type::Own(b) | Type::Lent(b) if b.sym)
}

/// Names for each slot of a local: a struct `p` gives `p.x`, `p.y`, ...
fn leaf_names(structs: &Structs, ty: Type, name: &str, out: &mut Vec<String>) {
    match ty {
        Type::Struct(s) => {
            for (f, t) in &structs[s] {
                leaf_names(structs, *t, &format!("{name}.{f}"), out);
            }
        }
        _ => out.push(name.to_string()),
    }
}
