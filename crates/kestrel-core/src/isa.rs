//! The BET-VM instruction set (SPEC §9.1) and its byte encoding.
//!
//! Every instruction is one opcode byte followed by fixed-size,
//! little-endian operands. Jump targets are absolute byte offsets inside the
//! function's own code.

use std::fmt;

/// Local slot index. Slots 0..=8 are shown as registers R-4..R+4.
pub type Slot = u16;
/// Byte offset inside a function's code.
pub type Addr = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    // Stack and locals
    /// Push word constant `k` from the constant pool.
    PushC(u32),
    /// Push real constant `k` from the constant pool.
    PushR(u32),
    /// Push text `k` from the string pool.
    PushS(u32),
    Load(Slot),
    Store(Slot),
    /// Push the local and poison its slot (ownership transfer).
    Move(Slot),
    Pop,

    // Integer ALU (overflow is NaT)
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Neg,
    Shl3,
    Shr3,
    Cmp3,
    KAnd,
    KOr,
    /// Turns a `cmp3` result into a trit: bit 0/1/2 of the mask is the answer
    /// for `-`/`0`/`+` (set = `+`, clear = `-`). NaT stays NaT.
    Test(u8),

    // Real ALU
    FAdd,
    FSub,
    FMul,
    FDiv,
    FPow,
    FNeg,
    FCmp3,

    // Text and width conversions
    TLen,
    /// Number to a new text, in decimal / ternary (`0t`) / heptavintimal (`0v`).
    TStr,
    TStrT,
    TStrV,
    TIdx,
    Narrow6,
    Narrow1,
    IToF,
    FToI,

    // Memory
    /// Pop N, push a pointer to a fresh symmetric buffer `-k..+k` (N = 2k+1, else Trap).
    Alloc,
    /// Pop N, push a pointer to a fresh buffer indexed `0..N-1` (N >= 0).
    AllocZ,
    /// Pop `n` values (the first pushed is the first cell) into a fresh buffer.
    Pack { n: u32, sym: bool },
    /// Pop index and pointer, push the cell.
    LdIdx,
    /// Pop value, index and pointer, store the cell.
    StIdx,
    /// Zero the buffer owned by the slot, free it, poison the slot.
    Purge(Slot),

    // Control flow
    Jmp(Addr),
    /// If the top of the stack is NaT, pop it and jump; otherwise leave it.
    JNat(Addr),
    /// Pop a trit and jump to the matching target. NaT is a Trap.
    Br3 { neg: Addr, zero: Addr, pos: Addr },
    /// Call function `f` with `argc` arguments taken from the stack.
    Call { f: u32, argc: u8 },
    /// Pop the return value, drop the frame, push the value for the caller.
    Ret,
    /// Stop with a trap; the reason is string `k`.
    Trap(u32),

    // I/O
    Input,
    Emit,
    Print,
    Show,
    /// Print an integer in ternary (`0t+-0`), no leading zeros.
    ShowT,
    /// Print an integer in heptavintimal (`0vD`), no leading zeros.
    ShowV,
    /// Print registers and heap, labeled with string `k`.
    Dump(u32),
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    BadOpcode { at: usize, byte: u8 },
    Truncated { at: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::BadOpcode { at, byte } => write!(f, "unknown opcode 0x{byte:02x} at byte {at}"),
            DecodeError::Truncated { at } => write!(f, "instruction at byte {at} is cut short"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Op {
    /// Opcode byte.
    pub fn opcode(self) -> u8 {
        use Op::*;
        match self {
            PushC(_) => 0x00,
            PushR(_) => 0x01,
            PushS(_) => 0x02,
            Load(_) => 0x03,
            Store(_) => 0x04,
            Move(_) => 0x05,
            Pop => 0x06,

            Add => 0x10,
            Sub => 0x11,
            Mul => 0x12,
            Div => 0x13,
            Mod => 0x14,
            Pow => 0x15,
            Neg => 0x16,
            Shl3 => 0x17,
            Shr3 => 0x18,
            Cmp3 => 0x19,
            KAnd => 0x1a,
            KOr => 0x1b,
            Test(_) => 0x1c,

            FAdd => 0x20,
            FSub => 0x21,
            FMul => 0x22,
            FDiv => 0x23,
            FPow => 0x24,
            FNeg => 0x25,
            FCmp3 => 0x26,

            TLen => 0x30,
            TIdx => 0x31,
            Narrow6 => 0x32,
            Narrow1 => 0x33,
            IToF => 0x34,
            FToI => 0x35,
            TStr => 0x36,
            TStrT => 0x37,
            TStrV => 0x38,

            Alloc => 0x40,
            LdIdx => 0x41,
            StIdx => 0x42,
            Purge(_) => 0x43,
            AllocZ => 0x44,
            Pack { .. } => 0x45,

            Jmp(_) => 0x50,
            JNat(_) => 0x51,
            Br3 { .. } => 0x52,
            Call { .. } => 0x53,
            Ret => 0x54,
            Trap(_) => 0x55,

            Input => 0x60,
            Emit => 0x61,
            Print => 0x62,
            Show => 0x63,
            Dump(_) => 0x64,
            ShowT => 0x65,
            ShowV => 0x66,
        }
    }

    /// Assembly mnemonic, as shown by the disassembler.
    pub fn mnemonic(self) -> &'static str {
        use Op::*;
        match self {
            PushC(_) => "pushc",
            PushR(_) => "pushr",
            PushS(_) => "pushs",
            Load(_) => "load",
            Store(_) => "store",
            Move(_) => "move",
            Pop => "pop",
            Add => "add",
            Sub => "sub",
            Mul => "mul",
            Div => "div",
            Mod => "mod",
            Pow => "pow",
            Neg => "neg",
            Shl3 => "shl3",
            Shr3 => "shr3",
            Cmp3 => "cmp3",
            KAnd => "kand",
            KOr => "kor",
            Test(_) => "test",
            FAdd => "fadd",
            FSub => "fsub",
            FMul => "fmul",
            FDiv => "fdiv",
            FPow => "fpow",
            FNeg => "fneg",
            FCmp3 => "fcmp3",
            TLen => "tlen",
            TStr => "tstr",
            TStrT => "tstrt",
            TStrV => "tstrv",
            TIdx => "tidx",
            Narrow6 => "narrow6",
            Narrow1 => "narrow1",
            IToF => "itof",
            FToI => "ftoi",
            Alloc => "alloc",
            AllocZ => "allocz",
            Pack { .. } => "pack",
            LdIdx => "ldidx",
            StIdx => "stidx",
            Purge(_) => "purge",
            Jmp(_) => "jmp",
            JNat(_) => "jnat",
            Br3 { .. } => "br3",
            Call { .. } => "call",
            Ret => "ret",
            Trap(_) => "trap",
            Input => "input",
            Emit => "emit",
            Print => "print",
            Show => "show",
            ShowT => "showt",
            ShowV => "showv",
            Dump(_) => "dump",
        }
    }

    /// Encoded size in bytes.
    pub fn size(self) -> usize {
        use Op::*;
        1 + match self {
            PushC(_) | PushR(_) | PushS(_) | Jmp(_) | JNat(_) | Dump(_) | Trap(_) => 4,
            Test(_) => 1,
            Load(_) | Store(_) | Move(_) | Purge(_) => 2,
            Br3 { .. } => 12,
            Call { .. } | Pack { .. } => 5,
            _ => 0,
        }
    }

    pub fn encode(self, out: &mut Vec<u8>) {
        use Op::*;
        out.push(self.opcode());
        match self {
            PushC(k) | PushR(k) | PushS(k) | Jmp(k) | JNat(k) | Dump(k) | Trap(k) => {
                out.extend_from_slice(&k.to_le_bytes())
            }
            Test(m) => out.push(m),
            Load(s) | Store(s) | Move(s) | Purge(s) => out.extend_from_slice(&s.to_le_bytes()),
            Br3 { neg, zero, pos } => {
                for a in [neg, zero, pos] {
                    out.extend_from_slice(&a.to_le_bytes());
                }
            }
            Call { f, argc } => {
                out.extend_from_slice(&f.to_le_bytes());
                out.push(argc);
            }
            Pack { n, sym } => {
                out.extend_from_slice(&n.to_le_bytes());
                out.push(u8::from(sym));
            }
            _ => {}
        }
    }

    /// Decodes the instruction at `at`, returning it and its size.
    pub fn decode(code: &[u8], at: usize) -> Result<(Op, usize), DecodeError> {
        use Op::*;
        let byte = *code.get(at).ok_or(DecodeError::Truncated { at })?;
        let mut r = Reader { code, pos: at + 1, start: at };
        let op = match byte {
            0x00 => PushC(r.u32()?),
            0x01 => PushR(r.u32()?),
            0x02 => PushS(r.u32()?),
            0x03 => Load(r.u16()?),
            0x04 => Store(r.u16()?),
            0x05 => Move(r.u16()?),
            0x06 => Pop,
            0x10 => Add,
            0x11 => Sub,
            0x12 => Mul,
            0x13 => Div,
            0x14 => Mod,
            0x15 => Pow,
            0x16 => Neg,
            0x17 => Shl3,
            0x18 => Shr3,
            0x19 => Cmp3,
            0x1a => KAnd,
            0x1b => KOr,
            0x1c => match r.u8()? {
                m @ 0..=7 => Test(m),
                _ => return Err(DecodeError::BadOpcode { at, byte }),
            },
            0x20 => FAdd,
            0x21 => FSub,
            0x22 => FMul,
            0x23 => FDiv,
            0x24 => FPow,
            0x25 => FNeg,
            0x26 => FCmp3,
            0x30 => TLen,
            0x31 => TIdx,
            0x32 => Narrow6,
            0x33 => Narrow1,
            0x34 => IToF,
            0x35 => FToI,
            0x36 => TStr,
            0x37 => TStrT,
            0x38 => TStrV,
            0x40 => Alloc,
            0x44 => AllocZ,
            0x45 => {
                let n = r.u32()?;
                match r.u8()? {
                    0 => Pack { n, sym: false },
                    1 => Pack { n, sym: true },
                    _ => return Err(DecodeError::BadOpcode { at, byte }),
                }
            }
            0x41 => LdIdx,
            0x42 => StIdx,
            0x43 => Purge(r.u16()?),
            0x50 => Jmp(r.u32()?),
            0x51 => JNat(r.u32()?),
            0x52 => Br3 { neg: r.u32()?, zero: r.u32()?, pos: r.u32()? },
            0x53 => Call { f: r.u32()?, argc: r.u8()? },
            0x54 => Ret,
            0x55 => Trap(r.u32()?),
            0x60 => Input,
            0x61 => Emit,
            0x62 => Print,
            0x63 => Show,
            0x64 => Dump(r.u32()?),
            0x65 => ShowT,
            0x66 => ShowV,
            _ => return Err(DecodeError::BadOpcode { at, byte }),
        };
        Ok((op, r.pos - at))
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Op::*;
        let m = self.mnemonic();
        match *self {
            PushC(k) | PushR(k) | PushS(k) | Dump(k) | Trap(k) => write!(f, "{m:<7} #{k}"),
            Test(mask) => {
                let name = match mask {
                    0b001 => "lt",
                    0b011 => "le",
                    0b010 => "eq",
                    0b101 => "ne",
                    0b110 => "ge",
                    0b100 => "gt",
                    _ => "",
                };
                write!(f, "{m:<7} {name}{}", if name.is_empty() { format!("{mask:03b}") } else { String::new() })
            }
            Load(s) | Store(s) | Move(s) | Purge(s) => write!(f, "{m:<7} {}", slot_name(s)),
            Jmp(a) | JNat(a) => write!(f, "{m:<7} @{a}"),
            Br3 { neg, zero, pos } => write!(f, "{m:<7} @{neg}, @{zero}, @{pos}"),
            Call { f: func, argc } => write!(f, "{m:<7} fn#{func}, {argc}"),
            Pack { n, sym } => write!(f, "{m:<7} {n}{}", if sym { ", sym" } else { "" }),
            _ => f.write_str(m),
        }
    }
}

/// Register name for slots 0..=8 (R-4..R+4), `L<n>` for spilled slots.
pub fn slot_name(s: Slot) -> String {
    match s {
        0..=3 => format!("R-{}", 4 - s),
        4 => "R0".to_string(),
        5..=8 => format!("R+{}", s - 4),
        _ => format!("L{s}"),
    }
}

struct Reader<'a> {
    code: &'a [u8],
    pos: usize,
    start: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let bytes = self
            .code
            .get(self.pos..self.pos + N)
            .ok_or(DecodeError::Truncated { at: self.start })?;
        self.pos += N;
        Ok(bytes.try_into().expect("slice has length N"))
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        self.take().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        self.take().map(u32::from_le_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_ops() -> Vec<Op> {
        use Op::*;
        vec![
            PushC(7), PushR(70_000), PushS(0), Load(4), Store(9), Move(0), Pop,
            Add, Sub, Mul, Div, Mod, Pow, Neg, Shl3, Shr3, Cmp3, KAnd, KOr, Test(0b101), Trap(4),
            FAdd, FSub, FMul, FDiv, FPow, FNeg, FCmp3,
            TLen, TIdx, Narrow6, Narrow1, IToF, FToI, TStr, TStrT, TStrV,
            Alloc, LdIdx, StIdx, Purge(8), AllocZ, Pack { n: 3, sym: true },
            Jmp(12), JNat(u32::MAX), Br3 { neg: 1, zero: 2, pos: 3 }, Call { f: 2, argc: 3 }, Ret,
            Input, Emit, Print, Show, Dump(5), ShowT, ShowV,
        ]
    }

    #[test]
    fn roundtrip_every_op() {
        let mut code = Vec::new();
        for op in all_ops() {
            let before = code.len();
            op.encode(&mut code);
            assert_eq!(code.len() - before, op.size(), "{op:?}");
        }
        let mut at = 0;
        for op in all_ops() {
            let (got, size) = Op::decode(&code, at).unwrap();
            assert_eq!(got, op);
            at += size;
        }
        assert_eq!(at, code.len());
    }

    #[test]
    fn opcodes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for op in all_ops() {
            assert!(seen.insert(op.opcode()), "duplicate opcode for {op:?}");
        }
    }

    #[test]
    fn decode_errors() {
        assert_eq!(Op::decode(&[0xff], 0), Err(DecodeError::BadOpcode { at: 0, byte: 0xff }));
        assert_eq!(Op::decode(&[0x50, 1, 2], 0), Err(DecodeError::Truncated { at: 0 }));
        assert_eq!(Op::decode(&[], 0), Err(DecodeError::Truncated { at: 0 }));
    }

    #[test]
    fn disassembly_text() {
        assert_eq!(Op::Load(0).to_string(), "load    R-4");
        assert_eq!(Op::Store(4).to_string(), "store   R0");
        assert_eq!(Op::Move(8).to_string(), "move    R+4");
        assert_eq!(Op::Purge(12).to_string(), "purge   L12");
        assert_eq!(Op::Br3 { neg: 4, zero: 9, pos: 14 }.to_string(), "br3     @4, @9, @14");
        assert_eq!(Op::Add.to_string(), "add");
    }
}
