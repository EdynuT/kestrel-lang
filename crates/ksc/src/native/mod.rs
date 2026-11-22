//! The native backend: a linked program (`.ksb`) -> an x86-64 ELF
//! executable, written here with no assembler or linker.
//!
//! The executable is a copy of the runtime (`ksvm`, which has the heap,
//! texts, real numbers and printing) with one more segment: the program's
//! machine code, its `.ksb` and a table of addresses ([`elf`]). Each
//! instruction of the `.ksb` becomes machine code ([`x86`]).
//!
//! The VM's stack and variables are used as they are, in memory: `rbx`
//! points just past the top of the stack and `r12` at the variables of the
//! running function (each a `ksvm::Value`, 24 bytes: its kind, then its
//! value). Jumps and calls are machine jumps and calls. Moving values,
//! `int` arithmetic, comparisons and branches are done right here when the
//! values are plain `int`s (the common case); anything else (NaT, an
//! overflow, a real, a text, any other instruction) calls the runtime
//! (`krt_op` in `ksvm::native`), which does what the interpreter does,
//! messages included. When an instruction fails or throws, `krt_unwind`
//! finds the handler and says where to go.
//!
//! Every native function keeps the machine stack aligned to 16 bytes in
//! its body, as calls into Rust require:
//! ```text
//! F:    push r12; frame.sp = rsp; r12 = its variables     (a call writes the frame itself)
//! op:   <fast code>, or (and as its slow path):
//!       mov [KRT_SP], rbx; mov edi, func; mov esi, op; call krt_op; mov rbx, [KRT_SP]
//!       cmp rax, -1; je FAIL
//! FAIL: call krt_unwind; mov rsp, rdx; jmp rax        (to a LANDING, here or in a caller)
//! LANDING: call krt_locals; mov r12, rax; mov rbx, [KRT_SP]; jmp HANDLER
//! ```

pub mod elf;
pub mod x86;

use std::collections::HashMap;

use kestrel_core::isa::Op;
use kestrel_core::word::WORD_MAX;
use kestrel_core::{Module, Word};

use elf::Elf;
use x86::{Alu, Asm, Cond, Label, Reg};

/// `ksvm::native::PRESENT`
const PRESENT: u64 = u64::from_le_bytes(*b"KRT-PROG");
/// `ksvm::native::ABI`: the runtime this backend is written for.
const ABI: u64 = 5;
/// The size of a `ksvm::Value`; its value starts at byte 8.
const V: i32 = 24;
/// The size of a frame (`ksvm`'s `Frame`), and where its fields are.
const FRAME: i32 = 48;
const F_FUNC: i32 = 0;
const F_PC: i32 = 8;
const F_LOCALS: i32 = 16;
const F_BASE: i32 = 24;
const F_SP: i32 = 32;
const F_SOFT: i32 = 40;
/// The size of a heap block (`ksvm`'s `Block`), and where its fields are.
const BLOCK: i32 = 64;
const B_AT: i32 = 0;
const B_LEN: i32 = 8;
const B_W: i32 = 16;
const B_LO: i32 = 24;
const B_GEN: i32 = 32;
const B_LIVE: i32 = 36;
/// The kind number of a pointer (`Value::Ptr`).
const PTR: i32 = 3;
/// A call writes the zeros of at most this many variables itself.
const MAX_ZEROED: usize = 32;

/// The runtime's functions and variables the code uses.
struct Runtime {
    grow: u64,
    op: u64,
    unwind: u64,
    locals: u64,
    sp: u64,
    limit: u64,
    frames: u64,
    nframes: u64,
    locals_at: u64,
    nlocals: u64,
    stack: u64,
    heap: u64,
    nheap: u64,
    /// The runtime's limits: frames, and variables of all frames.
    max_depth: i32,
    max_locals: i32,
}

