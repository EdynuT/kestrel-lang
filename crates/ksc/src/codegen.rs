//! Code generator: checked [`Hir`] -> `.ksb` [`Module`].
//!
//! The VM is a stack machine, so expressions compile to postfix order.
//! Locals become frame slots (slot 0..8 is shown as R-4..R+4): parameters
//! first, then everything else.

use std::collections::HashMap;

use kestrel_core::isa::{Addr, Op, Slot};
use kestrel_core::limits::{MAX_LOCALS, MAX_PARAMS};
use kestrel_core::ksb::{Function, Handler, Impl, Kind, Module};
use kestrel_core::{Real, Word, Word27};

use crate::diag::{Diagnostic, Span};
use crate::hir::*;

type R<T> = Result<T, Diagnostic>;

/// Compiles one file: an object for the linker (see `kestrel_core::link`).
/// Procs of other files are imports; interface calls and frees go to
/// functions the linker writes.
pub fn generate(hir: &Hir) -> R<Module> {
    let mut pools = Pools::default();
    let mut functions = Vec::new();
    let link_of: HashMap<&'static str, &ClassLink> = hir.class_info.iter().map(|c| (c.name, c)).collect();
    // A class whose fields own something gets a hidden function that frees
    // an object of it, after the user's procs. `burn` calls it, so a class
    // that holds objects of its own class (a tree) frees them by recursion.
    let mut owners: Vec<&'static str> =
        hir.classes.iter().filter(|(_, fields)| fields.iter().any(|(_, t)| owns(*t))).map(|(c, _)| *c).collect();
    owners.sort_unstable();
    let burners: HashMap<&'static str, u32> = owners.iter().enumerate().map(|(k, c)| (*c, (hir.procs.len() + k) as u32)).collect();
    // Then one `$burnI.I` per interface, written by the linker.
    let first = (hir.procs.len() + owners.len()) as u32;
    let burn_ifaces: HashMap<&'static str, u32> = hir.iface_names.iter().enumerate().map(|(k, i)| (*i, first + k as u32)).collect();
    let burn_procs: Vec<(&'static str, HProc)> = owners
        .iter()
        .map(|c| {
            let obj = Local { name: "obj".into(), ty: Type::Class(c), kind: LocalKind::Param, span: Span::default(), is_final: false };
            let body = HBlock { stmts: Vec::new(), scope: Vec::new(), span: Span::default() };
            let link = link_of.get(c).map_or(Link::Own, |l| l.link);
            (*c, HProc { name: format!("$burn.{c}"), params: 1, ret: Type::Void, locals: vec![obj], body, refs: vec![false], link, throws: Vec::new() })
        })
        .collect();
    // Which parameters of each proc are `x: &int` (written back after a call).
    let refs: Vec<Vec<bool>> = hir.procs.iter().map(|p| p.refs.clone()).collect();
    let cx = Shared { structs: &hir.structs, classes: &hir.classes, burners: &burners, burn_ifaces: &burn_ifaces, refs: &refs };
    for p in &hir.procs {
        functions.push(match p.link {
            Link::Own | Link::Shared => {
                let mut f = ProcGen::new(p, &cx, &mut pools)?.run()?;
                f.kind = if p.link == Link::Shared { Kind::Shared } else { Kind::Code };
                f
            }
            Link::Import => stub(&hir.structs, p, Kind::Import),
            Link::Dispatch => {
                let ret = match p.ret {
                    Type::Void => 1,
                    t => width(&hir.structs, t) as u16,
                };
                stub(&hir.structs, p, Kind::Dispatch { ret })
            }
        });
    }
    for (c, hp) in &burn_procs {
        functions.push(match hp.link {
            Link::Import => stub(&hir.structs, hp, Kind::Import),
            link => {
                let mut f = ProcGen::new(hp, &cx, &mut pools)?.burn_fn(c);
                f.kind = if link == Link::Shared { Kind::Shared } else { Kind::Code };
                f
            }
        });
    }
    for i in &hir.iface_names {
        let names = vec!["$obj".to_string(), "$class".to_string()];
        functions.push(Function { name: format!("$burnI.{i}"), params: 2, locals: 2, code: Vec::new(), local_names: names, kind: Kind::BurnDispatch, handlers: Vec::new() });
    }
    // What the linker needs: this file's classes and how they answer.
    let mut link = kestrel_core::ksb::Link::default();
    for c in hir.class_info.iter().filter(|c| c.link != Link::Import) {
        link.classes.push((c.name.to_string(), c.tag));
        if burners.contains_key(c.name) {
            for i in &c.ifaces {
                link.burns.push((format!("$burnI.{i}"), c.tag, format!("$burn.{}", c.name)));
            }
        }
    }
    for (dispatch, tag, pid, whole) in &hir.impls {
        link.impls.push(Impl { dispatch: dispatch.clone(), tag: *tag, function: hir.procs[*pid].name.clone(), whole: *whole });
    }
    let m = Module { entry: hir.entry as u32, consts: pools.consts, strings: pools.strings, functions, link: Some(link) };
    debug_assert_eq!(m.verify(), Ok(()));
    Ok(m)
}

/// A function without code (written elsewhere): its parameters only.
fn stub(structs: &Structs, p: &HProc, kind: Kind) -> Function {
    let mut names = Vec::new();
    for l in &p.locals[..p.params] {
        leaf_names(structs, l.ty, &l.name, &mut names);
    }
    let n = names.len();
    Function { name: p.name.clone(), params: n as u8, locals: n as u16, code: Vec::new(), local_names: names, kind, handlers: Vec::new() }
}

/// What every proc's code generation reads.
struct Shared<'a> {
    structs: &'a Structs,
    classes: &'a Structs,
    /// The hidden function that frees an object of each class that owns something.
    burners: &'a HashMap<&'static str, u32>,
    /// `$burnI.I` for each interface.
    burn_ifaces: &'a HashMap<&'static str, u32>,
    /// `x: &int` parameters of every proc, by proc number.
    refs: &'a [Vec<bool>],
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
    /// The hidden function that frees an object of each class that owns something.
    burners: &'a HashMap<&'static str, u32>,
    burn_ifaces: &'a HashMap<&'static str, u32>,
    /// `x: &int` parameters of every proc, by proc number.
    refs: &'a [Vec<bool>],
    pools: &'a mut Pools,
    slots: Vec<Slot>,
    names: Vec<String>,
    code: Vec<u8>,
    loops: Vec<LoopLabels>,
    /// Slots taken by the parameters (a struct parameter takes several).
    param_slots: usize,
    /// Where errors go (see `ksb::Handler`), innermost first.
    handlers: Vec<Handler>,
}

