//! The native backend: a linked program (`.ksb`) -> an x86-64 ELF
//! executable, written here with no assembler or linker.
//!
//! The executable is a copy of the runtime (`ksvm`, which has the heap,
//! texts, real numbers and printing) with one more segment: the program's
//! machine code, its `.ksb` and a table of addresses ([`elf`]). Each
//! instruction of the `.ksb` becomes machine code ([`x86`]): jumps and
//! calls are real jumps and calls; the other instructions call the runtime
//! (`krt_op` in `ksvm::native`), which runs them on its own stack.
//!
//! Every native function keeps the machine stack aligned to 16 bytes in
//! its body, as calls into Rust require:
//! ```text
//! F:    sub rsp, 8; mov rdi, rsp; call krt_enter_fn
//! op:   mov edi, func; mov esi, op; call krt_op; cmp rax, -1; je FAIL
//!       (then for jnat: test eax, eax; jnz A    br3: cmp eax, 1; jb A-; je A0; jmp A+
//!        call: call G     ret: add rsp, 8; ret)
//! jmp:  jmp A
//! FAIL: call krt_unwind; mov rsp, rdx; jmp rax     (a handler, in this or a caller's frame)
//! ```

pub mod elf;
pub mod x86;

use kestrel_core::isa::Op;
use kestrel_core::Module;

use elf::Elf;
use x86::{Asm, Cond, Reg};

/// `ksvm::native::PRESENT`
const PRESENT: u64 = u64::from_le_bytes(*b"KRT-PROG");

/// The executable for `module` (a linked program), made from `runtime`
/// (the bytes of a `ksvm` executable).
pub fn build(module: &Module, runtime: &[u8]) -> Result<Vec<u8>, String> {
    let elf = Elf::read(runtime)?;
    let sym = |n: &str| elf.symbol(n).ok_or_else(|| format!("the runtime has no `{n}` (is it a `ksvm` of this version, not stripped?)"));
    let (program, enter_fn, op, unwind) = (sym("KRT_PROGRAM")?, sym("krt_enter_fn")?, sym("krt_op")?, sym("krt_unwind")?);
    let base = elf.free_vaddr();
    let mut a = Asm::new(base);
    let entries: Vec<_> = module.functions.iter().map(|_| a.label()).collect();
    // For each function: the offsets of its handlers and their labels.
    let mut landings = Vec::new();
    for (k, f) in module.functions.iter().enumerate() {
        let mut ops = Vec::new();
        let mut pc = 0;
        while pc < f.code.len() {
            let (op, size) = Op::decode(&f.code, pc).map_err(|e| e.to_string())?;
            ops.push((op, pc));
            pc += size;
        }
        let at: std::collections::HashMap<usize, x86::Label> = ops.iter().map(|&(_, pc)| (pc, a.label())).collect();
        let fail = a.label();
        a.bind(entries[k]);
        a.sub_rsp(8);
        a.mov(Reg::Rdi, Reg::Rsp);
        a.call_addr(enter_fn);
        for (idx, &(o, pc)) in ops.iter().enumerate() {
            a.bind(at[&pc]);
            let to = |t: u32| at[&(t as usize)];
            if let Op::Jmp(t) = o {
                a.jmp(to(t));
                continue;
            }
            a.mov_imm(Reg::Rdi, k as u32);
            a.mov_imm(Reg::Rsi, idx as u32);
            a.call_addr(op);
            a.cmp_rax(-1);
            a.jcc(Cond::E, fail);
            match o {
                Op::JNat(t) => {
                    a.test_eax();
                    a.jcc(Cond::Ne, to(t));
                }
                Op::Br3 { neg, zero, pos } => {
                    a.cmp_rax(1);
                    a.jcc(Cond::B, to(neg));
                    a.jcc(Cond::E, to(zero));
                    a.jmp(to(pos));
                }
                Op::Call { f, .. } => a.call(entries[f as usize]),
                Op::Ret | Op::RetN(_) => {
                    a.add_rsp(8);
                    a.ret();
                }
                _ => {}
            }
        }
        a.bind(fail);
        a.call_addr(unwind);
        a.mov(Reg::Rsp, Reg::Rdx);
        a.jmp_reg(Reg::Rax);
        landings.push(f.handlers.iter().map(|h| (h.target, at[&(h.target as usize)])).collect::<Vec<_>>());
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
