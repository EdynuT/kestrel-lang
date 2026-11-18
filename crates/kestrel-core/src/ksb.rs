//! The `.ksb` bytecode file (SPEC §10): writing, reading and verification.
//!
//! Reading always verifies the module, so the VM can trust that every jump
//! lands on an instruction boundary and every index points at something real.

use std::fmt;

use crate::bet::Word27;
use crate::isa::{DecodeError, Op};
use crate::KSB_MAGIC;

pub const KSB_VERSION: u16 = 1;
/// Flag bit 0: each function carries the names of its local slots.
pub const FLAG_DEBUG_NAMES: u16 = 1;

const HEADER_LEN: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Module {
    /// Index of `main` in `functions`.
    pub entry: u32,
    /// Constant pool: words and reals in packed BET form.
    pub consts: Vec<Word27>,
    /// String pool: `text` literals and `dump` labels.
    pub strings: Vec<String>,
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Function {
    pub name: String,
    /// Parameters occupy the first `params` local slots.
    pub params: u8,
    pub locals: u16,
    pub code: Vec<u8>,
    /// Optional debug names, one per local slot (empty if absent).
    pub local_names: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum KsbError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    TrailingBytes,
    BadUtf8,
    BadEntry(u32),
    /// A problem inside a function's code.
    Code { func: String, at: usize, problem: CodeProblem },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodeProblem {
    Decode(DecodeError),
    JumpOutside(u32),
    BadConst(u32),
    BadString(u32),
    BadFunction(u32),
    BadSlot(u16),
    ArgcMismatch { expected: u8, got: u8 },
    ParamsExceedLocals,
    BadDebugNames,
}

impl fmt::Display for KsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KsbError::BadMagic => f.write_str("not a .ksb file (bad magic)"),
            KsbError::UnsupportedVersion(v) => write!(f, "unsupported .ksb version {v}"),
            KsbError::Truncated => f.write_str("file is truncated"),
            KsbError::TrailingBytes => f.write_str("unexpected bytes after the last function"),
            KsbError::BadUtf8 => f.write_str("string is not valid UTF-8"),
            KsbError::BadEntry(e) => write!(f, "entry function #{e} does not exist"),
            KsbError::Code { func, at, problem } => write!(f, "in {func}() at byte {at}: {problem:?}"),
        }
    }
}

impl std::error::Error for KsbError {}

