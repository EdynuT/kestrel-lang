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
        functions.push(ProcGen::new(p, &hir.structs, &hir.classes, &hir.ifaces, &mut pools)?.run()?);
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
    classes: &'a Structs,
    ifaces: &'a Ifaces,
    pools: &'a mut Pools,
    slots: Vec<Slot>,
    names: Vec<String>,
    code: Vec<u8>,
    loops: Vec<LoopLabels>,
    /// Slots taken by the parameters (a struct parameter takes several).
    param_slots: usize,
}

impl<'a> ProcGen<'a> {
    fn new(p: &'a HProc, structs: &'a Structs, classes: &'a Structs, ifaces: &'a Ifaces, pools: &'a mut Pools) -> R<Self> {
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
        Ok(ProcGen { p, structs, classes, ifaces, pools, slots, names, code: Vec::new(), loops: Vec::new(), param_slots })
    }

    fn width(&self, ty: Type) -> usize {
        width(self.structs, ty)
    }

    /// How many values an expression statement leaves to pop. Every proc
    /// call returns something (a dummy 0 for procs without `-> T`); built-ins
    /// without a result and prints push nothing.
    fn leftover(&self, e: &HExpr) -> usize {
        match &e.kind {
            HExprKind::Builtin(..) => self.width(e.ty),
            HExprKind::Print(_) => 0,
            HExprKind::Bind(_, _, body) => self.leftover(body),
            HExprKind::Call(..) | HExprKind::DynCall { .. } if e.ty == Type::Void => 1,
            _ => self.width(e.ty),
        }
    }

    /// "No value" for a type: NaT in every slot (a real NaT for reals).
    fn push_nat(&mut self, ty: Type) {
        match ty {
            Type::Scalar(Scalar::Real) => self.push_real(Real::NAT),
            Type::Iface(_) => {
                self.push_word(Word::NAT);
                self.push_word(Word::NAT);
            }
            Type::Struct(n) => {
                for (_, t) in self.structs[n].clone() {
                    self.push_nat(t);
                }
            }
            _ => self.push_word(Word::NAT),
        }
    }

