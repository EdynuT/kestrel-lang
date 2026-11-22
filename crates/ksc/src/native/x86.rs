//! A small x86-64 assembler: just the instructions the native code needs,
//! encoded by hand (see the Intel manual, volume 2), with labels.
//!
//! An instruction on 64-bit registers starts with a REX byte (`0x48`, plus
//! bits for registers `r8`-`r15`), then the opcode, then a ModRM byte that
//! names the registers (`mod` 11) or a register and a memory place (`mod`
//! 10: a base register plus a 32-bit offset; `mod` 00 with `rm` 101: an
//! offset from the next instruction, "RIP-relative").

/// A place in the code, known before its address is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(usize);

/// Where a 32-bit offset in the code points.
enum Target {
    Label(Label),
    /// An address outside this code (a runtime function or variable).
    Addr(u64),
}

/// The 64-bit registers, by their number in the encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
}

impl Reg {
    fn low(self) -> u8 {
        self as u8 & 7
    }
    fn high(self) -> u8 {
        (self as u8 >> 3) & 1
    }
}

/// The conditions of `jcc` and `setcc` (the low nibble of their opcodes).
#[derive(Clone, Copy)]
pub enum Cond {
    /// Overflow.
    O = 0x0,
    /// No overflow.
    No = 0x1,
    /// Below (unsigned `<`).
    B = 0x2,
    /// Above or equal (unsigned `>=`).
    Ae = 0x3,
    /// Below or equal (unsigned `<=`).
    Be = 0x6,
    E = 0x4,
    Ne = 0x5,
    /// Above (unsigned `>`).
    A = 0x7,
    /// Less (signed `<`).
    L = 0xc,
    /// Greater (signed `>`).
    G = 0xf,
}

/// Operations of the form `op r/m, r` (the opcode) and `op r/m, imm32` (the ModRM digit).
#[derive(Clone, Copy)]
pub enum Alu {
    Add,
    Or,
    Sub,
    Cmp,
}

impl Alu {
    fn rr(self) -> u8 {
        match self {
            Alu::Add => 0x01,
            Alu::Or => 0x09,
            Alu::Sub => 0x29,
            Alu::Cmp => 0x39,
        }
    }
    fn digit(self) -> u8 {
        match self {
            Alu::Add => 0,
            Alu::Or => 1,
            Alu::Sub => 5,
            Alu::Cmp => 7,
        }
    }
}

pub struct Asm {
    pub code: Vec<u8>,
    /// The address the code is loaded at.
    base: u64,
    labels: Vec<Option<usize>>,
    /// 32-bit offsets to fill in: where (they end the instruction), and where they point.
    fixups: Vec<(usize, Target)>,
}

impl Asm {
    pub fn new(base: u64) -> Asm {
        Asm { code: Vec::new(), base, labels: Vec::new(), fixups: Vec::new() }
    }

    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    /// `l` is here.
    pub fn bind(&mut self, l: Label) {
        self.labels[l.0] = Some(self.code.len());
    }

    /// The address of a bound label.
    pub fn addr(&self, l: Label) -> u64 {
        self.base + self.labels[l.0].expect("bound") as u64
    }

    fn rel32(&mut self, t: Target) {
        self.fixups.push((self.code.len(), t));
        self.code.extend_from_slice(&[0; 4]);
    }

    /// REX.W with the high bits of `reg` (ModRM.reg) and `rm` (ModRM.rm or base).
    fn rex(&mut self, reg: u8, rm: Reg) {
        self.code.push(0x48 | (reg >> 3 & 1) << 2 | rm.high());
    }