impl Module {
    /// Serializes the module. Debug names are written only if every
    /// function has exactly one name per local.
    pub fn to_bytes(&self) -> Vec<u8> {
        let debug = !self.functions.is_empty()
            && self.functions.iter().all(|f| f.local_names.len() == usize::from(f.locals));
        let flags = if debug { FLAG_DEBUG_NAMES } else { 0 };

        let mut out = Vec::new();
        out.extend_from_slice(&KSB_MAGIC);
        out.extend_from_slice(&KSB_VERSION.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        for n in [self.entry, len32(&self.consts), len32(&self.strings), len32(&self.functions)] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        for c in &self.consts {
            out.extend_from_slice(&c.bits().to_le_bytes());
        }
        for s in &self.strings {
            out.extend_from_slice(&len32(s.as_bytes()).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        for f in &self.functions {
            write_name16(&mut out, &f.name);
            out.push(f.params);
            out.extend_from_slice(&f.locals.to_le_bytes());
            out.extend_from_slice(&len32(&f.code).to_le_bytes());
            out.extend_from_slice(&f.code);
            if debug {
                for n in &f.local_names {
                    write_name16(&mut out, n);
                }
            }
        }
        out
    }

    /// Parses and verifies a module.
    pub fn from_bytes(bytes: &[u8]) -> Result<Module, KsbError> {
        let mut r = Cursor { bytes, pos: 0 };
        if r.take(4)? != KSB_MAGIC {
            return Err(KsbError::BadMagic);
        }
        let version = r.u16()?;
        if version != KSB_VERSION {
            return Err(KsbError::UnsupportedVersion(version));
        }
        let flags = r.u16()?;
        let entry = r.u32()?;
        let (n_consts, n_strings, n_funcs) = (r.u32()?, r.u32()?, r.u32()?);
        debug_assert_eq!(r.pos, HEADER_LEN);

        let mut m = Module { entry, ..Default::default() };
        for _ in 0..n_consts {
            let bits = u64::from_le_bytes(r.array()?);
            m.consts.push(Word27::from_bits(bits).ok_or(KsbError::Truncated)?);
        }
        for _ in 0..n_strings {
            let len = r.u32()? as usize;
            m.strings.push(r.string(len)?);
        }
        for _ in 0..n_funcs {
            let name_len = usize::from(r.u16()?);
            let mut f = Function { name: r.string(name_len)?, ..Default::default() };
            f.params = r.take(1)?[0];
            f.locals = r.u16()?;
            let code_len = r.u32()? as usize;
            f.code = r.take(code_len)?.to_vec();
            if flags & FLAG_DEBUG_NAMES != 0 {
                for _ in 0..f.locals {
                    let len = usize::from(r.u16()?);
                    f.local_names.push(r.string(len)?);
                }
            }
            m.functions.push(f);
        }
        if r.pos != bytes.len() {
            return Err(KsbError::TrailingBytes);
        }
        m.verify()?;
        Ok(m)
    }

    /// Checks indices, slots, call arity and jump targets in every function.
    pub fn verify(&self) -> Result<(), KsbError> {
        if self.entry as usize >= self.functions.len() {
            return Err(KsbError::BadEntry(self.entry));
        }
        for f in &self.functions {
            let fail = |at: usize, problem| KsbError::Code { func: f.name.clone(), at, problem };
            if u16::from(f.params) > f.locals {
                return Err(fail(0, CodeProblem::ParamsExceedLocals));
            }
            if !f.local_names.is_empty() && f.local_names.len() != usize::from(f.locals) {
                return Err(fail(0, CodeProblem::BadDebugNames));
            }
            // First pass: decode everything and record instruction starts.
            let mut starts = vec![false; f.code.len() + 1];
            let mut ops = Vec::new();
            let mut at = 0;
            while at < f.code.len() {
                let (op, size) = Op::decode(&f.code, at).map_err(|e| fail(at, CodeProblem::Decode(e)))?;
                starts[at] = true;
                ops.push((at, op));
                at += size;
            }
            // Second pass: every operand must point at something that exists.
            for (at, op) in ops {
                let check_jump = |a: u32| {
                    if starts.get(a as usize) == Some(&true) {
                        Ok(())
                    } else {
                        Err(fail(at, CodeProblem::JumpOutside(a)))
                    }
                };
                let check_slot = |s: u16| {
                    if s < f.locals { Ok(()) } else { Err(fail(at, CodeProblem::BadSlot(s))) }
                };
                match op {
                    Op::PushC(k) | Op::PushR(k) if k as usize >= self.consts.len() => {
                        return Err(fail(at, CodeProblem::BadConst(k)));
                    }
                    Op::PushS(k) | Op::Dump(k) if k as usize >= self.strings.len() => {
                        return Err(fail(at, CodeProblem::BadString(k)));
                    }
                    Op::Load(s) | Op::Store(s) | Op::Move(s) | Op::Purge(s) => check_slot(s)?,
                    Op::Jmp(a) | Op::JNat(a) => check_jump(a)?,
                    Op::Br3 { neg, zero, pos } => {
                        check_jump(neg)?;
                        check_jump(zero)?;
                        check_jump(pos)?;
                    }
                    Op::Call { f: callee, argc } => {
                        let target = self
                            .functions
                            .get(callee as usize)
                            .ok_or_else(|| fail(at, CodeProblem::BadFunction(callee)))?;
                        if target.params != argc {
                            return Err(fail(at, CodeProblem::ArgcMismatch { expected: target.params, got: argc }));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Human-readable listing of every function, for `ksc --disasm`.
    pub fn disassemble(&self) -> String {
        let mut s = String::new();
        for (i, f) in self.functions.iter().enumerate() {
            let entry = if i as u32 == self.entry { "  (entry)" } else { "" };
            s += &format!("fn#{i} {}  params={} locals={}{entry}\n", f.name, f.params, f.locals);
            let mut at = 0;
            while at < f.code.len() {
                match Op::decode(&f.code, at) {
                    Ok((op, size)) => {
                        s += &format!("  {at:>5}: {op}{}\n", self.annotate(f, op));
                        at += size;
                    }
                    Err(e) => {
                        s += &format!("  {at:>5}: <{e}>\n");
                        break;
                    }
                }
            }
        }
        s
    }

    /// Trailing comment with the constant, string, callee or variable name.
    fn annotate(&self, f: &Function, op: Op) -> String {
        let note = match op {
            Op::PushC(k) => self.consts.get(k as usize).map(|c| crate::Word::from(*c).to_string()),
            Op::PushR(k) => self.consts.get(k as usize).map(|c| crate::Real::from(*c).to_string()),
            Op::PushS(k) | Op::Dump(k) => self.strings.get(k as usize).map(|s| format!("{s:?}")),
            Op::Call { f: callee, .. } => self.functions.get(callee as usize).map(|g| g.name.clone()),
            Op::Load(s) | Op::Store(s) | Op::Move(s) | Op::Purge(s) => {
                f.local_names.get(usize::from(s)).cloned()
            }
            _ => None,
        };
        note.map(|n| format!("    // {n}")).unwrap_or_default()
    }
}

fn len32<T>(items: &[T]) -> u32 {
    u32::try_from(items.len()).expect("pool larger than 4 GiB")
}

fn write_name16(out: &mut Vec<u8>, s: &str) {
    let len = u16::try_from(s.len()).expect("name longer than 64 KiB");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], KsbError> {
        let end = self.pos.checked_add(n).ok_or(KsbError::Truncated)?;
        let s = self.bytes.get(self.pos..end).ok_or(KsbError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], KsbError> {
        Ok(self.take(N)?.try_into().expect("slice has length N"))
    }

    fn u16(&mut self) -> Result<u16, KsbError> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, KsbError> {
        self.array().map(u32::from_le_bytes)
    }

    fn string(&mut self, len: usize) -> Result<String, KsbError> {
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| KsbError::BadUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Real, Word};

    fn code(ops: &[Op]) -> Vec<u8> {
        let mut c = Vec::new();
        for op in ops {
            op.encode(&mut c);
        }
        c
    }

    /// main() { x := 8; print(sum(x)); } with sum(v) = v + v.
    fn sample() -> Module {
        let main = Function {
            name: "main".into(),
            params: 0,
            locals: 1,
            code: code(&[
                Op::PushC(0),
                Op::Store(0),
                Op::Load(0),
                Op::Call { f: 1, argc: 1 },
                Op::Show,
                Op::PushR(1),
                Op::Show,
                Op::PushC(2),
                Op::Ret,
            ]),
            local_names: vec!["x".into()],
        };
        let sum = Function {
            name: "sum".into(),
            params: 1,
            locals: 1,
            code: code(&[Op::Load(0), Op::Load(0), Op::Add, Op::Ret]),
            local_names: vec!["v".into()],
        };
        Module {
            entry: 0,
            consts: vec![
                Word27::from(Word::new(8)),
                Word27::from(Real::from_decimal("3", "14").unwrap()),
                Word27::from(Word::ZERO),
            ],
            strings: vec!["Hello".into()],
            functions: vec![main, sum],
        }
    }

    #[test]
    fn roundtrip() {
        let m = sample();
        let bytes = m.to_bytes();
        assert_eq!(&bytes[..4], b"K3B\x01");
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), FLAG_DEBUG_NAMES);
        assert_eq!(Module::from_bytes(&bytes), Ok(m));
    }

    #[test]
    fn roundtrip_without_debug_names() {
        let mut m = sample();
        for f in &mut m.functions {
            f.local_names.clear();
        }
        assert_eq!(Module::from_bytes(&m.to_bytes()), Ok(m));
    }

    #[test]
    fn header_errors() {
        let bytes = sample().to_bytes();
        assert_eq!(Module::from_bytes(b"ELF\x7f"), Err(KsbError::BadMagic));
        let mut v2 = bytes.clone();
        v2[4] = 2;
        assert_eq!(Module::from_bytes(&v2), Err(KsbError::UnsupportedVersion(2)));
        assert_eq!(Module::from_bytes(&bytes[..bytes.len() - 1]), Err(KsbError::Truncated));
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(Module::from_bytes(&extra), Err(KsbError::TrailingBytes));
    }

    fn problem(m: &Module) -> CodeProblem {
        match m.verify() {
            Err(KsbError::Code { problem, .. }) => problem,
            other => panic!("expected a code problem, got {other:?}"),
        }
    }

    #[test]
    fn verifier_catches_bad_code() {
        let mut m = sample();
        m.functions[1].code = code(&[Op::Jmp(2), Op::Ret]); // offset 2 is inside the jmp
        assert_eq!(problem(&m), CodeProblem::JumpOutside(2));

        let mut m = sample();
        m.functions[1].code = code(&[Op::PushC(99), Op::Ret]);
        assert_eq!(problem(&m), CodeProblem::BadConst(99));

        let mut m = sample();
        m.functions[1].code = code(&[Op::Load(1), Op::Ret]);
        assert_eq!(problem(&m), CodeProblem::BadSlot(1));

        let mut m = sample();
        m.functions[0].code = code(&[Op::Call { f: 1, argc: 3 }, Op::Ret]);
        assert_eq!(problem(&m), CodeProblem::ArgcMismatch { expected: 1, got: 3 });

        let mut m = sample();
        m.functions[0].code = vec![0xee];
        assert!(matches!(problem(&m), CodeProblem::Decode(_)));

        let mut m = sample();
        m.entry = 5;
        assert_eq!(m.verify(), Err(KsbError::BadEntry(5)));
    }

    #[test]
    fn disassembly() {
        let text = sample().disassemble();
        assert!(text.contains("fn#0 main  params=0 locals=1  (entry)"), "{text}");
        assert!(text.contains("pushc   #0    // 8"), "{text}");
        assert!(text.contains("pushr   #1    // 3.14"), "{text}");
        assert!(text.contains("store   R-4    // x"), "{text}");
        assert!(text.contains("call    fn#1, 1    // sum"), "{text}");
    }
}
