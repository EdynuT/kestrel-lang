//! A small x86-64 assembler: just the instructions the native code needs,
//! encoded by hand (see the Intel manual, volume 2), with labels.

/// A place in the code, known before its address is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(usize);

/// Where a 32-bit jump or call goes.
enum Target {
    Label(Label),
    /// An address outside this code (a runtime function).
    Addr(u64),
}

/// The 64-bit registers the code uses, by their number in the encoding.
#[derive(Clone, Copy)]
pub enum Reg {
    Rax = 0,
    Rdx = 2,
    Rsp = 4,
    Rsi = 6,
    Rdi = 7,
}

/// The conditions of `jcc` (the low nibble of its opcode).
#[derive(Clone, Copy)]
pub enum Cond {
    /// Below (unsigned `<`).
    B = 0x2,
    E = 0x4,
    Ne = 0x5,
}

pub struct Asm {
    pub code: Vec<u8>,
    /// The address the code is loaded at.
    base: u64,
    labels: Vec<Option<usize>>,
    /// rel32 fields to fill in: where, and where they point.
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

    /// `mov r32, imm32` (also zeroes the top half of the register).
    pub fn mov_imm(&mut self, r: Reg, v: u32) {
        self.code.push(0xb8 + r as u8);
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// `mov dst, src` (64 bits).
    pub fn mov(&mut self, dst: Reg, src: Reg) {
        self.code.extend_from_slice(&[0x48, 0x89, 0xc0 | (src as u8) << 3 | dst as u8]);
    }

    /// `sub rsp, n` / `add rsp, n` (n < 128).
    pub fn sub_rsp(&mut self, n: u8) {
        self.code.extend_from_slice(&[0x48, 0x83, 0xec, n]);
    }
    pub fn add_rsp(&mut self, n: u8) {
        self.code.extend_from_slice(&[0x48, 0x83, 0xc4, n]);
    }

    /// `cmp rax, imm8` (sign-extended: `-1` compares with `u64::MAX`).
    pub fn cmp_rax(&mut self, v: i8) {
        self.code.extend_from_slice(&[0x48, 0x83, 0xf8, v as u8]);
    }

    /// `test eax, eax`
    pub fn test_eax(&mut self) {
        self.code.extend_from_slice(&[0x85, 0xc0]);
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
        self.code.extend_from_slice(&[0xff, 0xe0 | r as u8]);
    }
    pub fn ret(&mut self) {
        self.code.push(0xc3);
    }

    /// Fills in every jump and call: the code is final.
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

    #[test]
    fn encodings() {
        // Checked against `objdump -d`.
        let mut a = Asm::new(0x1000);
        a.mov_imm(Reg::Rdi, 5); // bf 05 00 00 00
        a.mov(Reg::Rdi, Reg::Rsp); // 48 89 e7
        a.mov(Reg::Rsp, Reg::Rdx); // 48 89 d4
        a.sub_rsp(8); // 48 83 ec 08
        a.cmp_rax(-1); // 48 83 f8 ff
        a.jmp_reg(Reg::Rax); // ff e0
        let top = a.label();
        a.bind(top);
        a.jcc(Cond::E, top); // 0f 84 fa ff ff ff (back to itself)
        a.call_addr(0x1000); // e8 ..: back to the start
        let code = a.finish().unwrap();
        assert_eq!(
            code,
            [
                0xbf, 5, 0, 0, 0, 0x48, 0x89, 0xe7, 0x48, 0x89, 0xd4, 0x48, 0x83, 0xec, 8, 0x48, 0x83, 0xf8, 0xff, 0xff, 0xe0, 0x0f,
                0x84, 0xfa, 0xff, 0xff, 0xff, 0xe8, 0xe0, 0xff, 0xff, 0xff
            ]
        );
    }
}
