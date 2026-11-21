//! The BET virtual machine: runs a verified `.ksb` [`Module`].
//!
//! Values are tagged ([`Value`]), the heap hands out generation-checked
//! pointers, and any Trap zeroes the whole heap and every register before
//! the machine stops (SPEC §7).

use std::fmt;
use std::io::{BufRead, Write};

use kestrel_core::bet::{parse_hepta, parse_trits};
use kestrel_core::charset::{Class, char_to_tryte, tryte_to_char};
use kestrel_core::isa::{Op, slot_name};
use kestrel_core::{Module, Real, Word, Word27};

/// Deepest call chain before the VM traps with a stack overflow.
pub use kestrel_core::limits::MAX_CALL_DEPTH as MAX_DEPTH;
/// Largest buffer `claim` may create, in cells.
pub const MAX_CELLS: i64 = 1 << 24;
/// Limit on characters in texts made at run time that are alive at once
/// (each text also counts 1 for itself).
pub const MAX_TEXT_CHARS: usize = 1 << 24;
/// Text memory made before the first collection of unused texts.
const TEXT_GC_START: usize = 1 << 16;

/// A value on the stack or in a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Word(Word),
    Real(Real),
    /// Index into the module's string pool.
    Text(u32),
    Ptr(Ptr),
}

impl From<Word> for Value {
    fn from(w: Word) -> Value {
        Value::Word(w)
    }
}

impl From<Real> for Value {
    fn from(r: Real) -> Value {
        Value::Real(r)
    }
}

impl Value {
    pub const NAT: Value = Value::Word(Word::NAT);

    fn is_nat(self) -> bool {
        match self {
            Value::Word(w) => w.is_nat(),
            Value::Real(r) => r.is_nat(),
            _ => false,
        }
    }
}

/// Generation-checked pointer to the center cell of a heap block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ptr {
    block: u32,
    generation: u32,
}

/// A structural failure (SPEC §7). The heap was already zeroed when this is returned.
#[derive(Debug, PartialEq, Eq)]
pub struct Trap {
    pub func: String,
    pub at: usize,
    pub reason: String,
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TRAP in {}() at byte {}: {}", self.func, self.at, self.reason)
    }
}

impl std::error::Error for Trap {}

struct Block {
    cells: Vec<Value>,
    /// Cells per element: 1, or the number of fields of a struct.
    w: usize,
    /// Lowest valid index: `-k` for a symmetric buffer, `0` otherwise.
    lo: i64,
    generation: u32,
    live: bool,
}

struct Frame {
    func: usize,
    pc: usize,
    locals: Vec<Value>,
    /// Set by `soft` (an `on NaT` line): failed arithmetic gives NaT instead of trapping.
    soft: bool,
}

pub struct Vm<'m, R, W> {
    module: &'m Module,
    /// String pool split into chars, for `len` and `s[i]`.
    texts: Vec<Vec<char>>,
    /// Texts made at run time are collected when no value refers to them
    /// any more. The first `pool_texts` entries are the module's strings.
    pool_texts: usize,
    /// Characters (+1 per text) in made texts that may still be in use.
    live_chars: usize,
    /// Collect unused texts when `live_chars` passes this.
    text_gc_at: usize,
    /// Slots of collected texts, reused by new ones; `text_free[i]` marks them.
    free_texts: Vec<u32>,
    text_free: Vec<bool>,
    /// Program arguments (`ksvm prog.ksb a b c` gives `a`, `b`, `c`).
    args: Vec<String>,
    /// Instructions left before a Trap, if limited (see [`Vm::with_fuel`]).
    fuel: Option<u64>,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    heap: Vec<Block>,
    free: Vec<u32>,
    input: R,
    output: W,
    /// Byte offset of the instruction being executed, for trap reports.
    op_at: usize,
}

type Step = Result<Option<Value>, String>;

impl<'m, R: BufRead, W: Write> Vm<'m, R, W> {
    pub fn new(module: &'m Module, input: R, output: W) -> Self {
        Vm {
            module,
            texts: module.strings.iter().map(|s| s.chars().collect()).collect(),
            pool_texts: module.strings.len(),
            live_chars: 0,
            text_gc_at: TEXT_GC_START,
            free_texts: Vec::new(),
            text_free: vec![false; module.strings.len()],
            args: Vec::new(),
            fuel: None,
            stack: Vec::new(),
            frames: Vec::new(),
            heap: Vec::new(),
            free: Vec::new(),
            input,
            output,
            op_at: 0,
        }
    }

    /// Runs `main` to completion and returns its result.
    /// Sets the program arguments read by `argCount()` and `arg(i)`.
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Limits how many instructions may run; past it the program traps with
    /// "out of fuel". Used by tests so an accidental infinite loop still ends.
    pub fn with_fuel(mut self, steps: u64) -> Self {
        self.fuel = Some(steps);
        self
    }

    pub fn run(&mut self) -> Result<Value, Trap> {
        let entry = self.module.entry as usize;
        let result = self.enter(entry, Vec::new()).and_then(|()| loop {
            if let Some(v) = self.step()? {
                break Ok(v);
            }
        });
        let _ = self.output.flush();
        result.map_err(|reason| {
            let func = self.frames.last().map_or(entry, |f| f.func);
            let trap = Trap { func: self.module.functions[func].name.clone(), at: self.op_at, reason };
            self.scrub();
            trap
        })
    }

    /// Zero every heap cell and register (forensic wipe before aborting).
    fn scrub(&mut self) {
        for b in &mut self.heap {
            b.cells.fill(Value::Word(Word::ZERO));
            b.live = false;
        }
        for f in &mut self.frames {
            f.locals.fill(Value::NAT);
        }
        self.stack.fill(Value::NAT);
        self.stack.clear();
        self.frames.clear();
    }

    fn enter(&mut self, func: usize, args: Vec<Value>) -> Result<(), String> {
        if self.frames.len() >= MAX_DEPTH {
            return Err(format!("call stack overflow ({MAX_DEPTH} frames)"));
        }
        let f = &self.module.functions[func];
        let mut locals = vec![Value::Word(Word::ZERO); usize::from(f.locals)];
        locals[..args.len()].copy_from_slice(&args);
        self.frames.push(Frame { func, pc: 0, locals, soft: false });
        Ok(())
    }