impl<'a> ProcGen<'a> {
    fn new(p: &'a HProc, cx: &Shared<'a>, pools: &'a mut Pools) -> R<Self> {
        let Shared { structs, classes, burners, burn_ifaces, refs } = *cx;
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
        Ok(ProcGen { p, structs, classes, burners, burn_ifaces, refs, pools, slots, names, code: Vec::new(), loops: Vec::new(), param_slots, handlers: Vec::new() })
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
            HExprKind::Call(..) | HExprKind::IfNull { .. } if e.ty == Type::Void => 1,
            _ => self.width(e.ty),
        }
    }

    /// "No value" for a type: NaT in every slot (a real NaT for reals).
    fn push_nat(&mut self, ty: Type) {
        match ty {
            Type::Scalar(Scalar::Real) | Type::OptScalar(Scalar::Real) => self.push_real(Real::NAT),
            Type::Iface(_) | Type::OptIface(_) | Type::LentOptIface(_) => {
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

    /// Frees what `slot` owns. An object of a class whose fields own
    /// something goes to that class's hidden burn function; an array of
    /// objects frees each element first.
    fn burn(&mut self, slot: Slot, ty: Type) {
        match ty {
            Type::OptClass(_) | Type::OptIface(_) => {
                // Empty: nothing to free.  load v; jnat end; pop; <burn v>; end:
                self.emit(Op::Load(slot));
                let skip = self.emit(Op::JNat(0));
                self.emit(Op::Pop);
                self.burn(slot, ty.unopt());
                let end = self.here();
                self.patch(skip, 0, end);
                return;
            }
            Type::Class(c) => {
                if let Some(&f) = self.burners.get(c) {
                    self.call_burner(slot, f);
                    return;
                }
            }
            Type::Iface(i) => {
                // Which class it is decides what its fields own: `$burnI.I`
                // (written by the linker) knows every class.
                //   move v; load v.class; call $burnI.I; pop
                self.emit(Op::Move(slot));
                self.emit(Op::Load(slot + 1));
                self.emit(Op::Call { f: self.burn_ifaces[i], argc: 2 });
                self.emit(Op::Pop);
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

    /// Returns the value on the stack (one dummy word for a proc without
    /// `-> T`) and, above it, the final value of each `x: &int` parameter.
    fn ret(&mut self) {
        let mut n = match self.p.ret {
            Type::Void => 1,
            t => self.width(t),
        };
        for (k, by_ref) in self.p.refs.clone().into_iter().enumerate() {
            if by_ref {
                let w = self.width(self.p.locals[k].ty);
                self.load(self.slot(k), w);
                n += w;
            }
        }
        if n == 1 {
            self.emit(Op::Ret);
        } else {
            self.emit(Op::RetN(n as u16));
        }
    }

    /// Stores the value on top of the stack into what `target` reads (a
    /// local, a struct field, an object field or an array element).
    fn store_back(&mut self, target: &HExpr) -> R<()> {
        let w = self.width(target.ty);
        match &target.kind {
            HExprKind::Local(id) => self.store(self.slot(*id), w),
            HExprKind::Field(id, off) => self.store(self.slot(*id) + *off as Slot, w),
            HExprKind::ElemField(arr, idx, off) => {
                let tmp = self.scratch("$back", w);
                self.store(tmp, w);
                self.expr(arr)?;
                self.expr(idx)?;
                self.load(tmp, w);
                self.emit(Op::StElem { off: *off as u8, n: w as u8 });
            }
            HExprKind::Index(arr, idx) => {
                let tmp = self.scratch("$back", w);
                self.store(tmp, w);
                self.expr(arr)?;
                self.expr(idx)?;
                self.load(tmp, w);
                if w == 1 {
                    self.emit(Op::StIdx);
                } else {
                    self.emit(Op::StElem { off: 0, n: w as u8 });
                }
            }
            _ => unreachable!("the checker allows only these with `&`"),
        }
        Ok(())
    }

    /// `move slot; call f; pop`: the burn function frees the object (and
    /// purges it); the slot is left NaT, like after a purge.
    fn call_burner(&mut self, slot: Slot, f: u32) {
        self.emit(Op::Move(slot));
        self.emit(Op::Call { f, argc: 1 });
        self.emit(Op::Pop);
    }

    /// The body of `$burn.C`: reads what each field of the object (slot 0)
    /// owns into scratch slots, frees the object, then frees those. A last
    /// field of the same class (`proximo: No?`) is freed by looping instead
    /// of calling itself, so a long list does not overflow the call stack:
    ///   top: <read fields>; purge 0; <burn the others>;
    ///        load next; jnat END; pop; load next; store 0; jmp top
    ///   END: push 0; ret
    fn burn_fn(mut self, c: &'static str) -> Function {
        let mut owned = Vec::new();
        let mut off = 0;
        for (_, t) in self.classes[c].clone() {
            let w = self.width(t);
            if owns(t) {
                owned.push((off, t, w));
            }
            off += w;
        }
        let tail = match owned.last() {
            Some(&(_, t @ (Type::Class(d) | Type::OptClass(d)), _)) if d == c => Some(t),
            _ => None,
        };
        let top = self.here();
        let mut tmps = Vec::new();
        for &(off, t, w) in &owned {
            let tmp = self.scratch("$burn", w);
            self.emit(Op::Load(0));
            self.push_word(Word::ZERO);
            self.emit(Op::LdElem { off: off as u8, n: w as u8 });
            self.store(tmp, w);
            tmps.push((tmp, t));
        }
        self.emit(Op::Purge(0));
        let keep = tmps.len() - usize::from(tail.is_some());
        for &(tmp, t) in &tmps[..keep] {
            self.burn(tmp, t);
        }
        let mut done = None;
        if let Some(t) = tail {
            let (next, _) = tmps[keep];
            if t.is_opt() {
                self.emit(Op::Load(next));
                done = Some(self.emit(Op::JNat(0)));
                self.emit(Op::Pop);
            }
            self.emit(Op::Load(next));
            self.emit(Op::Store(0));
            self.emit(Op::Jmp(top));
        }
        if let Some(at) = done {
            let end = self.here();
            self.patch(at, 0, end);
        }
        self.push_word(Word::ZERO);
        self.emit(Op::Ret);
        Function { name: self.p.name.clone(), params: 1, locals: self.names.len() as u16, code: self.code, local_names: self.names, kind: Kind::Code, handlers: Vec::new() }
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
        // An error that leaves the proc frees what its variables still own
        // (a moved or burned variable holds NaT), so each starts empty.
        let owned: Vec<LocalId> = (0..self.p.locals.len()).filter(|&i| self.owned(i)).collect();
        for &i in owned.iter().filter(|&&i| i >= self.p.params) {
            let ty = self.p.locals[i].ty;
            self.push_nat(ty);
            self.store(self.slot(i), self.width(ty));
        }
        let start = self.here();
        self.block(&self.p.body)?;
        if !self.p.body.diverges() {
            // A proc without `-> T` that falls off its end returns nothing.
            self.push_word(Word::ZERO);
            self.ret();
        }
        if !owned.is_empty() {
            // The proc's own handler: frees its variables and passes the error on.
            let (end, target) = (self.here(), self.here());
            for &i in &owned {
                self.burn_alive(i);
            }
            self.emit(Op::Throw);
            self.handlers.push(Handler { start, end, target });
        }
        Ok(Function {
            name: self.p.name.clone(),
            params: self.param_slots as u8,
            locals: self.names.len() as u16,
            code: self.code,
            local_names: self.names,
            kind: Kind::Code,
            handlers: self.handlers,
        })
    }

    /// A variable that may own something when an error leaves its block.
    fn owned(&self, id: LocalId) -> bool {
        let l = &self.p.locals[id];
        matches!(l.kind, LocalKind::Param | LocalKind::Let) && owns(l.ty)
    }

    /// Frees what variable `id` owns, if it still owns something.
    ///   load v; jnat SKIP; pop; <burn v>; SKIP:
    fn burn_alive(&mut self, id: LocalId) {
        let (slot, ty) = (self.slot(id), self.p.locals[id].ty);
        if ty.is_opt() {
            self.burn(slot, ty); // checks for empty itself
            return;
        }
        self.emit(Op::Load(slot));
        let skip = self.emit(Op::JNat(0));
        self.emit(Op::Pop);
        self.burn(slot, ty);
        let end = self.here();
        self.patch(skip, 0, end);
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
                self.ret();
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
            HStmtKind::Throw { value, .. } => {
                self.expr(value)?;
                self.emit(Op::Throw);
            }
            HStmtKind::Try { body, catches } => self.try_catch(body, catches)?,
        }
        Ok(())
    }

    /// ```text
    ///         <body>; jmp END
    /// HANDLER: (error, class) store $error; <free what the body changed>
    ///         load $error.class; push C; cmp3; test =; br3 NEXT, NEXT, THIS
    ///   THIS: load $error; store e; <catch body>; jmp END
    ///   NEXT: ... (`catch e: Error` takes any)
    ///         load $error; throw          (no catch took it: pass it on)
    /// END:
    /// ```
    fn try_catch(&mut self, body: &HBlock, catches: &[HCatch]) -> R<()> {
        let start = self.here();
        self.block(body)?;
        let end = self.here();
        let mut exits = vec![self.emit(Op::Jmp(0))];
        let target = self.here();
        self.handlers.push(Handler { start, end, target });
        let err = self.scratch("$error", 2);
        self.store(err, 2);
        let mut changed = Vec::new();
        changed_in(body, &mut changed);
        for id in changed {
            if self.owned(id) {
                self.burn_alive(id);
            }
        }
        for c in catches {
            let s = self.slot(c.local);
            let test = match c.class {
                Some((_, tag)) => {
                    self.emit(Op::Load(err + 1));
                    self.push_word(Word::new(tag));
                    self.emit(Op::Cmp3);
                    self.emit(Op::Test(0b010));
                    let br = self.emit(Op::Br3 { neg: 0, zero: 0, pos: 0 });
                    let this = self.here();
                    self.patch(br, 2, this);
                    self.emit(Op::Load(err));
                    self.emit(Op::Store(s));
                    Some(br)
                }
                None => {
                    self.load(err, 2);
                    self.store(s, 2);
                    None
                }
            };
            self.block(&c.body)?;
            if !c.body.diverges() {
                exits.push(self.emit(Op::Jmp(0)));
            }
            match test {
                Some(br) => {
                    let next = self.here();
                    self.patch(br, 0, next);
                    self.patch(br, 1, next);
                }
                None => break,
            }
        }
        self.load(err, 2);
        self.emit(Op::Throw);
        let after = self.here();
        for j in exits {
            self.patch(j, 0, after);
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
                if let Type::Iface(_) | Type::OptIface(_) = e.ty {
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
                // `x: &int` arguments come back above the result, last on top:
                // each is stored into what was passed (`&n`, `&p.x`, `&v[i]`).
                let refs = self.refs.get(*pid).cloned().unwrap_or_default();
                for (a, _) in args.iter().zip(&refs).rev().filter(|(_, r)| **r) {
                    self.store_back(a)?;
                }
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
                // `isNaT` of an interface value tests its pointer, not its class number.
                if *b == Builtin::IsNaT && self.width(args[0].ty) == 2 {
                    self.emit(Op::Pop);
                }
                self.emit(match b {
                    Builtin::Pop => Op::PopEl,
                    Builtin::IsNaT => Op::IsNaT,
                    Builtin::Hash => Op::Hash,
                    Builtin::TCmp => Op::TCmp,
                    Builtin::HashMix => Op::HashMix,
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
            HExprKind::IfNull { value, tmp, then, null } => {
                //   value; store tmp; load tmp; jnat NULL; pop; then; jmp END
                //   NULL: null (or NaT; a dummy 0 for a call without result)   END:
                let s = self.slot(*tmp);
                self.expr(value)?;
                self.store(s, self.width(value.ty));
                self.emit(Op::Load(s));
                let jnat = self.emit(Op::JNat(0));
                self.emit(Op::Pop);
                self.expr(then)?;
                let jmp = self.emit(Op::Jmp(0));
                let at_null = self.here();
                self.patch(jnat, 0, at_null);
                match null {
                    Some(n) => self.expr(n)?,
                    None if e.ty == Type::Void => self.push_word(Word::ZERO),
                    None => self.push_nat(e.ty),
                }
                let end = self.here();
                self.patch(jmp, 0, end);
            }
            HExprKind::IsClass(value, tag, _) => {
                // Keep the class number, drop the pointer; empty (NaT) is false:
                // value; store $is; pop; load $is; jnat NO; push k; cmp3; test ==; jmp END
                // NO: push false   END:
                let s = self.scratch("$is", 1);
                self.expr(value)?;
                self.emit(Op::Store(s));
                self.emit(Op::Pop);
                self.emit(Op::Load(s));
                let jnat = self.emit(Op::JNat(0));
                self.push_word(Word::new(*tag));
                self.emit(Op::Cmp3);
                self.emit(Op::Test(0b010));
                let jmp = self.emit(Op::Jmp(0));
                let no = self.here();
                self.patch(jnat, 0, no);
                self.push_word(Word::NEG_ONE);
                let end = self.here();
                self.patch(jmp, 0, end);
            }
            HExprKind::TakeField(obj, off) => {
                // Read the field, then leave it empty:
                // obj; 0; ldelem off; obj; 0; NaT..; stelem off
                let w = self.width(e.ty);
                let zero = || HExpr { kind: HExprKind::Word(Word::ZERO), ty: Type::Scalar(Scalar::Int), span: e.span };
                self.expr(obj)?;
                self.expr(&zero())?;
                self.emit(Op::LdElem { off: *off as u8, n: w as u8 });
                self.expr(obj)?;
                self.expr(&zero())?;
                self.push_nat(e.ty);
                self.emit(Op::StElem { off: *off as u8, n: w as u8 });
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
    matches!(t, Type::Own(_) | Type::Class(_) | Type::Iface(_) | Type::OptClass(_) | Type::OptIface(_))
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
        Type::LentIface(_) | Type::Iface(_) | Type::OptIface(_) | Type::LentOptIface(_) => {
            out.push(name.to_string());
            out.push(format!("{name}.class"));
        }
        _ => out.push(name.to_string()),
    }
}
