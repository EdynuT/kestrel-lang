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
    /// Pop a value, push `+1` if it is NaT (no value), `-1` otherwise. Never traps.
    IsNaT,

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
    /// Inside an `on NaT` line: failed arithmetic gives NaT instead of a trap.
    Soft,
    /// Back to the default: failed arithmetic traps.
    Hard,
    /// `(bool) x`: keeps 1 or -1; anything else is a failed conversion.
    ToBool,

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
    /// Pop b, a (texts), push the new text `a + b`.
    TCat,
    /// Pop b, a (texts), push `+` if equal, `-` if not.
    TEq,
    /// Pop end, start, text; push the new text of characters `start..end-1`.
    TSub,
    /// Text to int / real (decimal; ints also `0t`/`0v`). Invalid text gives NaT.
    TToI,
    TToF,
    /// Pop a tryte, push `+`/`-`: is it in character class `k` (see `charset::Class`)?
    CharIs(u8),
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
    /// Pop `w` template values, then N: a fresh buffer of N elements of `w`
    /// cells each, every element a copy of the template (zeros of the right
    /// type: `0`, `0.0`, `""`, or a struct's fields).
    AllocT { w: u8, sym: bool },
    /// Like `pack`, for elements of `w` cells (a struct): pop `n * w` values.
    PackT { n: u32, w: u8, sym: bool },
    /// Pop index and pointer, push `n` cells of that element from cell `off` on.
    LdElem { off: u8, n: u8 },
    /// Pop `n` values, index and pointer; store them into that element from cell `off` on.
    StElem { off: u8, n: u8 },
    /// Arrays that change size (never `sym`). The element's cells are
    /// pushed before the pointer, so the VM knows how many to pop from the
    /// array's element width.
    /// `elem.. ptr` -> add the element at the end.
    Append,
    /// `ptr` -> remove the last element and push it.
    PopEl,
    /// `elem.. ptr i` -> put the element at index `i` (0..=len), shifting the rest.
    Insert,
    /// `ptr i` -> remove the element at index `i`, shifting the rest, and push it.
    Remove,
    /// `zero.. ptr n` -> grow to `n` elements with copies of `zero`, or shrink
    /// to `n` (only if every element cut off still equals `zero`).
    Resize,
    /// `ptr` -> no elements left.
    Clear,
    /// `ptr a b c d` -> swap the ranges `a..b` and `c..d` (inclusive, not overlapping).
    Replace,
    /// `v` -> an `int` hash of a number, bool, enum or text (`x.hashCode()`).
    Hash,
    /// `h x` -> `h * 31 + x`, wrapped into the `int` range (combines hashes).
    HashMix,
    /// `a b` (texts) -> `-1`, `0` or `1`: `a` comes before, is equal to, or
    /// comes after `b` (character by character).
    TCmp,
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
    /// Like `ret`, for a struct: pop `n` values and push them for the caller.
    RetN(u16),
    /// Stop with a trap; the reason is string `k`.
    Trap(u32),
    /// Pop an error (object pointer, class number) and go to the nearest
    /// handler that covers this instruction, in this function or a caller
    /// (see `ksb::Handler`); with none, the program stops.
    Throw,

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
    // Files, standard input and arguments
    /// Pop a path, push the file's contents as text (Trap if it cannot be read).
    ReadFile,
    /// Pop a path, push `+` if it is a readable file.
    FileExists,
    /// Pop text, path: write the text (UTF-8) to the file.
    WriteFile,
    /// Pop array, path: write each cell (0..255) as one byte.
    WriteBytes,
    /// Push one line of standard input, without its newline (empty at the end).
    ReadLine,
    /// Push `+` if standard input has nothing left.
    Eof,
    /// Push the number of program arguments.
    ArgCount,
    /// Pop i, push argument i as text.
    Arg,
    /// Pop a code and stop the program with it.
    Exit,
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
            IsNaT => 0x07,

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
            Soft => 0x1d,
            Hard => 0x1e,
            ToBool => 0x1f,

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
            TCat => 0x39,
            TEq => 0x3a,
            TSub => 0x3b,
            TToI => 0x3c,
            TToF => 0x3d,
            CharIs(_) => 0x3e,

            Alloc => 0x40,
            LdIdx => 0x41,
            StIdx => 0x42,
            Purge(_) => 0x43,
            AllocZ => 0x44,
            Pack { .. } => 0x45,
            Append => 0x70,
            PopEl => 0x71,
            Insert => 0x72,
            Remove => 0x73,
            Resize => 0x74,
            Clear => 0x75,
            Replace => 0x76,
            Hash => 0x77,
            HashMix => 0x78,
            TCmp => 0x79,
            AllocT { .. } => 0x46,
            PackT { .. } => 0x47,
            LdElem { .. } => 0x48,
            StElem { .. } => 0x49,

            Jmp(_) => 0x50,
            JNat(_) => 0x51,
            Br3 { .. } => 0x52,
            Call { .. } => 0x53,
            Ret => 0x54,
            RetN(_) => 0x56,
            Trap(_) => 0x55,
            Throw => 0x57,

            Input => 0x60,
            Emit => 0x61,
            Print => 0x62,
            Show => 0x63,
            Dump(_) => 0x64,
            ShowT => 0x65,
            ShowV => 0x66,
            ReadFile => 0x67,
            FileExists => 0x68,
            WriteFile => 0x69,
            WriteBytes => 0x6a,
            ReadLine => 0x6b,
            Eof => 0x6c,
            ArgCount => 0x6d,
            Arg => 0x6e,
            Exit => 0x6f,
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
            IsNaT => "isnat",
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
            Soft => "soft",
            Hard => "hard",
            ToBool => "tobool",
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
            TCat => "tcat",
            TEq => "teq",
            TSub => "tsub",
            TToI => "ttoi",
            TToF => "ttof",
            CharIs(_) => "charis",
            TIdx => "tidx",
            Narrow6 => "narrow6",
            Narrow1 => "narrow1",
            IToF => "itof",
            FToI => "ftoi",
            Alloc => "alloc",
            AllocZ => "allocz",
            Pack { .. } => "pack",
            Append => "append",
            PopEl => "popel",
            Insert => "insert",
            Remove => "remove",
            Resize => "resize",
            Clear => "clear",
            Replace => "replace",
            Hash => "hash",
            HashMix => "hashmix",
            TCmp => "tcmp",
            AllocT { .. } => "alloct",
            PackT { .. } => "packt",
            LdElem { .. } => "ldelem",
            StElem { .. } => "stelem",
            LdIdx => "ldidx",
            StIdx => "stidx",
            Purge(_) => "purge",
            Jmp(_) => "jmp",
            JNat(_) => "jnat",
            Br3 { .. } => "br3",
            Call { .. } => "call",
            Ret => "ret",
            Throw => "throw",
            RetN(_) => "retn",
            Trap(_) => "trap",
            Input => "input",
            Emit => "emit",
            Print => "print",
            Show => "show",
            ShowT => "showt",
            ShowV => "showv",
            ReadFile => "readf",
            FileExists => "fexists",
            WriteFile => "writef",
            WriteBytes => "writeb",
            ReadLine => "readln",
            Eof => "eof",
            ArgCount => "argc",
            Arg => "arg",
            Exit => "exit",
            Dump(_) => "dump",
        }
    }

    /// Encoded size in bytes.
    pub fn size(self) -> usize {
        use Op::*;
        1 + match self {
            PushC(_) | PushR(_) | PushS(_) | Jmp(_) | JNat(_) | Dump(_) | Trap(_) => 4,
            Test(_) | CharIs(_) => 1,
            Load(_) | Store(_) | Move(_) | Purge(_) | RetN(_) | AllocT { .. } | LdElem { .. } | StElem { .. } => 2,
            PackT { .. } => 6,
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
            Test(m) | CharIs(m) => out.push(m),
            RetN(n) => out.extend_from_slice(&n.to_le_bytes()),
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
            AllocT { w, sym } => out.extend_from_slice(&[w, u8::from(sym)]),
            PackT { n, w, sym } => {
                out.extend_from_slice(&n.to_le_bytes());
                out.extend_from_slice(&[w, u8::from(sym)]);
            }
            LdElem { off, n } | StElem { off, n } => out.extend_from_slice(&[off, n]),
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
            0x07 => IsNaT,
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
            0x1d => Soft,
            0x1e => Hard,
            0x1f => ToBool,
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
            0x39 => TCat,
            0x3a => TEq,
            0x3b => TSub,
            0x3c => TToI,
            0x3d => TToF,
            0x3e => match r.u8()? {
                k @ 0..=4 => CharIs(k),
                _ => return Err(DecodeError::BadOpcode { at, byte }),
            },
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
            0x46 => {
                let w = r.u8()?;
                match r.u8()? {
                    s @ 0..=1 => AllocT { w, sym: s == 1 },
                    _ => return Err(DecodeError::BadOpcode { at, byte }),
                }
            }
            0x47 => {
                let n = r.u32()?;
                let w = r.u8()?;
                match r.u8()? {
                    s @ 0..=1 => PackT { n, w, sym: s == 1 },
                    _ => return Err(DecodeError::BadOpcode { at, byte }),
                }
            }
            0x48 => LdElem { off: r.u8()?, n: r.u8()? },
            0x70 => Append,
            0x71 => PopEl,
            0x72 => Insert,
            0x73 => Remove,
            0x74 => Resize,
            0x75 => Clear,
            0x76 => Replace,
            0x77 => Hash,
            0x78 => HashMix,
            0x79 => TCmp,
            0x49 => StElem { off: r.u8()?, n: r.u8()? },
            0x41 => LdIdx,
            0x42 => StIdx,
            0x43 => Purge(r.u16()?),
            0x50 => Jmp(r.u32()?),
            0x51 => JNat(r.u32()?),
            0x52 => Br3 { neg: r.u32()?, zero: r.u32()?, pos: r.u32()? },
            0x53 => Call { f: r.u32()?, argc: r.u8()? },
            0x54 => Ret,
            0x55 => Trap(r.u32()?),
            0x56 => RetN(r.u16()?),
            0x57 => Throw,
            0x60 => Input,
            0x61 => Emit,
            0x62 => Print,
            0x63 => Show,
            0x64 => Dump(r.u32()?),
            0x65 => ShowT,
            0x66 => ShowV,
            0x67 => ReadFile,
            0x68 => FileExists,
            0x69 => WriteFile,
            0x6a => WriteBytes,
            0x6b => ReadLine,
            0x6c => Eof,
            0x6d => ArgCount,
            0x6e => Arg,
            0x6f => Exit,
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
            CharIs(k) => write!(f, "{m:<7} {}", ["digit", "letter", "upper", "lower", "space"].get(usize::from(k)).unwrap_or(&"?")),
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
            RetN(n) => write!(f, "{m:<7} {n}"),
            Pack { n, sym } => write!(f, "{m:<7} {n}{}", if sym { ", sym" } else { "" }),
            AllocT { w, sym } => write!(f, "{m:<7} w{w}{}", if sym { ", sym" } else { "" }),
            PackT { n, w, sym } => write!(f, "{m:<7} {n} w{w}{}", if sym { ", sym" } else { "" }),
            LdElem { off, n } | StElem { off, n } => write!(f, "{m:<7} +{off} n{n}"),
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
            PushC(7), PushR(70_000), PushS(0), Load(4), Store(9), Move(0), Pop, IsNaT,
            Add, Sub, Mul, Div, Mod, Pow, Neg, Shl3, Shr3, Cmp3, KAnd, KOr, Test(0b101), Soft, Hard, ToBool, Trap(4),
            FAdd, FSub, FMul, FDiv, FPow, FNeg, FCmp3,
            TLen, TIdx, Narrow6, Narrow1, IToF, FToI, TStr, TStrT, TStrV, TCat, TEq, TSub, TToI, TToF, CharIs(2),
            Alloc, LdIdx, StIdx, Purge(8), AllocZ, Pack { n: 3, sym: true }, AllocT { w: 2, sym: false }, PackT { n: 3, w: 2, sym: true }, LdElem { off: 1, n: 2 }, StElem { off: 0, n: 1 }, Append, PopEl, Insert, Remove, Resize, Clear, Replace, Hash, HashMix, TCmp,
            Jmp(12), JNat(u32::MAX), Br3 { neg: 1, zero: 2, pos: 3 }, Call { f: 2, argc: 3 }, Ret, RetN(3), Throw,
            Input, Emit, Print, Show, Dump(5), ShowT, ShowV,
            ReadFile, FileExists, WriteFile, WriteBytes, ReadLine, Eof, ArgCount, Arg, Exit,
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