    fn step(&mut self) -> Step {
        if let Some(f) = &mut self.fuel {
            if *f == 0 {
                return Err("out of fuel".into());
            }
            *f -= 1;
        }
        let frame = self.frames.last_mut().expect("a frame is active while running");
        let code = &self.module.functions[frame.func].code;
        if frame.pc >= code.len() {
            return Err("reached the end of the function without ret".into());
        }
        self.op_at = frame.pc;
        let (op, size) = Op::decode(code, frame.pc).map_err(|e| e.to_string())?;
        frame.pc += size;

        match op {
            Op::PushC(k) => self.push(Value::Word(Word::from(self.module.consts[k as usize]))),
            Op::PushR(k) => self.push(Value::Real(Real::from(self.module.consts[k as usize]))),
            Op::PushS(k) => self.push(Value::Text(k)),
            Op::Load(s) => {
                let v = self.frame().locals[usize::from(s)];
                self.push(v);
            }
            Op::Store(s) => {
                let v = self.pop()?;
                self.frame().locals[usize::from(s)] = v;
            }
            Op::Move(s) => {
                let slot = &mut self.frame().locals[usize::from(s)];
                let v = std::mem::replace(slot, Value::NAT);
                self.push(v);
            }
            Op::Pop => {
                self.pop()?;
            }
            Op::IsNaT => {
                let v = self.pop()?;
                self.push(Value::Word(if v.is_nat() { Word::ONE } else { Word::NEG_ONE }));
            }

            Op::Soft => self.frame().soft = true,
            Op::Hard => self.frame().soft = false,
            Op::ToBool => {
                let a = self.pop_word()?;
                let ok = matches!(a.get(), Some(1 | -1));
                self.check(if ok { a } else { Word::NAT }, a.is_nat(), || format!("(bool) {a}: a bool is only 1 (true) or -1 (false)"))?;
            }

            Op::Add => self.arith2("+", |a, b| a + b)?,
            Op::Sub => self.arith2("-", |a, b| a - b)?,
            Op::Mul => self.arith2("*", |a, b| a * b)?,
            Op::Div => self.arith2("/", |a, b| a / b)?,
            Op::Mod => self.arith2("%", |a, b| a % b)?,
            Op::Pow => self.arith2("**", Word::pow)?,
            Op::Shl3 => self.arith2("<<", Word::shl3)?,
            Op::Shr3 => self.arith2(">>", Word::shr3)?,
            Op::Cmp3 => self.int2(Word::cmp3)?,
            Op::KAnd => self.int2(Word::kand)?,
            Op::KOr => self.int2(Word::kor)?,
            Op::Neg => {
                let a = self.pop_word()?;
                self.check(-a, a.is_nat(), || "negation".into())?;
            }
            Op::Narrow6 => self.narrow("tryte", "-364..364", Word::narrow_tryte)?,
            Op::Narrow1 => self.narrow("trit", "-1..1", Word::narrow_trit)?,

            Op::FAdd => self.real2("+", |a, b| a + b)?,
            Op::FSub => self.real2("-", |a, b| a - b)?,
            Op::FMul => self.real2("*", |a, b| a * b)?,
            Op::FDiv => self.real2("/", |a, b| a / b)?,
            Op::FNeg => {
                let a = self.pop_real()?;
                self.check(-a, a.is_nat(), || "negation".into())?;
            }
            Op::FPow => {
                let e = self.pop_word()?;
                let a = self.pop_real()?;
                self.check(a.pow(e), a.is_nat() || e.is_nat(), || format!("overflow: {a} ** {e} does not fit in a real"))?;
            }
            Op::FCmp3 => {
                let b = self.pop_real()?;
                let a = self.pop_real()?;
                self.push(Value::Word(a.cmp3(b)));
            }
            Op::IToF => {
                let a = self.pop_word()?;
                self.check(Real::from_word(a), a.is_nat(), || format!("(real) {a}: does not fit in a real"))?;
            }
            Op::FToI => {
                let a = self.pop_real()?;
                self.check(a.to_word(), a.is_nat(), || format!("(int) {a}: does not fit in an int"))?;
            }

            Op::TLen => {
                let n = match self.pop()? {
                    Value::Text(t) => self.texts[t as usize].len(),
                    Value::Ptr(p) => {
                        let b = self.block(p)?;
                        b.cells.len() / b.w
                    }
                    other => return Err(format!("expected text or a buffer, found {other:?}")),
                };
                self.push(Value::Word(Word::new(n as i64)));
            }
            Op::TIdx => {
                let i = self.pop_word()?;
                let t = self.pop_text()?;
                let chars = &self.texts[t];
                let i = i.get().ok_or("text index is NaT")?;
                let c = usize::try_from(i)
                    .ok()
                    .and_then(|i| chars.get(i))
                    .ok_or_else(|| match chars.len() {
                        0 => format!("text index {i}: the text is empty"),
                        n => format!("text index {i} outside 0..{}", n - 1),
                    })?;
                let v = char_to_tryte(*c).ok_or_else(|| format!("character {c:?} is not in the tryte table"))?;
                self.push(Value::Word(Word::new(v)));
            }

            Op::Alloc | Op::AllocZ => {
                let n = self.claim_size(op == Op::Alloc, 1)?;
                let p = self.alloc(vec![Value::Word(Word::ZERO); n], 1, op == Op::Alloc);
                self.push(Value::Ptr(p));
            }
            Op::AllocT { w, sym } => {
                let w = usize::from(w).max(1);
                let template = self.pop_n(w)?;
                let n = self.claim_size(sym, w)?;
                let p = self.alloc(template.repeat(n), w, sym);
                self.push(Value::Ptr(p));
            }
            Op::Pack { n, sym } => {
                let cells = self.pop_n(n as usize)?;
                let p = self.alloc(cells, 1, sym);
                self.push(Value::Ptr(p));
            }
            Op::PackT { n, w, sym } => {
                let w = usize::from(w).max(1);
                let cells = self.pop_n(n as usize * w)?;
                let p = self.alloc(cells, w, sym);
                self.push(Value::Ptr(p));
            }
            Op::Append => {
                let p = self.pop_ptr()?;
                let w = self.block(p)?.w;
                let vals = self.pop_n(w)?;
                let b = self.block(p)?;
                if b.cells.len() + w > MAX_CELLS as usize {
                    return Err(format!("append: out of memory (limit {MAX_CELLS} cells)"));
                }
                b.cells.extend(vals);
            }
            Op::PopEl => {
                let p = self.pop_ptr()?;
                let b = self.block(p)?;
                if b.cells.is_empty() {
                    return Err("pop: the array is empty".into());
                }
                let at = b.cells.len() - b.w;
                let vals = b.cells.split_off(at);
                self.stack.extend(vals);
            }
            Op::Insert => {
                let i = self.pop_word()?;
                let p = self.pop_ptr()?;
                let w = self.block(p)?.w;
                let vals = self.pop_n(w)?;
                let b = self.block(p)?;
                let n = (b.cells.len() / w) as i64;
                let i = i.get().filter(|i| (0..=n).contains(i)).ok_or_else(|| format!("insert: index {i} outside 0..{n}"))?;
                if b.cells.len() + w > MAX_CELLS as usize {
                    return Err(format!("insert: out of memory (limit {MAX_CELLS} cells)"));
                }
                let at = i as usize * w;
                b.cells.splice(at..at, vals);
            }
            Op::Remove => {
                let i = self.pop_word()?;
                let p = self.pop_ptr()?;
                let b = self.block(p)?;
                let at = elem_cell(b, i, 0, b.w).map_err(|e| format!("remove: {e}"))?;
                let vals: Vec<Value> = b.cells.drain(at..at + b.w).collect();
                self.stack.extend(vals);
            }
            Op::Resize => {
                let n = self.pop_word()?;
                let p = self.pop_ptr()?;
                let w = self.block(p)?.w;
                let zero = self.pop_n(w)?;
                let n = n.get().filter(|n| *n >= 0).ok_or_else(|| format!("resize: size {n} cannot be negative"))?;
                if n.saturating_mul(w as i64) > MAX_CELLS {
                    return Err(format!("resize {n}: out of memory (limit {MAX_CELLS} cells)"));
                }
                let n = n as usize;
                let old = self.block(p)?.cells.len() / w;
                if n < old {
                    // Shrinking must not throw values away: the cut elements must be zero.
                    let tail = self.cells(p)?[n * w..].to_vec();
                    let cut = tail.chunks(w).position(|e| !e.iter().zip(&zero).all(|(c, z)| self.same(*c, *z)));
                    if let Some(k) = cut {
                        let first = n + k;
                        return Err(format!(
                            "resize(v, {n}): element {first} still holds a value; clear elements {n}..{} first (set them to zero)",
                            old - 1
                        ));
                    }
                }
                let b = self.block(p)?;
                b.cells.truncate(n * w);
                while b.cells.len() < n * w {
                    b.cells.extend_from_slice(&zero);
                }
            }
            Op::Clear => {
                let p = self.pop_ptr()?;
                self.block(p)?.cells.clear();
            }
            Op::Replace => {
                let d = self.pop_word()?;
                let c = self.pop_word()?;
                let b2 = self.pop_word()?;
                let a = self.pop_word()?;
                let p = self.pop_ptr()?;
                let blk = self.block(p)?;
                let (w, lo, n) = (blk.w, blk.lo, (blk.cells.len() / blk.w) as i64);
                let range = |x: Word, y: Word| -> Result<(usize, usize), String> {
                    let (x, y) = (x.get().ok_or("replace: a range bound is NaT")?, y.get().ok_or("replace: a range bound is NaT")?);
                    if x > y || x < lo || y >= lo + n {
                        return Err(format!("replace: range {x}..{y} is not inside {lo}..{}", lo + n - 1));
                    }
                    Ok(((x - lo) as usize, (y - lo) as usize))
                };
                let (mut r1, mut r2) = (range(a, b2)?, range(c, d)?);
                if r1.0 > r2.0 {
                    std::mem::swap(&mut r1, &mut r2);
                }
                if r1.1 >= r2.0 {
                    return Err(format!("replace: ranges {a}..{b2} and {c}..{d} overlap"));
                }
                // [.. A .. mid .. B ..] -> [.. B .. mid .. A ..]
                let cells = &mut blk.cells;
                let (a0, a1, b0, b1) = (r1.0 * w, (r1.1 + 1) * w, r2.0 * w, (r2.1 + 1) * w);
                let mut out = cells[..a0].to_vec();
                out.extend_from_slice(&cells[b0..b1]);
                out.extend_from_slice(&cells[a1..b0]);
                out.extend_from_slice(&cells[a0..a1]);
                out.extend_from_slice(&cells[b1..]);
                *cells = out;
            }
            Op::LdIdx => self.load_elem(0, 1)?,
            Op::StIdx => self.store_elem(0, 1)?,
            Op::LdElem { off, n } => self.load_elem(usize::from(off), usize::from(n))?,
            Op::StElem { off, n } => self.store_elem(usize::from(off), usize::from(n))?,
            Op::Purge(s) => {
                let slot = self.frame().locals[usize::from(s)];
                let Value::Ptr(p) = slot else {
                    return Err("burn of a buffer that was already burned or given away".into());
                };
                let b = self.block(p)?;
                b.cells.fill(Value::Word(Word::ZERO));
                b.cells = Vec::new();
                b.live = false;
                b.generation = b.generation.wrapping_add(1);
                self.free.push(p.block);
                self.frame().locals[usize::from(s)] = Value::NAT;
            }

            Op::Jmp(a) => self.frame().pc = a as usize,
            Op::Test(mask) => {
                let w = self.pop_word()?;
                let r = match w.get() {
                    None => Word::NAT,
                    Some(c) if mask & (1 << (c.signum() + 1)) != 0 => Word::ONE,
                    Some(_) => Word::NEG_ONE,
                };
                self.push(Value::Word(r));
            }
            Op::Trap(k) => return Err(self.module.strings[k as usize].clone()),
            Op::JNat(a) => {
                if self.stack.last().is_some_and(|v| v.is_nat()) {
                    self.stack.pop();
                    self.frame().pc = a as usize;
                }
            }
            Op::Br3 { neg, zero, pos } => {
                let t = self.pop_word()?.get().ok_or("tri/if/while condition is NaT")?;
                self.frame().pc = match t.signum() {
                    -1 => neg,
                    0 => zero,
                    _ => pos,
                } as usize;
            }
            Op::Call { f, argc } => {
                let argc = usize::from(argc);
                if self.stack.len() < argc {
                    return Err("stack underflow in call".into());
                }
                let args = self.stack.split_off(self.stack.len() - argc);
                self.enter(f as usize, args)?;
            }
            Op::Ret => {
                let v = self.pop()?;
                self.frames.pop();
                if self.frames.is_empty() {
                    return Ok(Some(v));
                }
                self.push(v);
            }
            Op::RetN(n) => {
                let n = usize::from(n);
                if self.stack.len() < n {
                    return Err("stack underflow in retn".into());
                }
                let vals = self.stack.split_off(self.stack.len() - n);
                self.frames.pop();
                if self.frames.is_empty() {
                    return Err("`main` cannot return a struct".into());
                }
                self.stack.extend(vals);
            }

            Op::ReadFile => {
                let path = self.pop_string()?;
                let s = std::fs::read_to_string(&path).map_err(|e| format!("cannot read file {path:?}: {e}"))?;
                self.make_text(s.chars().collect())?;
            }
            Op::FileExists => {
                let path = self.pop_string()?;
                let ok = std::path::Path::new(&path).is_file();
                self.push(Value::Word(if ok { Word::ONE } else { Word::NEG_ONE }));
            }
            Op::WriteFile => {
                let text = self.pop_string()?;
                let path = self.pop_string()?;
                std::fs::write(&path, text).map_err(|e| format!("cannot write file {path:?}: {e}"))?;
            }
            Op::WriteBytes => {
                let p = self.pop_ptr()?;
                let path = self.pop_string()?;
                let mut bytes = Vec::new();
                for (i, c) in self.cells(p)?.iter().enumerate() {
                    match c {
                        Value::Word(w) if (0..=255).contains(&w.get().unwrap_or(-1)) => bytes.push(w.get().unwrap_or(0) as u8),
                        Value::Word(w) => return Err(format!("writeBytes: cell {i} is {w}, not a byte (0..255)")),
                        other => return Err(format!("writeBytes: cell {i} is {other:?}, not a byte")),
                    }
                }
                std::fs::write(&path, bytes).map_err(|e| format!("cannot write file {path:?}: {e}"))?;
            }
            Op::ReadLine => {
                self.output.flush().map_err(io_err)?;
                let mut line = String::new();
                self.input.read_line(&mut line).map_err(io_err)?;
                let line = line.strip_suffix('\n').unwrap_or(&line);
                let line = line.strip_suffix('\r').unwrap_or(line);
                let chars = line.chars().collect();
                self.make_text(chars)?;
            }
            Op::Eof => {
                let done = self.input.fill_buf().map_err(io_err)?.is_empty();
                self.push(Value::Word(if done { Word::ONE } else { Word::NEG_ONE }));
            }
            Op::ArgCount => self.push(Value::Word(Word::new(self.args.len() as i64))),
            Op::Arg => {
                let i = self.pop_word()?.get().ok_or("arg: the index is NaT")?;
                let n = self.args.len();
                let a = usize::try_from(i).ok().and_then(|i| self.args.get(i)).ok_or_else(|| format!("arg({i}): there are {n} arguments"))?;
                let chars = a.chars().collect();
                self.make_text(chars)?;
            }
            Op::Exit => {
                let code = self.pop_word()?;
                return Ok(Some(Value::Word(code)));
            }
            Op::Input => {
                self.output.flush().map_err(io_err)?;
                let mut line = String::new();
                self.input.read_line(&mut line).map_err(io_err)?;
                let line = line.trim();
                // null (NaT) when the line is not an integer: `input()` is `int?`.
                self.push(parse_input(line).into());
            }
            Op::Emit => {
                // A tryte buffer prints as the characters of its cells.
                let s: String = match self.pop()? {
                    Value::Ptr(p) => self.cells(p)?.iter().map(|c| if let Value::Word(w) = c { tryte_char(*w) } else { '\u{FFFD}' }).collect(),
                    Value::Word(w) => tryte_char(w).to_string(),
                    other => return Err(format!("expected an int, found {other:?}")),
                };
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
            }
            Op::Print => {
                // A text without a value prints as NaT, like Java prints `null`.
                let s: String = match self.pop()? {
                    v if v.is_nat() => "NaT".into(),
                    Value::Text(t) => self.texts[t as usize].iter().collect(),
                    other => return Err(format!("expected a text, found {other:?}")),
                };
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
            }
            Op::Show | Op::ShowT | Op::ShowV => {
                let v = self.pop()?;
                let s = self.render(v, op)?;
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
            }
            Op::TStr | Op::TStrT | Op::TStrV => {
                let v = self.pop()?;
                let s: Vec<char> = self.render(v, op)?.chars().collect();
                self.make_text(s)?;
            }
            Op::TCat => {
                let b = self.pop_text()?;
                let a = self.pop_text()?;
                let s: Vec<char> = self.texts[a].iter().chain(&self.texts[b]).copied().collect();
                self.make_text(s)?;
            }
            Op::TEq => {
                let b = self.pop_text()?;
                let a = self.pop_text()?;
                let eq = self.texts[a] == self.texts[b];
                self.push(Value::Word(if eq { Word::ONE } else { Word::NEG_ONE }));
            }
            Op::TSub => {
                let end = self.pop_word()?.get().ok_or("sub: an index is NaT")?;
                let start = self.pop_word()?.get().ok_or("sub: an index is NaT")?;
                let t = self.pop_text()?;
                let len = self.texts[t].len() as i64;
                if !(0 <= start && start <= end && end <= len) {
                    return Err(format!("sub(s, {start}, {end}): needs 0 <= start <= end <= {len}"));
                }
                let s = self.texts[t][start as usize..end as usize].to_vec();
                self.make_text(s)?;
            }
            Op::TToI => {
                let t = self.pop_text()?;
                let s: String = self.texts[t].iter().collect();
                self.push(parse_input(&s).into());
            }
            Op::TToF => {
                let t = self.pop_text()?;
                let s: String = self.texts[t].iter().collect();
                self.push(parse_real(&s).into());
            }
            Op::CharIs(k) => {
                let w = self.pop_word()?;
                let class = Class::ALL[usize::from(k)];
                let r = match w.get() {
                    None => Word::NAT,
                    Some(v) if class.contains(v) => Word::ONE,
                    Some(_) => Word::NEG_ONE,
                };
                self.push(Value::Word(r));
            }
            Op::Dump(k) => self.dump(k).map_err(io_err)?,
        }
        Ok(None)
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is active while running")
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> Result<Value, String> {
        self.stack.pop().ok_or_else(|| "stack underflow".to_string())
    }

    /// Stores a text made at run time and pushes it. Every operand of the
    /// op that made it was already popped, so a collection here is safe.
    fn make_text(&mut self, s: Vec<char>) -> Result<(), String> {
        let cost = s.len() + 1;
        if self.live_chars + cost > self.text_gc_at {
            let scanned = self.collect_texts();
            // Grow the threshold with what is alive and with what a collection
            // has to look at, so collecting stays cheap on average.
            self.text_gc_at = TEXT_GC_START.max(self.live_chars * 2).max(scanned);
        }
        if self.live_chars + cost > MAX_TEXT_CHARS {
            return Err(format!("out of text memory (limit {MAX_TEXT_CHARS} characters in texts alive at once)"));
        }
        self.live_chars += cost;
        let id = match self.free_texts.pop() {
            Some(id) => {
                self.texts[id as usize] = s;
                self.text_free[id as usize] = false;
                id
            }
            None => {
                self.texts.push(s);
                self.text_free.push(false);
                (self.texts.len() - 1) as u32
            }
        };
        self.push(Value::Text(id));
        Ok(())
    }

    /// Frees made texts that no value refers to (mark and sweep over the
    /// stack, every frame's variables and every live array). Returns how many
    /// values it looked at.
    fn collect_texts(&mut self) -> usize {
        let mut used = vec![false; self.texts.len()];
        let mut scanned = 0;
        let cells = self.heap.iter().filter(|b| b.live).flat_map(|b| b.cells.iter());
        for v in self.stack.iter().chain(self.frames.iter().flat_map(|f| f.locals.iter())).chain(cells) {
            scanned += 1;
            if let Value::Text(t) = v {
                used[*t as usize] = true;
            }
        }
        for (i, used) in used.iter().enumerate().skip(self.pool_texts) {
            if !used && !self.text_free[i] {
                self.live_chars -= self.texts[i].len() + 1;
                self.texts[i] = Vec::new();
                self.text_free[i] = true;
                self.free_texts.push(i as u32);
            }
        }
        scanned
    }

    /// A value as `print`/`toText` writes it; the `T`/`V` ops take integers only.
    fn render(&self, v: Value, op: Op) -> Result<String, String> {
        let base = match op {
            Op::ShowT | Op::TStrT => Some(true),
            Op::ShowV | Op::TStrV => Some(false),
            _ => None,
        };
        Ok(match (v, base) {
            (Value::Word(w), _) => word_text(w, base),
            // An array prints its cells: `[-8, -1, 0]`, `["Ana", "Bia"]`.
            (Value::Ptr(p), _) => {
                let mut cells = Vec::new();
                for &c in self.cells(p)? {
                    cells.push(match c {
                        Value::Text(t) => format!("\"{}\"", self.texts[t as usize].iter().collect::<String>()),
                        _ => self.render(c, op)?,
                    });
                }
                format!("[{}]", cells.join(", "))
            }
            (Value::Real(r), None) => r.to_string(),
            (Value::Text(t), None) => self.texts[t as usize].iter().collect(),
            (other, Some(_)) => return Err(format!("expected an int, found {other:?}")),
        })
    }

    fn pop_word(&mut self) -> Result<Word, String> {
        match self.pop()? {
            Value::Word(w) => Ok(w),
            other => Err(format!("expected an int, found {other:?}")),
        }
    }

    fn pop_real(&mut self) -> Result<Real, String> {
        match self.pop()? {
            Value::Real(r) => Ok(r),
            other => Err(format!("expected a real, found {other:?}")),
        }
    }

    fn pop_string(&mut self) -> Result<String, String> {
        let t = self.pop_text()?;
        Ok(self.texts[t].iter().collect())
    }

    fn pop_text(&mut self) -> Result<usize, String> {
        match self.pop()? {
            Value::Text(t) => Ok(t as usize),
            v if v.is_nat() => Err("use of a text that has no value (NaT)".into()),
            other => Err(format!("expected a text, found {other:?}")),
        }
    }

    fn pop_ptr(&mut self) -> Result<Ptr, String> {
        match self.pop()? {
            Value::Ptr(p) => Ok(p),
            v if v.is_nat() => Err("use of an array or object that has no value, or was burned or given away".into()),
            other => Err(format!("expected a buffer, found {other:?}")),
        }
    }

    fn int2(&mut self, f: impl FnOnce(Word, Word) -> Word) -> Result<(), String> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        self.push(Value::Word(f(a, b)));
        Ok(())
    }