    /// ModRM for `[base + disp]` (always a 32-bit offset), with `reg` in the middle.
    fn mem(&mut self, reg: u8, base: Reg, disp: i32) {
        self.code.push(0x80 | (reg & 7) << 3 | base.low());
        if base.low() == 4 {
            self.code.push(0x24); // rsp and r12 as a base need a SIB byte
        }
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    /// `mov dst, [base + disp]`
    pub fn load(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.rex(dst as u8, base);
        self.code.push(0x8b);
        self.mem(dst as u8, base, disp);
    }

    /// `mov [base + disp], src`
    pub fn store(&mut self, base: Reg, disp: i32, src: Reg) {
        self.rex(src as u8, base);
        self.code.push(0x89);
        self.mem(src as u8, base, disp);
    }

    /// `mov qword [base + disp], imm32` (sign-extended).
    pub fn store_imm(&mut self, base: Reg, disp: i32, v: i32) {
        self.rex(0, base);
        self.code.push(0xc7);
        self.mem(0, base, disp);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `mov dst, [rip + to addr]` / `mov [rip + to addr], src`: a runtime variable.
    pub fn load_global(&mut self, dst: Reg, addr: u64) {
        self.rex(dst as u8, Reg::Rax);
        self.code.extend_from_slice(&[0x8b, (dst.low() << 3) | 5]);
        self.rel32(Target::Addr(addr));
    }
    pub fn store_global(&mut self, addr: u64, src: Reg) {
        self.rex(src as u8, Reg::Rax);
        self.code.extend_from_slice(&[0x89, (src.low() << 3) | 5]);
        self.rel32(Target::Addr(addr));
    }

    /// `cmp r, [rip + to addr]`
    pub fn cmp_global(&mut self, r: Reg, addr: u64) {
        self.alu_global(Alu::Cmp, r, addr);
    }

    /// `op r, [rip + to addr]`
    pub fn alu_global(&mut self, op: Alu, r: Reg, addr: u64) {
        self.rex(r as u8, Reg::Rax);
        self.code.extend_from_slice(&[op.rr() + 2, (r.low() << 3) | 5]);
        self.rel32(Target::Addr(addr));
    }

    /// `imul dst, src, imm32`
    pub fn imul_imm(&mut self, dst: Reg, src: Reg, v: i32) {
        self.rex(dst as u8, src);
        self.code.extend_from_slice(&[0x69, 0xc0 | dst.low() << 3 | src.low()]);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `shr r, n`
    pub fn shr(&mut self, r: Reg, n: u8) {
        self.rex(0, r);
        self.code.extend_from_slice(&[0xc1, 0xe8 | r.low(), n]);
    }

    /// `mov r, imm64`
    pub fn mov_imm64(&mut self, r: Reg, v: i64) {
        self.code.extend_from_slice(&[0x48 | r.high(), 0xb8 + r.low()]);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `mov r32, imm32` (also zeroes the top half of the register).
    pub fn mov_imm(&mut self, r: Reg, v: u32) {
        if r.high() == 1 {
            self.code.push(0x41);
        }
        self.code.push(0xb8 + r.low());
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `mov dst, src`
    pub fn mov(&mut self, dst: Reg, src: Reg) {
        self.alu_raw(0x89, dst, src);
    }

    fn alu_raw(&mut self, op: u8, dst: Reg, src: Reg) {
        self.rex(src as u8, dst);
        self.code.extend_from_slice(&[op, 0xc0 | src.low() << 3 | dst.low()]);
    }

    /// `op dst, src`
    pub fn alu(&mut self, op: Alu, dst: Reg, src: Reg) {
        self.alu_raw(op.rr(), dst, src);
    }

    /// `op r, imm32` (sign-extended).
    pub fn alu_imm(&mut self, op: Alu, r: Reg, v: i32) {
        self.rex(0, r);
        self.code.extend_from_slice(&[0x81, 0xc0 | op.digit() << 3 | r.low()]);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `or dst, [base + disp]`
    pub fn or_load(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.alu_load(Alu::Or, dst, base, disp);
    }

    /// `op dst, [base + disp]`
    pub fn alu_load(&mut self, op: Alu, dst: Reg, base: Reg, disp: i32) {
        self.rex(dst as u8, base);
        self.code.push(op.rr() + 2);
        self.mem(dst as u8, base, disp);
    }

    /// `cmp qword [base + disp], imm32`
    pub fn cmp_mem(&mut self, base: Reg, disp: i32, v: i32) {
        self.rex(0, base);
        self.code.push(0x81);
        self.mem(7, base, disp);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `cmp dword [base + disp], r32`
    pub fn cmp32_mem(&mut self, base: Reg, disp: i32, r: Reg) {
        if r.high() == 1 || base.high() == 1 {
            self.code.push(0x40 | r.high() << 2 | base.high());
        }
        self.code.push(0x39);
        self.mem(r as u8, base, disp);
    }

    /// `cmp byte [base + disp], imm8`
    pub fn cmp8_mem(&mut self, base: Reg, disp: i32, v: u8) {
        if base.high() == 1 {
            self.code.push(0x41);
        }
        self.code.push(0x80);
        self.mem(7, base, disp);
        self.code.push(v);
    }

    /// `mov dst32, src32` (zeroes the top half of `dst`).
    pub fn mov32(&mut self, dst: Reg, src: Reg) {
        if dst.high() == 1 || src.high() == 1 {
            self.code.push(0x40 | src.high() << 2 | dst.high());
        }
        self.code.extend_from_slice(&[0x89, 0xc0 | src.low() << 3 | dst.low()]);
    }

    /// `imul dst, src` (sets the overflow flag when the product does not fit).
    pub fn imul(&mut self, dst: Reg, src: Reg) {
        self.rex(dst as u8, src);
        self.code.extend_from_slice(&[0x0f, 0xaf, 0xc0 | dst.low() << 3 | src.low()]);
    }

    /// `cqo`: sign-extends `rax` into `rdx`, before `idiv`.
    pub fn cqo(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x99]);
    }

    /// `idiv r`: `rdx:rax / r`, the quotient (towards zero) in `rax`, the remainder in `rdx`.
    pub fn idiv(&mut self, r: Reg) {
        self.rex(0, r);
        self.code.extend_from_slice(&[0xf7, 0xf8 | r.low()]);
    }

    /// `neg r`
    pub fn neg(&mut self, r: Reg) {
        self.rex(0, r);
        self.code.extend_from_slice(&[0xf7, 0xd8 | r.low()]);
    }

    /// `test r, r`
    pub fn test(&mut self, r: Reg) {
        self.alu_raw(0x85, r, r);
    }

    /// `r = sign(r)`: -1, 0 or 1 (uses `rcx` and `rdx`).
    ///   test r, r; setg dl; setl cl; sub dl, cl; movsx r, dl
    pub fn sign(&mut self, r: Reg) {
        self.test(r);
        self.code.extend_from_slice(&[0x0f, 0x90 | Cond::G as u8, 0xc2]);
        self.code.extend_from_slice(&[0x0f, 0x90 | Cond::L as u8, 0xc1]);
        self.code.extend_from_slice(&[0x28, 0xca]);
        self.rex(r as u8, Reg::Rdx);
        self.code.extend_from_slice(&[0x0f, 0xbe, 0xc0 | r.low() << 3 | Reg::Rdx.low()]);
    }

    pub fn push(&mut self, r: Reg) {
        if r.high() == 1 {
            self.code.push(0x41);
        }
        self.code.push(0x50 + r.low());
    }
    pub fn pop(&mut self, r: Reg) {
        if r.high() == 1 {
            self.code.push(0x41);
        }
        self.code.push(0x58 + r.low());
    }

    pub fn call_addr(&mut self, addr: u64) {
        self.code.push(0xe8);
        self.rel32(Target::Addr(addr));
    }
    pub fn call(&mut self, l: Label) {
        self.code.push(0xe8);
        self.rel32(Target::Label(l));
    }
    pub fn jmp(&mut self, l: Label) {
        self.code.push(0xe9);
        self.rel32(Target::Label(l));
    }
    pub fn jcc(&mut self, c: Cond, l: Label) {
        self.code.extend_from_slice(&[0x0f, 0x80 | c as u8]);
        self.rel32(Target::Label(l));
    }
    /// `jmp r` (to an address in a register).
    pub fn jmp_reg(&mut self, r: Reg) {
        self.code.extend_from_slice(&[0xff, 0xe0 | r.low()]);
    }
    pub fn ret(&mut self) {
        self.code.push(0xc3);
    }

    /// Fills in every jump, call and RIP-relative offset: the code is final.
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        for (at, t) in std::mem::take(&mut self.fixups) {
            let to = match t {
                Target::Label(l) => self.addr(l),
                Target::Addr(a) => a,
            };
            let from = self.base + at as u64 + 4;
            let rel = i32::try_from(to as i64 - from as i64).map_err(|_| "a jump is too far for 32 bits".to_string())?;
            self.code[at..at + 4].copy_from_slice(&rel.to_le_bytes());
        }
        Ok(self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut Asm)) -> Vec<u8> {
        let mut a = Asm::new(0x1000);
        f(&mut a);
        a.finish().unwrap()
    }

    #[test]
    fn encodings() {
        // Each one checked against `objdump -d`.
        assert_eq!(bytes(|a| a.mov_imm(Reg::Rdi, 5)), [0xbf, 5, 0, 0, 0]);
        assert_eq!(bytes(|a| a.mov(Reg::Rdi, Reg::Rsp)), [0x48, 0x89, 0xe7]);
        assert_eq!(bytes(|a| a.mov(Reg::R12, Reg::Rax)), [0x49, 0x89, 0xc4]);
        assert_eq!(bytes(|a| a.load(Reg::Rax, Reg::Rbx, -40)), [0x48, 0x8b, 0x83, 0xd8, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(|a| a.load(Reg::Rax, Reg::R12, 8)), [0x49, 0x8b, 0x84, 0x24, 8, 0, 0, 0]);
        assert_eq!(bytes(|a| a.store(Reg::R12, 0, Reg::Rcx)), [0x49, 0x89, 0x8c, 0x24, 0, 0, 0, 0]);
        assert_eq!(bytes(|a| a.store_imm(Reg::Rbx, 0, 0)), [0x48, 0xc7, 0x83, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bytes(|a| a.alu_imm(Alu::Add, Reg::Rbx, 24)), [0x48, 0x81, 0xc3, 24, 0, 0, 0]);
        assert_eq!(bytes(|a| a.alu(Alu::Sub, Reg::Rax, Reg::Rcx)), [0x48, 0x29, 0xc8]);
        assert_eq!(bytes(|a| a.or_load(Reg::Rax, Reg::Rbx, -24)), [0x48, 0x0b, 0x83, 0xe8, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(|a| a.imul(Reg::Rax, Reg::Rcx)), [0x48, 0x0f, 0xaf, 0xc1]);
        assert_eq!(bytes(|a| a.neg(Reg::Rax)), [0x48, 0xf7, 0xd8]);
        assert_eq!(bytes(|a| a.mov_imm64(Reg::Rdx, -1)), [0x48, 0xba, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(|a| a.push(Reg::R12)), [0x41, 0x54]);
        assert_eq!(bytes(|a| a.push(Reg::R9)), [0x41, 0x51]);
        assert_eq!(bytes(|a| a.mov(Reg::R8, Reg::Rax)), [0x49, 0x89, 0xc0]);
        assert_eq!(bytes(|a| a.mov(Reg::Rax, Reg::R9)), [0x4c, 0x89, 0xc8]);
        assert_eq!(bytes(|a| a.load(Reg::R10, Reg::Rbx, -8)), [0x4c, 0x8b, 0x93, 0xf8, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(|a| a.cqo()), [0x48, 0x99]);
        assert_eq!(bytes(|a| a.cmp_mem(Reg::R12, 0, 3)), [0x49, 0x81, 0xbc, 0x24, 0, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(bytes(|a| a.cmp32_mem(Reg::Rsi, 32, Reg::Rax)), [0x39, 0x86, 32, 0, 0, 0]);
        assert_eq!(bytes(|a| a.cmp8_mem(Reg::Rsi, 36, 0)), [0x80, 0xbe, 36, 0, 0, 0, 0]);
        assert_eq!(bytes(|a| a.mov32(Reg::Rsi, Reg::Rax)), [0x89, 0xc6]);
        assert_eq!(bytes(|a| a.alu_load(Alu::Sub, Reg::Rcx, Reg::Rsi, 24)), [0x48, 0x2b, 0x8e, 24, 0, 0, 0]);
        assert_eq!(bytes(|a| a.imul_imm(Reg::R12, Reg::R12, 24)), [0x4d, 0x69, 0xe4, 24, 0, 0, 0]);
        assert_eq!(bytes(|a| a.shr(Reg::R11, 3)), [0x49, 0xc1, 0xeb, 3]);
        assert_eq!(bytes(|a| a.alu_global(Alu::Add, Reg::Rax, 0x1007)), [0x48, 0x03, 0x05, 0, 0, 0, 0]);
        assert_eq!(bytes(|a| a.idiv(Reg::Rcx)), [0x48, 0xf7, 0xf9]);
        assert_eq!(bytes(|a| a.pop(Reg::R12)), [0x41, 0x5c]);
        assert_eq!(bytes(|a| a.jmp_reg(Reg::Rax)), [0xff, 0xe0]);
        // test rax,rax; setg dl; setl cl; sub dl,cl; movsx rax,dl
        assert_eq!(bytes(|a| a.sign(Reg::Rax)), [0x48, 0x85, 0xc0, 0x0f, 0x9f, 0xc2, 0x0f, 0x9c, 0xc1, 0x28, 0xca, 0x48, 0x0f, 0xbe, 0xc2]);
        // RIP-relative: the offset counts from the end of the instruction.
        assert_eq!(bytes(|a| a.load_global(Reg::Rbx, 0x1000)), [0x48, 0x8b, 0x1d, 0xf9, 0xff, 0xff, 0xff]);
        assert_eq!(bytes(|a| a.cmp_global(Reg::Rbx, 0x1007)), [0x48, 0x3b, 0x1d, 0, 0, 0, 0]);
        // A jump back to itself, and a call back to the start.
        let code = bytes(|a| {
            let top = a.label();
            a.bind(top);
            a.jcc(Cond::E, top);
            a.call_addr(0x1000);
        });
        assert_eq!(code, [0x0f, 0x84, 0xfa, 0xff, 0xff, 0xff, 0xe8, 0xf5, 0xff, 0xff, 0xff]);
    }
}
