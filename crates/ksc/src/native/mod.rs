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
//! F:    push r12; mov rdi, rsp; call krt_enter_fn; mov r12, rax; mov rbx, [KRT_SP]
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
const ABI: u64 = 2;
/// The size of a `ksvm::Value`; its value starts at byte 8.
const V: i32 = 24;

/// The runtime's functions and variables the code uses.
struct Runtime {
    enter_fn: u64,
    op: u64,
    unwind: u64,
    locals: u64,
    sp: u64,
    limit: u64,
}

/// The executable for `module` (a linked program), made from `runtime`
/// (the bytes of a `ksvm` executable).
pub fn build(module: &Module, runtime: &[u8]) -> Result<Vec<u8>, String> {
    let elf = Elf::read(runtime)?;
    let sym = |n: &str| elf.symbol(n).ok_or_else(|| format!("the runtime has no `{n}` (is it a `ksvm` of this version, not stripped?)"));
    let abi = elf.file_offset(sym("KRT_ABI")?).and_then(|at| runtime.get(at..at + 16)).ok_or("the runtime's `KRT_ABI` is not stored")?;
    if abi[..8] != ABI.to_le_bytes() || abi[8..] != (V as u64).to_le_bytes() {
        return Err("the runtime (`ksvm`) is not from this version of `ksc`".into());
    }
    let program = sym("KRT_PROGRAM")?;
    let rt = Runtime {
        enter_fn: sym("krt_enter_fn")?,
        op: sym("krt_op")?,
        unwind: sym("krt_unwind")?,
        locals: sym("krt_locals")?,
        sp: sym("KRT_SP")?,
        limit: sym("KRT_LIMIT")?,
    };
    let base = elf.free_vaddr();
    let mut a = Asm::new(base);
    let entries: Vec<Label> = module.functions.iter().map(|_| a.label()).collect();
    let mut landings = Vec::new();
    for k in 0..module.functions.len() {
        landings.push(Gen { a: &mut a, rt: &rt, module, k, entries: &entries, slow: Vec::new() }.function()?);
    }
    // Addresses relative to `KRT_PROGRAM`, which is where the runtime finds them.
    let rel = |addr: u64| (addr as i64 - program as i64).to_le_bytes();
    let mut table = (module.functions.len() as u32).to_le_bytes().to_vec();
    for (k, l) in landings.iter().enumerate() {
        table.extend_from_slice(&rel(a.addr(entries[k])));
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

/// The code of one function.
struct Gen<'a> {
    a: &'a mut Asm,
    rt: &'a Runtime,
    module: &'a Module,
    k: usize,
    entries: &'a [Label],
    /// Instructions done here only for plain `int`s: their slow path (the
    /// runtime), written after the function: label, instruction number, the
    /// instruction, and where to go on.
    slow: Vec<(Label, usize, Op, Label)>,
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
        let mut at: HashMap<usize, Label> = ops.iter().map(|&(_, pc)| (pc, self.a.label())).collect();
        let end = self.a.label();
        at.insert(f.code.len(), end);
        let fail = self.a.label();
        let a = &mut *self.a;
        a.bind(self.entries[self.k]);
        a.push(Reg::R12);
        a.mov(Reg::Rdi, Reg::Rsp);
        a.call_addr(self.rt.enter_fn);
        a.mov(Reg::R12, Reg::Rax);
        a.load_global(Reg::Rbx, self.rt.sp);
        for (idx, &(op, pc)) in ops.iter().enumerate() {
            self.a.bind(at[&pc]);
            let next = ops.get(idx + 1).map_or(end, |&(_, p)| at[&p]);
            let to = |t: u32| at[&(t as usize)];
            if let Op::Jmp(t) = op {
                self.a.jmp(to(t));
            } else if !self.fast(idx, op, next, &to) {
                self.generic(idx, op, fail, &to);
            }
        }
        // Falling off the end: the runtime reports it (there is no instruction `ops.len()`).
        self.a.bind(end);
        self.generic(ops.len(), Op::Pop, fail, &|_| end);
        for (label, idx, op, next) in std::mem::take(&mut self.slow) {
            self.a.bind(label);
            self.generic(idx, op, fail, &|t| at[&(t as usize)]);
            if !matches!(op, Op::Br3 { .. }) {
                self.a.jmp(next);
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

    /// Instruction number `idx`, run by the runtime.
    fn generic(&mut self, idx: usize, op: Op, fail: Label, to: &dyn Fn(u32) -> Label) {
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

    /// A label for the slow path of instruction `idx` (written after the function).
    fn slow_path(&mut self, idx: usize, op: Op, next: Label) -> Label {
        let l = self.a.label();
        self.slow.push((l, idx, op, next));
        l
    }

    /// `[r12 + ...]`: variable `s`.
    fn local(s: u16) -> i32 {
        i32::from(s) * V
    }

    /// Goes to `slow` unless the value at `[rbx + at]` is a `Word` (kind 0).
    fn word_at(&mut self, at: i32, slow: Label) {
        self.a.load(Reg::Rax, Reg::Rbx, at);
        self.a.test(Reg::Rax);
        self.a.jcc(Cond::Ne, slow);
    }

    /// Goes to `slow` if `r` is NaT (`i64::MIN`: the only value whose `- 1` overflows).
    fn not_nat(&mut self, r: Reg, slow: Label) {
        self.a.mov(Reg::Rdx, r);
        self.a.alu_imm(Alu::Sub, Reg::Rdx, 1);
        self.a.jcc(Cond::O, slow);
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

    /// Goes to `slow` unless the stack has room for one more value.
    fn room(&mut self, slow: Label) {
        self.a.cmp_global(Reg::Rbx, self.rt.limit);
        self.a.jcc(Cond::Ae, slow);
    }

    /// Copies a value (three 8-byte parts) from `[from + f]` to `[to + t]`.
    fn copy(&mut self, from: Reg, f: i32, to: Reg, t: i32) {
        for q in [0, 8, 16] {
            self.a.load(Reg::Rax, from, f + q);
            self.a.store(to, t + q, Reg::Rax);
        }
    }

    /// The two `int`s on top of the stack into `rax` (below) and `rcx` (top), or `slow`.
    fn two_ints(&mut self, slow: Label) {
        let a = &mut *self.a;
        a.load(Reg::Rax, Reg::Rbx, -2 * V);
        a.or_load(Reg::Rax, Reg::Rbx, -V);
        a.test(Reg::Rax);
        a.jcc(Cond::Ne, slow);
        a.load(Reg::Rcx, Reg::Rbx, -V + 8);
        self.not_nat(Reg::Rcx, slow);
        self.a.load(Reg::Rax, Reg::Rbx, -2 * V + 8);
        self.not_nat(Reg::Rax, slow);
    }

    /// Replaces the two values on top with the `int` in `rax`.
    fn put_int2(&mut self) {
        self.a.store(Reg::Rbx, -2 * V + 8, Reg::Rax);
        self.a.alu_imm(Alu::Sub, Reg::Rbx, V);
    }

    /// Writes `op` as machine code for its common case (plain `int`s), with
    /// the runtime as its slow path. False if `op` always goes to the runtime.
    fn fast(&mut self, idx: usize, op: Op, next: Label, to: &dyn Fn(u32) -> Label) -> bool {
        match op {
            Op::PushC(k) => {
                let Some(v) = Word::from(self.module.consts[k as usize]).get() else { return false };
                let slow = self.slow_path(idx, op, next);
                self.room(slow);
                let a = &mut *self.a;
                a.store_imm(Reg::Rbx, 0, 0);
                a.mov_imm64(Reg::Rax, v);
                a.store(Reg::Rbx, 8, Reg::Rax);
                a.alu_imm(Alu::Add, Reg::Rbx, V);
            }
            Op::Load(s) | Op::Move(s) => {
                let slow = self.slow_path(idx, op, next);
                self.room(slow);
                self.copy(Reg::R12, Self::local(s), Reg::Rbx, 0);
                if let Op::Move(_) = op {
                    // The variable is left without a value (NaT).
                    self.a.store_imm(Reg::R12, Self::local(s), 0);
                    self.a.mov_imm64(Reg::Rax, i64::MIN);
                    self.a.store(Reg::R12, Self::local(s) + 8, Reg::Rax);
                }
                self.a.alu_imm(Alu::Add, Reg::Rbx, V);
            }
            Op::Store(s) => {
                self.a.alu_imm(Alu::Sub, Reg::Rbx, V);
                self.copy(Reg::Rbx, 0, Reg::R12, Self::local(s));
            }
            Op::Pop => self.a.alu_imm(Alu::Sub, Reg::Rbx, V),
            Op::Add | Op::Sub | Op::Mul => {
                let slow = self.slow_path(idx, op, next);
                self.two_ints(slow);
                match op {
                    Op::Add => self.a.alu(Alu::Add, Reg::Rax, Reg::Rcx),
                    Op::Sub => self.a.alu(Alu::Sub, Reg::Rax, Reg::Rcx),
                    _ => {
                        self.a.imul(Reg::Rax, Reg::Rcx);
                        self.a.jcc(Cond::O, slow);
                    }
                }
                self.in_range(slow);
                self.put_int2();
            }
            Op::Cmp3 => {
                let slow = self.slow_path(idx, op, next);
                self.two_ints(slow);
                // Both are within ±WORD_MAX, so the difference does not overflow.
                self.a.alu(Alu::Sub, Reg::Rax, Reg::Rcx);
                self.a.sign(Reg::Rax);
                self.put_int2();
            }
            Op::Neg => {
                let slow = self.slow_path(idx, op, next);
                self.word_at(-V, slow);
                self.a.load(Reg::Rax, Reg::Rbx, -V + 8);
                self.not_nat(Reg::Rax, slow);
                self.a.neg(Reg::Rax);
                self.a.store(Reg::Rbx, -V + 8, Reg::Rax);
            }
            Op::Test(mask) => {
                // The trit is `+` for the signs whose bit is set in `mask`, `-` otherwise.
                let slow = self.slow_path(idx, op, next);
                self.word_at(-V, slow);
                self.a.load(Reg::Rax, Reg::Rbx, -V + 8);
                self.not_nat(Reg::Rax, slow);
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
                a.store(Reg::Rbx, -V + 8, Reg::Rax);
            }
            Op::Br3 { neg, zero, pos } => {
                let slow = self.slow_path(idx, op, next);
                self.word_at(-V, slow);
                self.a.load(Reg::Rax, Reg::Rbx, -V + 8);
                self.not_nat(Reg::Rax, slow);
                let a = &mut *self.a;
                a.alu_imm(Alu::Sub, Reg::Rbx, V);
                a.test(Reg::Rax);
                a.jcc(Cond::L, to(neg));
                a.jcc(Cond::E, to(zero));
                a.jmp(to(pos));
            }
            Op::JNat(t) => {
                // A real may be NaT too: the runtime decides that one.
                let slow = self.slow_path(idx, op, next);
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
            _ => return false,
        }
        true
    }
}