    /// Pushes `v`. A NaT result traps, unless this line has `on NaT` (soft
    /// mode). `nat_in` says an operand was already NaT; otherwise `why`
    /// explains what went wrong.
    fn check(&mut self, v: impl Into<Value>, nat_in: bool, why: impl FnOnce() -> String) -> Result<(), String> {
        let v = v.into();
        if v.is_nat() && !self.frames.last().is_some_and(|f| f.soft) {
            let what = if nat_in { "an operation got a NaT value".to_string() } else { why() };
            return Err(format!("{what} (handle it with `on NaT`)"));
        }
        self.push(v);
        Ok(())
    }

    fn arith2(&mut self, op: &str, f: impl FnOnce(Word, Word) -> Word) -> Result<(), String> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        self.check(f(a, b), a.is_nat() || b.is_nat(), || match op {
            "/" if b == Word::ZERO => "division by zero".into(),
            "%" if b == Word::ZERO => "remainder of a division by zero".into(),
            "**" if b.get().is_some_and(|e| e < 0) => format!("{a} ** {b}: the exponent cannot be negative"),
            _ => format!("overflow: {a} {op} {b} does not fit in an int"),
        })
    }

    fn narrow(&mut self, ty: &str, range: &str, f: impl FnOnce(Word) -> Word) -> Result<(), String> {
        let a = self.pop_word()?;
        self.check(f(a), a.is_nat(), || format!("overflow: {a} does not fit in a {ty} ({range})"))
    }

    fn real2(&mut self, op: &str, f: impl FnOnce(Real, Real) -> Real) -> Result<(), String> {
        let b = self.pop_real()?;
        let a = self.pop_real()?;
        self.check(f(a, b), a.is_nat() || b.is_nat(), || match op {
            "/" if b.is_zero() => "division by zero".into(),
            _ => format!("overflow: {a} {op} {b} does not fit in a real"),
        })
    }

    /// Pops the element count of a `claim` and checks it (`w` cells each).
    fn claim_size(&mut self, sym: bool, w: usize) -> Result<usize, String> {
        let n = self.pop_word()?.get().ok_or("claim size is NaT")?;
        if sym && (n <= 0 || n % 2 == 0) {
            return Err(format!("claim {n}: a sym array needs an odd, positive size (2k+1)"));
        }
        if n < 0 {
            return Err(format!("claim {n}: size cannot be negative"));
        }
        if n.saturating_mul(w as i64) > MAX_CELLS {
            return Err(format!("claim {n}: out of memory (limit {MAX_CELLS} cells)"));
        }
        Ok(n as usize)
    }

    /// Whether two cells hold the same value (texts compare by their characters).
    fn same(&self, a: Value, b: Value) -> bool {
        match (a, b) {
            (Value::Text(x), Value::Text(y)) => self.texts[x as usize] == self.texts[y as usize],
            _ => a == b,
        }
    }

    /// Pops the top `n` values, first pushed first.
    fn pop_n(&mut self, n: usize) -> Result<Vec<Value>, String> {
        if self.stack.len() < n {
            return Err("stack underflow".to_string());
        }
        Ok(self.stack.split_off(self.stack.len() - n))
    }

    /// `index`, `ptr` -> `n` cells of that element, from cell `off` on.
    fn load_elem(&mut self, off: usize, n: usize) -> Result<(), String> {
        let i = self.pop_word()?;
        let p = self.pop_ptr()?;
        let b = self.block(p)?;
        let at = elem_cell(b, i, off, n)?;
        let vals = b.cells[at..at + n].to_vec();
        self.stack.extend(vals);
        Ok(())
    }

    /// `n` values, `index`, `ptr`: store them into that element from cell `off` on.
    fn store_elem(&mut self, off: usize, n: usize) -> Result<(), String> {
        let vals = self.pop_n(n)?;
        let i = self.pop_word()?;
        let p = self.pop_ptr()?;
        let b = self.block(p)?;
        let at = elem_cell(b, i, off, n)?;
        b.cells[at..at + n].copy_from_slice(&vals);
        Ok(())
    }

    fn alloc(&mut self, cells: Vec<Value>, w: usize, sym: bool) -> Ptr {
        let lo = if sym { -((cells.len() / w) as i64 / 2) } else { 0 };
        if let Some(idx) = self.free.pop() {
            let b = &mut self.heap[idx as usize];
            b.cells = cells;
            b.w = w;
            b.lo = lo;
            b.live = true;
            Ptr { block: idx, generation: b.generation }
        } else {
            self.heap.push(Block { cells, w, lo, generation: 0, live: true });
            Ptr { block: (self.heap.len() - 1) as u32, generation: 0 }
        }
    }

    fn cells(&self, p: Ptr) -> Result<&[Value], String> {
        self.heap
            .get(p.block as usize)
            .filter(|b| b.live && b.generation == p.generation)
            .map(|b| b.cells.as_slice())
            .ok_or_else(|| "access through a stale pointer (buffer already burned)".to_string())
    }

    fn block(&mut self, p: Ptr) -> Result<&mut Block, String> {
        self.heap
            .get_mut(p.block as usize)
            .filter(|b| b.live && b.generation == p.generation)
            .ok_or_else(|| "access through a stale pointer (buffer already burned)".to_string())
    }

    fn dump(&mut self, label: u32) -> std::io::Result<()> {
        let frame = self.frames.last().expect("a frame is active while running");
        let f = &self.module.functions[frame.func];
        let out = &mut self.output;
        writeln!(out, "\n=== dump: {} (in {}) ===", self.module.strings[label as usize], f.name)?;
        for (i, v) in frame.locals.iter().enumerate() {
            let name = f.local_names.get(i).map_or("", String::as_str);
            if name.starts_with('$') {
                continue; // compiler temporary
            }
            let reg = slot_name(i as u16);
            let shown = match *v {
                Value::Word(w) => {
                    let b = Word27::from(w);
                    format!("{w:<14} {}  {}", b.to_trits(27), b.to_hepta())
                }
                Value::Real(r) => format!("{r:<14} (real)  {}", Word27::from(r).to_hepta()),
                Value::Text(t) => format!("{:?}", self.texts[t as usize].iter().collect::<String>()),
                Value::Ptr(p) => format!("-> heap #{}", p.block),
            };
            writeln!(out, "  {reg:<4} {name:<10} = {shown}")?;
        }
        for (i, b) in self.heap.iter().enumerate().filter(|(_, b)| b.live) {
            write!(out, "  heap #{i} ({} cells):", b.cells.len())?;
            for (k, elem) in b.cells.chunks(b.w.max(1)).enumerate() {
                let i = k as i64 + b.lo;
                let shown: Vec<String> = elem
                    .iter()
                    .map(|c| match c {
                        Value::Word(w) => w.to_string(),
                        Value::Real(r) => r.to_string(),
                        Value::Text(t) => format!("{:?}", self.texts[*t as usize].iter().collect::<String>()),
                        Value::Ptr(p) => format!("->#{}", p.block),
                    })
                    .collect();
                let shown = if b.w > 1 { format!("({})", shown.join(" ")) } else { shown.join(" ") };
                if b.lo < 0 {
                    write!(out, " [{i:+}]={shown}")?;
                } else {
                    write!(out, " [{i}]={shown}")?;
                }
            }
            writeln!(out)?;
        }
        writeln!(out, "===")
    }
}