/// The executable for `module` (a linked program), made from `runtime`
/// (the bytes of a `ksvm` executable).
pub fn build(module: &Module, runtime: &[u8]) -> Result<Vec<u8>, String> {
    let elf = Elf::read(runtime)?;
    let sym = |n: &str| elf.symbol(n).ok_or_else(|| format!("the runtime has no `{n}` (is it a `ksvm` of this version, not stripped?)"));
    let abi = elf.file_offset(sym("KRT_ABI")?).and_then(|at| runtime.get(at..at + 48)).ok_or("the runtime's `KRT_ABI` is not stored")?;
    let abi: Vec<u64> = abi.chunks(8).map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes"))).collect();
    if abi[..3] != [ABI, V as u64, FRAME as u64] || abi[5] != BLOCK as u64 {
        return Err("the runtime (`ksvm`) is not from this version of `ksc`".into());
    }
    let program = sym("KRT_PROGRAM")?;
    let rt = Runtime {
        grow: sym("krt_grow")?,
        op: sym("krt_op")?,
        unwind: sym("krt_unwind")?,
        locals: sym("krt_locals")?,
        sp: sym("KRT_SP")?,
        limit: sym("KRT_LIMIT")?,
        frames: sym("KRT_FRAMES")?,
        nframes: sym("KRT_NFRAMES")?,
        locals_at: sym("KRT_LOCALS")?,
        nlocals: sym("KRT_NLOCALS")?,
        stack: sym("KRT_STACK")?,
        heap: sym("KRT_HEAP")?,
        nheap: sym("KRT_NHEAP")?,
        max_depth: i32::try_from(abi[3]).map_err(|e| e.to_string())?,
        max_locals: i32::try_from(abi[4]).map_err(|e| e.to_string())?,
    };
    let base = elf.free_vaddr();
    let mut a = Asm::new(base);
    let entries: Vec<Label> = module.functions.iter().map(|_| a.label()).collect();
    let mut landings = Vec::new();
    // The runtime starts at `main` here: `rbx` from where it keeps the stack's top.
    let start = a.label();
    a.bind(start);
    a.load_global(Reg::Rbx, rt.sp);
    a.jmp(entries[module.entry as usize]);
    for k in 0..module.functions.len() {
        landings.push(Gen { a: &mut a, rt: &rt, module, k, entries: &entries, slow: Vec::new(), pending: Vec::new(), real: 0, after: 0 }.function()?);
    }
    // Addresses relative to `KRT_PROGRAM`, which is where the runtime finds them.
    let rel = |addr: u64| (addr as i64 - program as i64).to_le_bytes();
    let mut table = (module.functions.len() as u32).to_le_bytes().to_vec();
    for (k, l) in landings.iter().enumerate() {
        let entry = if k == module.entry as usize { start } else { entries[k] };
        table.extend_from_slice(&rel(a.addr(entry)));
        table.extend_from_slice(&(l.len() as u32).to_le_bytes());
        for &(pc, label) in l {
            table.extend_from_slice(&pc.to_le_bytes());
            table.extend_from_slice(&rel(a.addr(label)));
        }
    }
    let mut seg = a.finish()?;
    let ksb_at = base + seg.len() as u64;
    let ksb = module.to_bytes();
    seg.extend_from_slice(&ksb);
    let table_at = base + seg.len() as u64;
    seg.extend_from_slice(&table);
    let mut fields = PRESENT.to_le_bytes().to_vec();
    for v in [rel(base), rel(ksb_at), (ksb.len() as u64).to_le_bytes(), rel(table_at), (table.len() as u64).to_le_bytes()] {
        fields.extend_from_slice(&v);
    }
    let at = elf.file_offset(program).ok_or("`KRT_PROGRAM` is not stored in the runtime's file")?;
    Ok(elf.with_segment(base, seg, &[(at, fields)]))
}

/// A value the code keeps track of instead of pushing it on the VM's stack
/// yet: it is pushed (by [`Gen::flush`]) only when something needs it there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opnd {
    /// An `int` constant (never NaT).
    Const(i64),
    /// The value of variable `s` (any value: it is checked where it is used).
    Local(u16),
    /// A value in registers: its kind in `r8`, its value in `r9` (and `r10`).
    Tmp,
}

/// Where a value of any kind is, to be copied once an instruction knows it can go on.
enum Loc {
    Const(i64),
    /// Three 8-byte parts from `[base + disp]`.
    Mem(Reg, i32),
    Tmp,
}

/// The most values kept back at once.
const MAX_PENDING: usize = 6;

/// The code of one function.
struct Gen<'a> {
    a: &'a mut Asm,
    rt: &'a Runtime,
    module: &'a Module,
    k: usize,
    entries: &'a [Label],
    /// Instructions done here for plain `int`s, and their slow path (the
    /// runtime), written after the function: label, instruction number, the
    /// instruction, where to go on, the values kept back when it started.
    slow: Vec<(Label, usize, Op, Label, Vec<Opnd>)>,
    /// The values on top of the VM's stack that are not pushed yet (last on top).
    pending: Vec<Opnd>,
    /// Values the instruction being written took from the real stack.
    real: i32,
    /// The offset just past the instruction being written (where a call returns).
    after: usize,
}

