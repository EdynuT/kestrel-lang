//! The `.ksb` bytecode file (SPEC §10): writing, reading and verification.
//!
//! Reading always verifies the module, so the VM can trust that every jump
//! lands on an instruction boundary and every index points at something real.

use std::fmt;

use crate::bet::Word27;
use crate::isa::{DecodeError, Op};
use crate::KSB_MAGIC;

pub const KSB_VERSION: u16 = 4;
/// Flag bit 0: each function carries the names of its local slots.
pub const FLAG_DEBUG_NAMES: u16 = 1;
/// Flag bit 1: an object file (one `.ks` file, not yet linked): it ends
/// with a [`Link`] section.
pub const FLAG_OBJECT: u16 = 2;

const HEADER_LEN: usize = 24;

/// The end of an executable made by `ksc`: a copy of `ksvm` with the
/// program's `.ksb` after it, then the `.ksb`'s size (u64) and this mark.
pub const EXE_MARK: [u8; 8] = *b"K3B-EXE1";

/// `runtime` (the `ksvm` executable) with `program` stuck to its end.
pub fn bundle(runtime: &[u8], program: &[u8]) -> Vec<u8> {
    let mut out = runtime.to_vec();
    out.extend_from_slice(program);
    out.extend_from_slice(&(program.len() as u64).to_le_bytes());
    out.extend_from_slice(&EXE_MARK);
    out
}

/// The program stuck to the end of an executable by [`bundle`], if any.
pub fn bundled(exe: &[u8]) -> Option<&[u8]> {
    let rest = exe.strip_suffix(&EXE_MARK)?;
    let (rest, len) = rest.split_at_checked(rest.len().checked_sub(8)?)?;
    let len = usize::try_from(u64::from_le_bytes(len.try_into().ok()?)).ok()?;
    rest.get(rest.len().checked_sub(len)?..)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Module {
    /// Index of `main` in `functions`.
    pub entry: u32,
    /// Constant pool: words and reals in packed BET form.
    pub consts: Vec<Word27>,
    /// String pool: `text` literals and `dump` labels.
    pub strings: Vec<String>,
    pub functions: Vec<Function>,
    /// What the linker needs, in an object file (`None` in a program).
    pub link: Option<Link>,
}

/// What an object file tells the linker, besides its functions.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Link {
    /// The classes it defines, with their numbers (two classes must never
    /// share a number).
    pub classes: Vec<(String, i64)>,
    /// How its classes answer interface calls: for the dispatch function
    /// `$dispatch.I.m`, objects of class number `tag` run `function`;
    /// `whole` passes the receiver as the interface (pointer and class
    /// number), for a default method, instead of just the pointer.
    pub impls: Vec<Impl>,
    /// `$burnI.I` frees an object of class number `tag` with `function`
    /// (`$burn.C`); classes not listed own nothing and are just freed.
    pub burns: Vec<(String, i64, String)>,
    /// For `ksc`'s build cache: what this object was made from.
    pub stamp: Vec<(String, u64)>,
    /// The file's declarations, as Kestrel text (its source with the bodies
    /// of plain procs and methods emptied): what other files need to use
    /// it, so a library works without its `.ks`, as C's `.h` files.
    pub decls: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impl {
    pub dispatch: String,
    pub tag: i64,
    pub function: String,
    pub whole: bool,
}

/// Where a function's code comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    /// Its code is here.
    #[default]
    Code,
    /// Its code is here, and other objects may have the same one (a copy of
    /// a generic, `Box<int>.get`): the linker keeps one.
    Shared,
    /// Defined in another object; no code.
    Import,
    /// An interface call (`$dispatch.I.m`), written by the linker from
    /// every object's [`Impl`]s. `ret` is how many values it returns.
    Dispatch { ret: u16 },
    /// Frees an object seen through an interface (`$burnI.I`), written by the linker.
    BurnDispatch,
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
    pub kind: Kind,
    /// Where a `throw` (or a failure of the VM) inside this function goes,
    /// innermost first: like the exception table of a Java `.class`.
    pub handlers: Vec<Handler>,
    /// The `.ks` file it was written in (empty if unknown), for tracebacks.
    pub file: String,
    /// Which line of `file` each part of the code comes from: from byte
    /// offset `.0` on (until the next entry), line `.1`. In offset order.
    pub lines: Vec<(u32, u32)>,
}

impl Function {
    /// The line of `file` the instruction at byte `at` comes from.
    pub fn line_at(&self, at: usize) -> Option<u32> {
        let k = self.lines.partition_point(|&(off, _)| off as usize <= at);
        k.checked_sub(1).map(|k| self.lines[k].1)
    }
}