/// An integer in decimal, or in ternary (`Some(true)`) / heptavintimal (`Some(false)`).
fn word_text(w: Word, base: Option<bool>) -> String {
    match base {
        _ if w.is_nat() => "NaT".to_string(),
        None => w.to_string(),
        Some(true) => Word27::from(w).to_trits_short(),
        Some(false) => Word27::from(w).to_hepta_short(),
    }
}

fn tryte_char(w: Word) -> char {
    w.get().and_then(tryte_to_char).unwrap_or('\u{FFFD}')
}

/// First cell of `n` cells at offset `off` inside element `i`.
fn elem_cell(b: &Block, i: Word, off: usize, n: usize) -> Result<usize, String> {
    let i = i.get().ok_or("buffer index is NaT")?;
    let hi = b.lo + (b.cells.len() / b.w) as i64 - 1;
    if i < b.lo || i > hi {
        let range = match (b.lo < 0, hi < b.lo) {
            (_, true) => "none: it is empty".to_string(),
            (true, _) => format!("{}..+{hi}", b.lo),
            (false, _) => format!("0..{hi}"),
        };
        return Err(format!("index {i} outside {range}"));
    }
    if off + n > b.w {
        return Err(format!("cells {off}..{} outside an element of {} cells", off + n, b.w));
    }
    Ok((i - b.lo) as usize * b.w + off)
}