impl Gen<'_> {
    /// Gives the handlers' landings: their offset in the `.ksb` code and label.
    fn function(mut self) -> Result<Vec<(u32, Label)>, String> {
        let f = &self.module.functions[self.k];
        let mut ops = Vec::new();
        let mut pc = 0;
        while pc < f.code.len() {
            let (op, size) = Op::decode(&f.code, pc).map_err(|e| e.to_string())?;
            ops.push((op, pc));
            pc += size;
        }
        // Where code jumps to: nothing may be kept back there (it can be
        // reached from more than one place).
        let mut targets: std::collections::HashSet<usize> = f.handlers.iter().map(|h| h.target as usize).collect();
        for &(op, _) in &ops {
            match op {
                Op::Jmp(t) | Op::JNat(t) => {
                    targets.insert(t as usize);
                }
                Op::Br3 { neg, zero, pos } => targets.extend([neg, zero, pos].map(|t| t as usize)),
                _ => {}
            }
        }
        let mut at: HashMap<usize, Label> = ops.iter().map(|&(_, pc)| (pc, self.a.label())).collect();
        let end = self.a.label();
        at.insert(f.code.len(), end);
        let fail = self.a.label();
        let a = &mut *self.a;
        // The frame was made by the caller: keep this code's stack pointer
        // in it, and find its variables.
        //   rax = the top frame; frame.sp = rsp; r12 = LOCALS + frame.locals * V
        a.bind(self.entries[self.k]);
        a.push(Reg::R12);
        a.load_global(Reg::Rax, self.rt.nframes);
        a.imul_imm(Reg::Rax, Reg::Rax, FRAME);
        a.alu_global(Alu::Add, Reg::Rax, self.rt.frames);
        a.store(Reg::Rax, F_SP - FRAME, Reg::Rsp);
        a.load(Reg::R12, Reg::Rax, F_LOCALS - FRAME);
        a.imul_imm(Reg::R12, Reg::R12, V);
        a.alu_global(Alu::Add, Reg::R12, self.rt.locals_at);
        for (idx, &(op, pc)) in ops.iter().enumerate() {
            if targets.contains(&pc) {
                self.flush();
            }
            self.a.bind(at[&pc]);
            // Where a slow path comes back: right after this instruction,
            // before anything kept back is pushed for the next one.
            let next = self.a.label();
            let to = |t: u32| at[&(t as usize)];
            self.real = 0;
            self.after = pc + op.size();
            if let Op::Jmp(t) = op {
                self.flush();
                self.a.jmp(to(t));
            } else if !self.fast(idx, op, next, &to) {
                self.flush();
                self.generic(idx, op, fail, &to);
            }
            self.a.bind(next);
        }
        // Falling off the end: the runtime reports it (there is no instruction `ops.len()`).
        self.flush();
        self.a.bind(end);
        self.generic(ops.len(), Op::Pop, fail, &|_| end);
        for (label, idx, op, next, kept) in std::mem::take(&mut self.slow) {
            self.a.bind(label);
            self.pending = kept;
            self.flush();
            self.generic(idx, op, fail, &|t| at[&(t as usize)]);
            let result = matches!(op, Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Cmp3 | Op::Neg | Op::Test(_) | Op::LdIdx | Op::TLen);
            if !result {
                if !matches!(op, Op::Br3 { .. }) {
                    self.a.jmp(next);
                }
            } else {
                // Its result goes where the fast code keeps it.
                let a = &mut *self.a;
                a.load(Reg::R8, Reg::Rbx, -V);
                a.load(Reg::R9, Reg::Rbx, -V + 8);
                a.load(Reg::R10, Reg::Rbx, -V + 16);
                a.alu_imm(Alu::Sub, Reg::Rbx, V);
                a.jmp(next);
            }
        }
        let a = &mut *self.a;
        a.bind(fail);
        a.call_addr(self.rt.unwind);
        a.mov(Reg::Rsp, Reg::Rdx);
        a.jmp_reg(Reg::Rax);
        let mut out = Vec::new();
        for h in &f.handlers {
            let land = a.label();
            a.bind(land);
            a.call_addr(self.rt.locals);
            a.mov(Reg::R12, Reg::Rax);
            a.load_global(Reg::Rbx, self.rt.sp);
            a.jmp(at[&(h.target as usize)]);
            out.push((h.target, land));
        }
        Ok(out)
    }

    /// Instruction number `idx`, run by the runtime (nothing may be kept back).
    fn generic(&mut self, idx: usize, op: Op, fail: Label, to: &dyn Fn(u32) -> Label) {
        debug_assert!(self.pending.is_empty());
        let (a, rt) = (&mut *self.a, self.rt);
        a.store_global(rt.sp, Reg::Rbx);
        a.mov_imm(Reg::Rdi, self.k as u32);
        a.mov_imm(Reg::Rsi, idx as u32);
        a.call_addr(rt.op);
        a.load_global(Reg::Rbx, rt.sp);
        a.alu_imm(Alu::Cmp, Reg::Rax, -1);
        a.jcc(Cond::E, fail);
        match op {
            Op::JNat(t) => {
                a.test(Reg::Rax);
                a.jcc(Cond::Ne, to(t));
            }
            Op::Br3 { neg, zero, pos } => {
                a.alu_imm(Alu::Cmp, Reg::Rax, 1);
                a.jcc(Cond::B, to(neg));
                a.jcc(Cond::E, to(zero));
                a.jmp(to(pos));
            }
            Op::Call { f, .. } => a.call(self.entries[f as usize]),
            Op::Ret | Op::RetN(_) => {
                a.pop(Reg::R12);
                a.ret();
            }
            _ => {}
        }
    }

    /// `[r12 + ...]`: variable `s`.
    fn local(s: u16) -> i32 {
        i32::from(s) * V
    }

    /// Makes room on the stack for `n` values, keeping the registers in use.
    fn room(&mut self, n: usize) {
        let (ok, a) = (self.a.label(), &mut *self.a);
        a.mov(Reg::R11, Reg::Rbx);
        a.alu_imm(Alu::Add, Reg::R11, n as i32 * V);
        a.cmp_global(Reg::R11, self.rt.limit);
        a.jcc(Cond::B, ok);
        // Six pushes keep the machine stack aligned.
        let saved = [Reg::Rax, Reg::Rcx, Reg::R8, Reg::R9, Reg::R10, Reg::R11];
        saved.iter().for_each(|&r| a.push(r));
        a.store_global(self.rt.sp, Reg::Rbx);
        a.mov_imm(Reg::Rdi, n as u32);
        a.call_addr(self.rt.grow);
        a.load_global(Reg::Rbx, self.rt.sp);
        saved.iter().rev().for_each(|&r| a.pop(r));
        a.bind(ok);
    }

    /// Pushes every value kept back, in order.
    fn flush(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return;
        }
        self.room(pending.len());
        let a = &mut *self.a;
        for (i, p) in pending.iter().enumerate() {
            let at = i as i32 * V;
            match *p {
                Opnd::Const(v) => {
                    a.store_imm(Reg::Rbx, at, 0);
                    a.mov_imm64(Reg::R11, v);
                    a.store(Reg::Rbx, at + 8, Reg::R11);
                }
                Opnd::Local(s) => {
                    for q in [0, 8, 16] {
                        a.load(Reg::R11, Reg::R12, Self::local(s) + q);
                        a.store(Reg::Rbx, at + q, Reg::R11);
                    }
                }
                Opnd::Tmp => {
                    a.store(Reg::Rbx, at, Reg::R8);
                    a.store(Reg::Rbx, at + 8, Reg::R9);
                    a.store(Reg::Rbx, at + 16, Reg::R10);
                }
            }
        }
        a.alu_imm(Alu::Add, Reg::Rbx, pending.len() as i32 * V);
    }

    /// Pushes what is kept back below the top `n` values (an instruction
    /// that takes `n` values sees only those), and gives the label of the
    /// instruction's slow path.
    fn prepare(&mut self, idx: usize, op: Op, next: Label, n: usize) -> Label {
        if self.pending.len() > n {
            let top = self.pending.split_off(self.pending.len() - n);
            self.flush();
            self.pending = top;
        }
        let l = self.a.label();
        self.slow.push((l, idx, op, next, self.pending.clone()));
        l
    }

    /// Goes to `slow` if `r` is NaT (`i64::MIN`: the only value whose `- 1` overflows).
    fn not_nat(&mut self, r: Reg, slow: Label) {
        self.a.mov(Reg::Rdx, r);
        self.a.alu_imm(Alu::Sub, Reg::Rdx, 1);
        self.a.jcc(Cond::O, slow);
    }

    /// Takes the top value into `dst`, going to `slow` unless it is an `int`
    /// (not NaT). Uses `rdx`.
    fn take_int(&mut self, dst: Reg, slow: Label) {
        match self.pending.pop() {
            Some(Opnd::Const(v)) => self.a.mov_imm64(dst, v),
            Some(Opnd::Local(s)) => {
                self.a.load(dst, Reg::R12, Self::local(s));
                self.a.test(dst);
                self.a.jcc(Cond::Ne, slow);
                self.a.load(dst, Reg::R12, Self::local(s) + 8);
                self.not_nat(dst, slow);
            }
            Some(Opnd::Tmp) => {
                self.a.test(Reg::R8);
                self.a.jcc(Cond::Ne, slow);
                self.a.mov(dst, Reg::R9);
                self.not_nat(dst, slow);
            }
            None => {
                self.real += 1;
                let at = -self.real * V;
                self.a.load(dst, Reg::Rbx, at);
                self.a.test(dst);
                self.a.jcc(Cond::Ne, slow);
                self.a.load(dst, Reg::Rbx, at + 8);
                self.not_nat(dst, slow);
            }
        }
    }

    /// Takes the top value, of any kind, without reading it yet.
    fn take_any(&mut self) -> Loc {
        match self.pending.pop() {
            Some(Opnd::Const(v)) => Loc::Const(v),
            Some(Opnd::Local(s)) => Loc::Mem(Reg::R12, Self::local(s)),
            Some(Opnd::Tmp) => Loc::Tmp,
            None => {
                self.real += 1;
                Loc::Mem(Reg::Rbx, -self.real * V)
            }
        }
    }

    /// Takes the top value, a pointer, into `rax` (block number in the low
    /// half, generation in the high half), or goes to `slow`.
    fn take_ptr(&mut self, slow: Label) {
        match self.take_any() {
            Loc::Const(_) => self.a.jmp(slow), // a number is never a pointer
            Loc::Mem(base, at) => {
                self.a.cmp_mem(base, at, PTR);
                self.a.jcc(Cond::Ne, slow);
                self.a.load(Reg::Rax, base, at + 8);
            }
            Loc::Tmp => {
                self.a.alu_imm(Alu::Cmp, Reg::R8, PTR);
                self.a.jcc(Cond::Ne, slow);
                self.a.mov(Reg::Rax, Reg::R9);
            }
        }
    }

    /// The block of the pointer in `rax`, into `rsi`, if it is alive, has one cell per
    /// element and the generation matches; otherwise `slow`.
    fn block(&mut self, slow: Label) {
        let (a, rt) = (&mut *self.a, self.rt);
        a.mov32(Reg::Rsi, Reg::Rax);
        a.shr(Reg::Rax, 32);
        a.cmp_global(Reg::Rsi, rt.nheap);
        a.jcc(Cond::Ae, slow);
        a.imul_imm(Reg::Rsi, Reg::Rsi, BLOCK);
        a.alu_global(Alu::Add, Reg::Rsi, rt.heap);
        a.cmp32_mem(Reg::Rsi, B_GEN, Reg::Rax);
        a.jcc(Cond::Ne, slow);
        a.cmp8_mem(Reg::Rsi, B_LIVE, 0);
        a.jcc(Cond::E, slow);
        a.cmp_mem(Reg::Rsi, B_W, 1);
        a.jcc(Cond::Ne, slow);
    }

    /// The address of cell `rcx` (an index) of the block in `rsi`, into `rcx`, or `slow`.
    fn cell(&mut self, slow: Label) {
        let a = &mut *self.a;
        a.alu_load(Alu::Sub, Reg::Rcx, Reg::Rsi, B_LO);
        a.alu_load(Alu::Cmp, Reg::Rcx, Reg::Rsi, B_LEN);
        a.jcc(Cond::Ae, slow); // also a negative one: unsigned, it is huge
        a.imul_imm(Reg::Rcx, Reg::Rcx, V);
        a.alu_load(Alu::Add, Reg::Rcx, Reg::Rsi, B_AT);
    }

    /// The values taken from the real stack are gone (the instruction succeeded).
    fn drop_taken(&mut self) {
        if self.real > 0 {
            self.a.alu_imm(Alu::Sub, Reg::Rbx, self.real * V);
            self.real = 0;
        }
    }

    /// The `int` in `rax` is the result, kept back in registers.
    fn result(&mut self) {
        self.drop_taken();
        self.a.mov_imm(Reg::R8, 0);
        self.a.mov(Reg::R9, Reg::Rax);
        self.pending.push(Opnd::Tmp);
    }

    /// Goes to `slow` unless `-WORD_MAX <= rax <= WORD_MAX` (uses `rcx`, `rdx`).
    fn in_range(&mut self, slow: Label) {
        let a = &mut *self.a;
        a.mov_imm64(Reg::Rdx, WORD_MAX);
        a.alu(Alu::Add, Reg::Rdx, Reg::Rax);
        a.mov_imm64(Reg::Rcx, 2 * WORD_MAX);
        a.alu(Alu::Cmp, Reg::Rdx, Reg::Rcx);
        a.jcc(Cond::A, slow);
    }

    /// Writes `op` as machine code for its common case (plain `int`s), with
    /// the runtime as its slow path. False if `op` always goes to the runtime.
    fn fast(&mut self, idx: usize, op: Op, next: Label, to: &dyn Fn(u32) -> Label) -> bool {
        match op {
            Op::PushC(k) => {
                let Some(v) = Word::from(self.module.consts[k as usize]).get() else { return false };
                if self.pending.len() >= MAX_PENDING {
                    self.flush();
                }
                self.pending.push(Opnd::Const(v));
            }
            Op::Load(s) => {
                if self.pending.len() >= MAX_PENDING {
                    self.flush();
                }
                self.pending.push(Opnd::Local(s));
            }
            Op::Store(s) => {
                let v = self.pending.pop();
                // A value kept back may read the variable: push it before it changes.
                if self.pending.contains(&Opnd::Local(s)) {
                    self.flush();
                }
                let (a, at) = (&mut *self.a, Self::local(s));
                match v {
                    Some(Opnd::Const(c)) => {
                        a.store_imm(Reg::R12, at, 0);
                        a.mov_imm64(Reg::Rax, c);
                        a.store(Reg::R12, at + 8, Reg::Rax);
                    }
                    Some(Opnd::Local(from)) => {
                        for q in [0, 8, 16] {
                            a.load(Reg::Rax, Reg::R12, Self::local(from) + q);
                            a.store(Reg::R12, at + q, Reg::Rax);
                        }
                    }
                    Some(Opnd::Tmp) => {
                        a.store(Reg::R12, at, Reg::R8);
                        a.store(Reg::R12, at + 8, Reg::R9);
                        a.store(Reg::R12, at + 16, Reg::R10);
                    }
                    None => {
                        a.alu_imm(Alu::Sub, Reg::Rbx, V);
                        for q in [0, 8, 16] {
                            a.load(Reg::Rax, Reg::Rbx, q);
                            a.store(Reg::R12, at + q, Reg::Rax);
                        }
                    }
                }
            }
            Op::Move(s) => {
                // Pushed now: the variable is left without a value (NaT).
                if self.pending.contains(&Opnd::Local(s)) || self.pending.len() >= MAX_PENDING {
                    self.flush();
                }
                self.room(self.pending.len() + 1);
                self.flush();
                let a = &mut *self.a;
                for q in [0, 8, 16] {
                    a.load(Reg::Rax, Reg::R12, Self::local(s) + q);
                    a.store(Reg::Rbx, q, Reg::Rax);
                }
                a.store_imm(Reg::R12, Self::local(s), 0);
                a.mov_imm64(Reg::Rax, i64::MIN);
                a.store(Reg::R12, Self::local(s) + 8, Reg::Rax);
                a.alu_imm(Alu::Add, Reg::Rbx, V);
            }
            Op::JNat(t) => {
                // A real may be NaT too: the runtime decides that one.
                self.flush();
                let slow = self.prepare(idx, op, next, 0);
                let a = &mut *self.a;
                a.load(Reg::Rax, Reg::Rbx, -V);
                a.alu_imm(Alu::Cmp, Reg::Rax, 1);
                a.jcc(Cond::E, slow);
                a.test(Reg::Rax);
                a.jcc(Cond::Ne, next); // a text or a pointer is never NaT
                a.load(Reg::Rax, Reg::Rbx, -V + 8);
                a.alu_imm(Alu::Sub, Reg::Rax, 1);
                a.jcc(Cond::No, next);
                a.alu_imm(Alu::Sub, Reg::Rbx, V);
                a.jmp(to(t));
            }
            Op::Call { f, argc } => {
                let callee = &self.module.functions[f as usize];
                let (n, argc) = (usize::from(callee.locals), usize::from(argc));
                if n - argc.min(n) > MAX_ZEROED {
                    return false;
                }
                self.flush();
                let slow = self.prepare(idx, op, next, 0);
                let (a, rt) = (&mut *self.a, self.rt);
                // Room for one more frame and its variables, or the runtime says why not.
                a.load_global(Reg::Rax, rt.nframes);
                a.alu_imm(Alu::Cmp, Reg::Rax, rt.max_depth);
                a.jcc(Cond::Ae, slow);
                a.load_global(Reg::Rcx, rt.nlocals);
                a.mov(Reg::Rdx, Reg::Rcx);
                a.alu_imm(Alu::Add, Reg::Rdx, n as i32);
                a.alu_imm(Alu::Cmp, Reg::Rdx, rt.max_locals);
                a.jcc(Cond::A, slow);
                // The arguments become the first variables, the others start at 0.
                a.imul_imm(Reg::Rdi, Reg::Rcx, V);
                a.alu_global(Alu::Add, Reg::Rdi, rt.locals_at);
                for i in 0..argc as i32 {
                    for q in [0, 8, 16] {
                        a.load(Reg::R11, Reg::Rbx, (i - argc as i32) * V + q);
                        a.store(Reg::Rdi, i * V + q, Reg::R11);
                    }
                }
                for i in argc as i32..n as i32 {
                    a.store_imm(Reg::Rdi, i * V, 0);
                    a.store_imm(Reg::Rdi, i * V + 8, 0);
                }
                a.alu_imm(Alu::Sub, Reg::Rbx, argc as i32 * V);
                // The new frame, after the caller's (which now waits past this call).
                a.imul_imm(Reg::Rsi, Reg::Rax, FRAME);
                a.alu_global(Alu::Add, Reg::Rsi, rt.frames);
                a.store_imm(Reg::Rsi, F_PC - FRAME, self.after as i32);
                a.store_imm(Reg::Rsi, F_FUNC, f as i32);
                a.store_imm(Reg::Rsi, F_PC, 0);
                a.store(Reg::Rsi, F_LOCALS, Reg::Rcx);
                // Its base: the stack's height, (rbx - STACK) / V, with V = 8 * 3:
                // a shift and a multiplication by the inverse of 3 (exact, as it divides).
                a.mov(Reg::R11, Reg::Rbx);
                a.alu_global(Alu::Sub, Reg::R11, rt.stack);
                a.shr(Reg::R11, 3);
                a.mov_imm64(Reg::Rdx, 0xAAAA_AAAA_AAAA_AAABu64 as i64);
                a.imul(Reg::R11, Reg::Rdx);
                a.store(Reg::Rsi, F_BASE, Reg::R11);
                a.store_imm(Reg::Rsi, F_SP, 0);
                a.store_imm(Reg::Rsi, F_SOFT, 0);
                a.alu_imm(Alu::Add, Reg::Rax, 1);
                a.store_global(rt.nframes, Reg::Rax);
                a.alu_imm(Alu::Add, Reg::Rcx, n as i32);
                a.store_global(rt.nlocals, Reg::Rcx);
                a.call(self.entries[f as usize]);
            }
            Op::Ret | Op::RetN(_) => {
                // The result stays on the stack for the caller. Leaving
                // `main` (the last frame) ends the program: the runtime does that.
                self.flush();
                let slow = self.prepare(idx, op, next, 0);
                let (a, rt) = (&mut *self.a, self.rt);
                a.load_global(Reg::Rax, rt.nframes);
                a.alu_imm(Alu::Cmp, Reg::Rax, 1);
                a.jcc(Cond::Be, slow);
                a.alu_imm(Alu::Sub, Reg::Rax, 1);
                a.store_global(rt.nframes, Reg::Rax);
                a.imul_imm(Reg::Rax, Reg::Rax, FRAME);
                a.alu_global(Alu::Add, Reg::Rax, rt.frames);
                a.load(Reg::Rcx, Reg::Rax, F_LOCALS);
                a.store_global(rt.nlocals, Reg::Rcx);
                a.pop(Reg::R12);
                a.ret();
            }
            Op::LdIdx => {
                let slow = self.prepare(idx, op, next, 2);
                self.take_int(Reg::Rcx, slow);
                self.take_ptr(slow);
                self.block(slow);
                self.cell(slow);
                self.drop_taken();
                let a = &mut *self.a;
                a.load(Reg::R8, Reg::Rcx, 0);
                a.load(Reg::R9, Reg::Rcx, 8);
                a.load(Reg::R10, Reg::Rcx, 16);
                self.pending.push(Opnd::Tmp);
            }
            Op::StIdx => {
                let slow = self.prepare(idx, op, next, 3);
                let value = self.take_any();
                self.take_int(Reg::Rcx, slow);
                self.take_ptr(slow);
                self.block(slow);
                self.cell(slow);
                let a = &mut *self.a;
                match value {
                    Loc::Const(v) => {
                        a.store_imm(Reg::Rcx, 0, 0);
                        a.mov_imm64(Reg::R11, v);
                        a.store(Reg::Rcx, 8, Reg::R11);
                    }
                    Loc::Mem(base, at) => {
                        for q in [0, 8, 16] {
                            a.load(Reg::R11, base, at + q);
                            a.store(Reg::Rcx, q, Reg::R11);
                        }
                    }
                    Loc::Tmp => {
                        a.store(Reg::Rcx, 0, Reg::R8);
                        a.store(Reg::Rcx, 8, Reg::R9);
                        a.store(Reg::Rcx, 16, Reg::R10);
                    }
                }
                self.drop_taken();
            }
            Op::TLen => {
                // An array's length; a text's goes to the runtime.
                let slow = self.prepare(idx, op, next, 1);
                self.take_ptr(slow);
                self.block(slow);
                self.a.load(Reg::Rax, Reg::Rsi, B_LEN);
                self.result();
            }
            Op::Pop => {
                if self.pending.pop().is_none() {
                    self.a.alu_imm(Alu::Sub, Reg::Rbx, V);
                }
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Cmp3 => {
                let slow = self.prepare(idx, op, next, 2);
                self.take_int(Reg::Rcx, slow);
                self.take_int(Reg::Rax, slow);
                let a = &mut *self.a;
                match op {
                    Op::Add => a.alu(Alu::Add, Reg::Rax, Reg::Rcx),
                    Op::Sub => a.alu(Alu::Sub, Reg::Rax, Reg::Rcx),
                    Op::Mul => {
                        a.imul(Reg::Rax, Reg::Rcx);
                        a.jcc(Cond::O, slow);
                    }
                    Op::Div | Op::Mod => {
                        // Division by zero: the runtime gives NaT or the trap.
                        a.test(Reg::Rcx);
                        a.jcc(Cond::E, slow);
                        a.cqo();
                        a.idiv(Reg::Rcx);
                        if op == Op::Mod {
                            a.mov(Reg::Rax, Reg::Rdx);
                        }
                    }
                    // Both are within ±WORD_MAX, so the difference does not overflow.
                    _ => {
                        a.alu(Alu::Sub, Reg::Rax, Reg::Rcx);
                        a.sign(Reg::Rax);
                    }
                }
                if matches!(op, Op::Add | Op::Sub | Op::Mul) {
                    self.in_range(slow);
                }
                self.result();
            }
            Op::Neg => {
                let slow = self.prepare(idx, op, next, 1);
                self.take_int(Reg::Rax, slow);
                self.a.neg(Reg::Rax);
                self.result();
            }
            Op::Test(mask) => {
                // The trit is `+` for the signs whose bit is set in `mask`, `-` otherwise.
                let slow = self.prepare(idx, op, next, 1);
                self.take_int(Reg::Rax, slow);
                let trit = |bit: u8| if mask & (1 << bit) != 0 { 1 } else { -1 };
                let (neg, zero, done) = (self.a.label(), self.a.label(), self.a.label());
                let a = &mut *self.a;
                a.test(Reg::Rax);
                a.jcc(Cond::L, neg);
                a.jcc(Cond::E, zero);
                a.mov_imm64(Reg::Rax, trit(2));
                a.jmp(done);
                a.bind(neg);
                a.mov_imm64(Reg::Rax, trit(0));
                a.jmp(done);
                a.bind(zero);
                a.mov_imm64(Reg::Rax, trit(1));
                a.bind(done);
                self.result();
            }
            Op::Br3 { neg, zero, pos } => {
                let slow = self.prepare(idx, op, next, 1);
                self.take_int(Reg::Rax, slow);
                self.drop_taken();
                let a = &mut *self.a;
                a.test(Reg::Rax);
                a.jcc(Cond::L, to(neg));
                a.jcc(Cond::E, to(zero));
                a.jmp(to(pos));
            }
            _ => return false,
        }
        true
    }
}