/// Code at `start..end` (byte offsets) that fails goes to `target`, with
/// the error (object pointer, class number) as the only values on the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handler {
    pub start: u32,
    pub end: u32,
    pub target: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum KsbError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    TrailingBytes,
    BadUtf8,
    BadEntry(u32),
    BadKind(u8),
    /// A program (not an object) with a function that has no code.
    Unlinked(String),
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
    /// A handler whose range or target is not at an instruction.
    BadHandler,
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
            KsbError::BadKind(k) => write!(f, "unknown function kind {k}"),
            KsbError::Unlinked(name) => write!(f, "`{name}` has no code: this is an unlinked object file (link it with ksc)"),
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
        let flags = if debug { FLAG_DEBUG_NAMES } else { 0 } | if self.link.is_some() { FLAG_OBJECT } else { 0 };

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
            match f.kind {
                Kind::Code => out.push(0),
                Kind::Shared => out.push(1),
                Kind::Import => out.push(2),
                Kind::Dispatch { ret } => {
                    out.push(3);
                    out.extend_from_slice(&ret.to_le_bytes());
                }
                Kind::BurnDispatch => out.push(4),
            }
            out.extend_from_slice(&len32(&f.code).to_le_bytes());
            out.extend_from_slice(&f.code);
            out.extend_from_slice(&len32(&f.handlers).to_le_bytes());
            for h in &f.handlers {
                for n in [h.start, h.end, h.target] {
                    out.extend_from_slice(&n.to_le_bytes());
                }
            }
            write_name16(&mut out, &f.file);
            out.extend_from_slice(&len32(&f.lines).to_le_bytes());
            for (off, line) in &f.lines {
                out.extend_from_slice(&off.to_le_bytes());
                out.extend_from_slice(&line.to_le_bytes());
            }
            if debug {
                for n in &f.local_names {
                    write_name16(&mut out, n);
                }
            }
        }
        if let Some(link) = &self.link {
            out.extend_from_slice(&len32(&link.classes).to_le_bytes());
            for (name, tag) in &link.classes {
                write_name16(&mut out, name);
                out.extend_from_slice(&tag.to_le_bytes());
            }
            out.extend_from_slice(&len32(&link.impls).to_le_bytes());
            for i in &link.impls {
                write_name16(&mut out, &i.dispatch);
                out.extend_from_slice(&i.tag.to_le_bytes());
                write_name16(&mut out, &i.function);
                out.push(u8::from(i.whole));
            }
            out.extend_from_slice(&len32(&link.burns).to_le_bytes());
            for (dispatch, tag, function) in &link.burns {
                write_name16(&mut out, dispatch);
                out.extend_from_slice(&tag.to_le_bytes());
                write_name16(&mut out, function);
            }
            out.extend_from_slice(&len32(&link.stamp).to_le_bytes());
            for (name, hash) in &link.stamp {
                write_name16(&mut out, name);
                out.extend_from_slice(&hash.to_le_bytes());
            }
            out.extend_from_slice(&len32(link.decls.as_bytes()).to_le_bytes());
            out.extend_from_slice(link.decls.as_bytes());
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
            f.kind = match r.take(1)?[0] {
                0 => Kind::Code,
                1 => Kind::Shared,
                2 => Kind::Import,
                3 => Kind::Dispatch { ret: r.u16()? },
                4 => Kind::BurnDispatch,
                k => return Err(KsbError::BadKind(k)),
            };
            let code_len = r.u32()? as usize;
            f.code = r.take(code_len)?.to_vec();
            for _ in 0..r.u32()? {
                f.handlers.push(Handler { start: r.u32()?, end: r.u32()?, target: r.u32()? });
            }
            f.file = r.name16()?;
            for _ in 0..r.u32()? {
                f.lines.push((r.u32()?, r.u32()?));
            }
            if flags & FLAG_DEBUG_NAMES != 0 {
                for _ in 0..f.locals {
                    let len = usize::from(r.u16()?);
                    f.local_names.push(r.string(len)?);
                }
            }
            m.functions.push(f);
        }
        if flags & FLAG_OBJECT != 0 {
            let mut link = Link::default();
            for _ in 0..r.u32()? {
                let name = r.name16()?;
                link.classes.push((name, i64::from_le_bytes(r.array()?)));
            }
            for _ in 0..r.u32()? {
                let dispatch = r.name16()?;
                let tag = i64::from_le_bytes(r.array()?);
                let function = r.name16()?;
                let whole = r.take(1)?[0] != 0;
                link.impls.push(Impl { dispatch, tag, function, whole });
            }
            for _ in 0..r.u32()? {
                let dispatch = r.name16()?;
                let tag = i64::from_le_bytes(r.array()?);
                link.burns.push((dispatch, tag, r.name16()?));
            }
            for _ in 0..r.u32()? {
                let name = r.name16()?;
                link.stamp.push((name, u64::from_le_bytes(r.array()?)));
            }
            let len = r.u32()? as usize;
            link.decls = r.string(len)?;
            m.link = Some(link);
        }
        if r.pos != bytes.len() {
            return Err(KsbError::TrailingBytes);
        }
        m.verify()?;
        Ok(m)
    }

    /// Checks indices, slots, call arity and jump targets in every function.
    pub fn verify(&self) -> Result<(), KsbError> {
        if self.entry as usize >= self.functions.len() && !(self.link.is_some() && self.entry == 0) {
            return Err(KsbError::BadEntry(self.entry));
        }
        for f in &self.functions {
            if !matches!(f.kind, Kind::Code | Kind::Shared) {
                if self.link.is_none() {
                    return Err(KsbError::Unlinked(f.name.clone()));
                }
                continue;
            }
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
                    Op::PushS(k) | Op::Dump(k) | Op::Trap(k) if k as usize >= self.strings.len() => {
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
            for h in &f.handlers {
                let edge = |a: u32| starts.get(a as usize) == Some(&true) || a as usize == f.code.len();
                if h.start > h.end || !edge(h.start) || !edge(h.end) || starts.get(h.target as usize) != Some(&true) {
                    return Err(fail(h.target as usize, CodeProblem::BadHandler));
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
            for h in &f.handlers {
                s += &format!("  catch @{}..@{} -> @{}\n", h.start, h.end, h.target);
            }
        }
        s
    }

    /// Trailing comment with the constant, string, callee or variable name.
    fn annotate(&self, f: &Function, op: Op) -> String {
        let note = match op {
            Op::PushC(k) => self.consts.get(k as usize).map(|c| crate::Word::from(*c).to_string()),
            Op::PushR(k) => self.consts.get(k as usize).map(|c| crate::Real::from(*c).to_string()),
            Op::PushS(k) | Op::Dump(k) | Op::Trap(k) => self.strings.get(k as usize).map(|s| format!("{s:?}")),
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

    fn name16(&mut self) -> Result<String, KsbError> {
        let len = usize::from(self.u16()?);
        self.string(len)
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

    /// main() { x: int = 8; print(sum(x)); } with sum(v) = v + v.
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
            kind: Kind::Code,
            handlers: Vec::new(),
            file: String::new(),
            lines: Vec::new(),
        };
        let sum = Function {
            name: "sum".into(),
            params: 1,
            locals: 1,
            code: code(&[Op::Load(0), Op::Load(0), Op::Add, Op::Ret]),
            local_names: vec!["v".into()],
            kind: Kind::Code,
            // load@0 load@3 add@6 ret@7
            handlers: vec![Handler { start: 0, end: 6, target: 7 }],
            file: "sum.ks".into(),
            lines: vec![(0, 2), (6, 3)],
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
            link: None,
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
    fn bundles() {
        let program = sample().to_bytes();
        let exe = bundle(b"\x7fELF...runtime", &program);
        assert_eq!(bundled(&exe), Some(&program[..]));
        assert_eq!(bundled(b"\x7fELF...runtime"), None);
        assert_eq!(bundled(&EXE_MARK), None);
    }

    #[test]
    fn object_roundtrip() {
        let mut m = sample();
        m.functions.push(Function { name: "other".into(), params: 2, locals: 2, kind: Kind::Import, ..Default::default() });
        m.functions.push(Function { name: "$dispatch.I.m".into(), params: 2, locals: 2, kind: Kind::Dispatch { ret: 1 }, ..Default::default() });
        for f in &mut m.functions[2..] {
            f.local_names = vec![String::new(); 2];
        }
        let link = Link {
            classes: vec![("Circle".into(), 77)],
            impls: vec![Impl { dispatch: "$dispatch.I.m".into(), tag: 77, function: "Circle.m".into(), whole: false }],
            burns: vec![("$burnI.I".into(), 77, "$burn.Circle".into())],
            stamp: vec![("source".into(), 42)],
            decls: "public proc other(a: int, b: int) { }".into(),
        };
        m.link = Some(link);
        assert_eq!(Module::from_bytes(&m.to_bytes()), Ok(m.clone()));
        // A program must be linked: no function without code.
        m.link = None;
        assert_eq!(Module::from_bytes(&m.to_bytes()), Err(KsbError::Unlinked("other".into())));
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
        v2[4] = 9;
        assert_eq!(Module::from_bytes(&v2), Err(KsbError::UnsupportedVersion(9)));
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
        m.functions[1].handlers[0].target = 2; // inside the first load
        assert_eq!(problem(&m), CodeProblem::BadHandler);
        m.functions[1].handlers[0] = Handler { start: 6, end: 3, target: 7 };
        assert_eq!(problem(&m), CodeProblem::BadHandler);
        m = sample();
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