/// `input()`: decimal, `0t...` or `0v...`. Anything else is NaT.
fn parse_input(s: &str) -> Word {
    let parsed = if let Some(t) = s.strip_prefix("0t") {
        parse_trits(t).ok()
    } else if let Some(v) = s.strip_prefix("0v") {
        parse_hepta(v).ok()
    } else {
        s.parse::<i64>().ok().map(Word::new)
    };
    parsed.unwrap_or(Word::NAT)
}

/// `toReal`: `[-]digits[.digits]`. Anything else is NaT.
fn parse_real(s: &str) -> Real {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty() {
        return Real::NAT;
    }
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return Real::NAT;
    }
    let r = Real::from_decimal(int, frac).unwrap_or_else(|| long_decimal(int, frac));
    if neg { -r } else { r }
}

/// A decimal with too many digits for an exact conversion: keep its first 36
/// significant digits (far more than a real holds) and scale by a power of 10.
fn long_decimal(int: &str, frac: &str) -> Real {
    const KEEP: usize = 36;
    let all = format!("{int}{frac}");
    let digits = all.trim_start_matches('0');
    if digits.is_empty() {
        return Real::ZERO;
    }
    let kept = &digits[..digits.len().min(KEEP)];
    // value = kept * 10^e10
    let e10 = (int.len() as i64 - all.len() as i64) + (digits.len() - kept.len()) as i64;
    let Some(m) = Real::from_decimal(kept, "") else { return Real::NAT };
    let ten = Real::from_word(Word::new(10));
    if e10 >= 0 {
        return m * ten.pow(Word::new(e10));
    }
    // Dividing by a power of 10 too big for a real means the value rounds to 0.
    let scale = ten.pow(Word::new(-e10));
    if scale.is_nat() { Real::ZERO } else { m / scale }
}