    /// The zero value of a type, as `claim` fills new arrays.
    fn push_zero(&mut self, ty: Type) {
        match ty {
            Type::Scalar(Scalar::Real) => self.push_real(Real::ZERO),
            Type::Bool => self.push_word(Word::NEG_ONE),
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

    /// `w` scratch slots, for values the generated code keeps for a moment.
    fn scratch(&mut self, name: &str, w: usize) -> Slot {
        let s = self.names.len() as Slot;
        for _ in 0..w {
            self.names.push(name.into());
        }
        s
    }

    /// Frees what `slot` owns. An object first frees what its fields own
    /// (each read into a scratch slot), an array of objects each element.
    fn burn(&mut self, slot: Slot, ty: Type) {
        match ty {
            Type::Class(c) => {
                let mut off = 0;
                for (_, t) in self.classes[c].clone() {
                    let w = self.width(t);
                    if owns(t) {
                        let tmp = self.scratch("$burn", w);
                        self.emit(Op::Load(slot));
                        self.push_word(Word::ZERO);
                        self.emit(Op::LdElem { off: off as u8, n: w as u8 });
                        self.store(tmp, w);
                        self.burn(tmp, t);
                    }
                    off += w;
                }
            }
            Type::Iface(i) => {
                // Which class it is decides what its fields own. Classes whose
                // fields own nothing just need the purge at the end.
                //   load v.class; push k; cmp3; test ==; br3 next, next, this
                //   this: <burn v as K>; jmp end
                //   next: ...; purge v
                let classes = self.classes;
                let deep: Vec<(i64, &'static str)> =
                    self.ifaces[i].iter().filter(|(_, c)| classes[c].iter().any(|(_, t)| owns(*t))).copied().collect();
                let mut exits = Vec::new();
                for (tag, c) in deep {
                    self.emit(Op::Load(slot + 1));
                    self.push_word(Word::new(tag));
                    self.emit(Op::Cmp3);
                    self.emit(Op::Test(0b010));
                    let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                    let this = self.here();
                    self.burn(slot, Type::Class(c));
                    exits.push(self.emit(Op::Jmp(0)));
                    let next = self.here();
                    self.patch(br, 0, next);
                    self.patch(br, 1, next);
                    self.patch(br, 2, this);
                }
                self.emit(Op::Purge(slot));
                let end = self.here();
                for at in exits {
                    self.patch(at, 0, end);
                }
                return;
            }
            Type::Own(b) if owns(b.ty()) => {
                // An array of objects: pop and burn each one until it is empty.
                //   top: load v; tlen; br3 end, end, body
                //   body: load v; popel; store $burn; <burn $burn>; jmp top
                let elem = b.ty();
                let w = self.width(elem);
                let tmp = self.scratch("$burn", w);
                let top = self.here();
                self.emit(Op::Load(slot));
                self.emit(Op::TLen);
                let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                let body = self.here();
                self.emit(Op::Load(slot));
                self.emit(Op::PopEl);
                self.store(tmp, w);
                self.burn(tmp, elem);
                self.emit(Op::Jmp(top));
                let end = self.here();
                self.patch(br, 0, end);
                self.patch(br, 1, end);
                self.patch(br, 2, body);
            }
            _ => {}
        }
        self.emit(Op::Purge(slot));
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
            HStmtKind::Declare(local) => {
                // No value yet: every slot holds NaT (set again on each pass of a loop).
                let ty = self.p.locals[*local].ty;
                self.push_nat(ty);
                self.store(self.slot(*local), self.width(ty));
            }
            HStmtKind::SetElem { arr, index, field, value } => {
                self.expr(arr)?;
                self.expr(index)?;
                self.expr(value)?;
                let n = self.width(value.ty);
                if *field == 0 && n == 1 {
                    self.emit(Op::StIdx);
                } else {
                    self.emit(Op::StElem { off: *field as u8, n: n as u8 });
                }
            }
            HStmtKind::Burn(id) => self.burn(self.slot(*id), self.p.locals[*id].ty),
            HStmtKind::Return(v) => {
                match v {
                    Some(e) => self.expr(e)?,
                    None => self.push_word(Word::ZERO),
                }
                match self.p.ret {
                    Type::Struct(_) | Type::Iface(_) => self.emit(Op::RetN(self.width(self.p.ret) as u16)),
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
            HExprKind::Local(id) | HExprKind::Lend(id) => self.load(self.slot(*id), self.width(e.ty)),
            HExprKind::ToIface(obj, tag) => {
                self.expr(obj)?;
                self.push_word(Word::new(*tag));
            }
            HExprKind::DynCall { impls, args } => {
                // The receiver goes to scratch slots (pointer, class); then
                // self, args; for each class: tag == k ? call K.m : next
                self.expr(&args[0])?;
                let s = self.scratch("$recv", 2);
                self.store(s, 2);
                self.emit(Op::Load(s));
                for a in &args[1..] {
                    self.expr(a)?;
                }
                let argc = 1 + args[1..].iter().map(|a| self.width(a.ty)).sum::<usize>();
                let mut exits = Vec::new();
                for (tag, pid) in impls {
                    self.emit(Op::Load(s + 1));
                    self.push_word(Word::new(*tag));
                    self.emit(Op::Cmp3);
                    self.emit(Op::Test(0b010));
                    let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                    let call = self.here();
                    self.patch(br, 2, call);
                    self.emit(Op::Call { f: *pid as u32, argc: argc as u8 });
                    exits.push(self.emit(Op::Jmp(0)));
                    let next = self.here();
                    self.patch(br, 0, next);
                    self.patch(br, 1, next);
                }
                let k = self.pools.string("interface call on an object of an unknown class");
                self.emit(Op::Trap(k));
                let end = self.here();
                for at in exits {
                    self.patch(at, 0, end);
                }
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
                // The pointer moves; an interface value also carries its class number.
                let s = self.slot(*id);
                self.emit(Op::Move(s));
                if let Type::Iface(_) = e.ty {
                    self.emit(Op::Load(s + 1));
                }
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
                    (_, Type::Bool) => {
                        self.emit(Op::ToBool);
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
                    self.expr(base)?;
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
                    if *fmt == Fmt::Quoted {
                        // text; jnat NAT; "\""; print; print; "\""; print; jmp END; NAT: "NaT"; print; END:
                        self.expr(p)?;
                        let jnat = self.emit(Op::JNat(0));
                        let q = self.pools.string("\"");
                        self.emit(Op::PushS(q));
                        self.emit(Op::Print);
                        self.emit(Op::Print);
                        self.emit(Op::PushS(q));
                        self.emit(Op::Print);
                        let jmp = self.emit(Op::Jmp(0));
                        let nat = self.here();
                        self.patch(jnat, 0, nat);
                        let k = self.pools.string("NaT");
                        self.emit(Op::PushS(k));
                        self.emit(Op::Print);
                        let end = self.here();
                        self.patch(jmp, 0, end);
                        continue;
                    }
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
                        Fmt::Quoted => unreachable!("handled above"),
                    });
                }
            }
            HExprKind::Builtin(b @ (Builtin::Append | Builtin::Insert | Builtin::Resize), args) => {
                // The element (or the zero to grow with) goes below the pointer.
                match b {
                    Builtin::Resize => self.push_zero(elem_ty(args[0].ty)),
                    Builtin::Append => self.expr(&args[1])?,
                    _ => self.expr(&args[2])?,
                }
                self.expr(&args[0])?;
                if *b != Builtin::Append {
                    self.expr(&args[1])?;
                }
                self.emit(match b {
                    Builtin::Append => Op::Append,
                    Builtin::Insert => Op::Insert,
                    _ => Op::Resize,
                });
            }
            HExprKind::Builtin(Builtin::ToText(Fmt::Plain), args) if args[0].ty == Type::Bool => {
                // `true` / `false`: branch on the value and push the right word
                // (`NaT` for a bool without a value).
                self.expr(&args[0])?;
                let jnat = self.emit(Op::JNat(0));
                let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                let mut exits = Vec::new();
                for (arm, word) in [(0, Some("false")), (1, None), (2, Some("true"))] {
                    let at = self.here();
                    self.patch(br, arm, at);
                    match word {
                        Some(w) => {
                            let k = self.pools.string(w);
                            self.emit(Op::PushS(k));
                            exits.push(self.emit(Op::Jmp(0)));
                        }
                        None => {
                            let k = self.pools.string("a bool holds 0");
                            self.emit(Op::Trap(k));
                        }
                    }
                }
                let nat = self.here();
                self.patch(jnat, 0, nat);
                let k = self.pools.string("NaT");
                self.emit(Op::PushS(k));
                let end = self.here();
                for at in exits {
                    self.patch(at, 0, end);
                }
            }
            HExprKind::Builtin(b, args) => {
                for a in args {
                    self.expr(a)?;
                }
                self.emit(match b {
                    Builtin::Pop => Op::PopEl,
                    Builtin::IsNaT => Op::IsNaT,
                    Builtin::Remove => Op::Remove,
                    Builtin::Clear => Op::Clear,
                    Builtin::Replace => Op::Replace,
                    Builtin::Append | Builtin::Insert | Builtin::Resize => unreachable!("handled above"),
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
            HExprKind::NewObj(values) => {
                for v in values {
                    self.expr(v)?;
                }
                match values.iter().map(|v| self.width(v.ty)).sum::<usize>() {
                    1 => self.emit(Op::Pack { n: 1, sym: false }),
                    w => self.emit(Op::PackT { n: 1, w: w as u8, sym: false }),
                };
            }
            HExprKind::ElemField(arr, idx, off) => {
                self.expr(arr)?;
                self.expr(idx)?;
                self.emit(Op::LdElem { off: *off as u8, n: self.width(e.ty) as u8 });
            }
        }
        Ok(())
    }
}

/// Whether a value of type `t` owns something that `burn` frees.
fn owns(t: Type) -> bool {
    matches!(t, Type::Own(_) | Type::Class(_) | Type::Iface(_))
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
        Type::LentIface(_) | Type::Iface(_) => {
            out.push(name.to_string());
            out.push(format!("{name}.class"));
        }
        _ => out.push(name.to_string()),
    }
}