fn io_err(e: std::io::Error) -> String {
    format!("I/O error: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_core::ksb::Function;

    fn code(ops: &[Op]) -> Vec<u8> {
        let mut c = Vec::new();
        for op in ops {
            op.encode(&mut c);
        }
        c
    }

    fn func(name: &str, params: u8, locals: u16, ops: &[Op]) -> Function {
        Function { name: name.into(), params, locals, code: code(ops), local_names: vec![] }
    }

    /// Words used as constants: index i holds CONSTS[i].
    const CONSTS: [i64; 8] = [0, 1, 2, 5, 8, -1, 3, 4];
    fn c(v: i64) -> Op {
        Op::PushC(CONSTS.iter().position(|&x| x == v).expect("add the value to CONSTS") as u32)
    }

    fn module(functions: Vec<Function>, strings: &[&str]) -> Module {
        let mut consts: Vec<Word27> = CONSTS.iter().map(|&v| Word27::from(Word::new(v))).collect();
        consts.push(Word27::from(Real::from_decimal("0", "1").unwrap())); // #8 = 0.1
        consts.push(Word27::from(Real::from_decimal("0", "2").unwrap())); // #9 = 0.2
        let m = Module { entry: 0, consts, strings: strings.iter().map(|s| s.to_string()).collect(), functions };
        m.verify().expect("test module is valid");
        m
    }

    fn run(m: &Module, input: &str) -> (Result<Value, Trap>, String) {
        let mut out = Vec::new();
        let r = Vm::new(m, input.as_bytes(), &mut out).run();
        (r, String::from_utf8(out).unwrap())
    }

    fn trap_reason(m: &Module) -> String {
        run(m, "").0.expect_err("expected a trap").reason
    }

    #[test]
    fn call_and_return() {
        // main: print(sum(8)); return 0     sum(v): return v + v
        let m = module(
            vec![
                func("main", 0, 0, &[c(8), Op::Call { f: 1, argc: 1 }, Op::Show, c(0), Op::Ret]),
                func("sum", 1, 1, &[Op::Load(0), Op::Load(0), Op::Add, Op::Ret]),
            ],
            &[],
        );
        let (r, out) = run(&m, "");
        assert_eq!(r, Ok(Value::Word(Word::ZERO)));
        assert_eq!(out, "16");
    }

    #[test]
    fn reals_and_text() {
        let m = module(
            vec![func(
                "main",
                0,
                0,
                &[
                    Op::PushR(9), Op::PushR(8), Op::FAdd, Op::Show,
                    Op::PushS(0), Op::Print,
                    Op::PushS(1), Op::TLen, Op::Show,
                    Op::PushS(1), c(1), Op::TIdx, Op::Emit,
                    Op::PushS(1), c(1), Op::TIdx, Op::Neg, Op::Emit,
                    c(0), Op::Ret,
                ],
            )],
            &[" | ", "Kestrel"],
        );
        assert_eq!(run(&m, "").1, "0.3 | 7eE");
    }

    #[test]
    fn symmetric_buffer() {
        // b = claim 5; b[-2] = 8; b[2] = 5; print(b[-2] + b[2]); burn b
        let m = module(
            vec![func(
                "main",
                0,
                1,
                &[
                    c(5), Op::Alloc, Op::Store(0),
                    Op::Load(0), c(2), Op::Neg, c(8), Op::StIdx,
                    Op::Load(0), c(2), c(5), Op::StIdx,
                    Op::Load(0), c(2), Op::Neg, Op::LdIdx,
                    Op::Load(0), c(2), Op::LdIdx,
                    Op::Add, Op::Show,
                    Op::Purge(0),
                    c(0), Op::Ret,
                ],
            )],
            &[],
        );
        let (r, out) = run(&m, "");
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(out, "13");
    }

    #[test]
    fn traps() {
        let even = module(vec![func("main", 0, 0, &[c(4), Op::Alloc, c(0), Op::Ret])], &[]);
        assert!(trap_reason(&even).contains("odd"));

        let oob = module(vec![func("main", 0, 1, &[c(3), Op::Alloc, Op::Store(0), Op::Load(0), c(2), Op::LdIdx, c(0), Op::Ret])], &[]);
        assert!(trap_reason(&oob).contains("outside -1..+1"));

        let double_burn = module(vec![func("main", 0, 1, &[c(1), Op::Alloc, Op::Store(0), Op::Purge(0), Op::Purge(0), c(0), Op::Ret])], &[]);
        assert!(trap_reason(&double_burn).contains("already burned"));

        // A copy of the pointer survives the burn, but its generation is stale
        // even after the block is reused by a new claim.
        let stale = module(
            vec![func(
                "main",
                0,
                2,
                &[
                    c(1), Op::Alloc, Op::Store(0), Op::Load(0), Op::Store(1), Op::Purge(0),
                    c(1), Op::Alloc, Op::Store(0),
                    Op::Load(1), c(0), Op::LdIdx, c(0), Op::Ret,
                ],
            )],
            &[],
        );
        assert!(trap_reason(&stale).contains("stale pointer"));

        let div0 = module(vec![func("main", 0, 0, &[c(1), c(0), Op::Div, c(0), Op::Ret])], &[]);
        assert_eq!(trap_reason(&div0), "division by zero (handle it with `on NaT`)");

        let tri_nat = module(vec![func("main", 0, 0, &[Op::Soft, c(1), c(0), Op::Div, Op::Br3 { neg: 0, zero: 0, pos: 0 }])], &[]);
        assert_eq!(trap_reason(&tri_nat), "tri/if/while condition is NaT");

        let moved = module(vec![func("main", 0, 2, &[c(1), Op::Alloc, Op::Store(0), Op::Move(0), Op::Store(1), Op::Load(0), c(0), Op::LdIdx, c(0), Op::Ret])], &[]);
        assert!(trap_reason(&moved).contains("burned or given away"));

        let recursion = module(vec![func("main", 0, 0, &[Op::Call { f: 0, argc: 0 }, Op::Ret])], &[]);
        assert!(trap_reason(&recursion).contains("stack overflow"));
    }

    #[test]
    fn trap_zeroes_the_heap() {
        // Fill a buffer, then trap on an even claim while it is still alive.
        let m = module(
            vec![func("main", 0, 1, &[c(3), Op::Alloc, Op::Store(0), Op::Load(0), c(0), c(8), Op::StIdx, c(2), Op::Alloc, c(0), Op::Ret])],
            &[],
        );
        let mut out = Vec::new();
        let mut vm = Vm::new(&m, "".as_bytes(), &mut out);
        assert!(vm.run().is_err());
        assert!(vm.heap.iter().all(|b| !b.live && b.cells.iter().all(|c| *c == Value::Word(Word::ZERO))));
        assert!(vm.stack.is_empty() && vm.frames.is_empty());
    }

    #[test]
    fn nat_guard_and_input() {
        // x = input() on NaT { print "bad"; return -1 }; print(x)
        // Soft, Input, Hard (1 each) + JNat(5) + Show(1) + PushC(5) + Ret(1): the handler starts at byte 15.
        let m = module(
            vec![func(
                "main",
                0,
                0,
                &[Op::Soft, Op::Input, Op::Hard, Op::JNat(15), Op::Show, c(0), Op::Ret, Op::PushS(0), Op::Print, c(-1), Op::Ret],
            )],
            &["bad"],
        );
        let hard = module(vec![func("main", 0, 0, &[Op::Input, Op::Show, c(0), Op::Ret])], &[]);
        // Not an integer: `input()` gives null (NaT) instead of trapping.
        let (r, shown) = run(&hard, "banana\n");
        assert!(r.is_ok() && shown == "NaT", "{shown}");
        assert_eq!(run(&m, "42\n").1, "42");
        assert_eq!(run(&m, "0t+0-\n").1, "8");
        assert_eq!(run(&m, "0vAm\n").1, "14");
        let (r, out) = run(&m, "banana\n");
        assert_eq!((r.unwrap(), out.as_str()), (Value::Word(Word::NEG_ONE), "bad"));
    }

    #[test]
    fn dump_shows_registers_and_heap() {
        let mut m = module(vec![func("main", 0, 2, &[c(8), Op::Store(4 - 4), c(3), Op::Alloc, Op::Store(1), Op::Dump(0), Op::Purge(1), c(0), Op::Ret])], &["here"]);
        m.functions[0].local_names = vec!["x".into(), "buf".into()];
        let out = run(&m, "").1;
        assert!(out.contains("=== dump: here (in main) ==="), "{out}");
        assert!(out.contains("R-4  x          = 8              0t"), "{out}");
        assert!(out.contains("0t000000000000000000000000+0-  0v00000000H"), "{out}");
        assert!(out.contains("R-3  buf        = -> heap #0"), "{out}");
        assert!(out.contains("heap #0 (3 cells): [-1]=0 [+0]=0 [+1]=0"), "{out}");
    }
}
