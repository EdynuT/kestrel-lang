//! The BET virtual machine: runs a verified `.ksb` [`Module`].
//!
//! Values are tagged ([`Value`]), the heap hands out generation-checked
//! pointers, and any Trap zeroes the whole heap and every register before
//! the machine stops (SPEC §7).

use std::fmt;
use std::io::{BufRead, Write};

use kestrel_core::bet::{parse_hepta, parse_trits};
use kestrel_core::charset::{char_to_tryte, tryte_to_char};
use kestrel_core::isa::{Op, slot_name};
use kestrel_core::{Module, Real, Word, Word27};

/// Deepest call chain before the VM traps with a stack overflow.
pub const MAX_DEPTH: usize = 4096;
/// Largest buffer `claim` may create, in cells.
pub const MAX_CELLS: i64 = 1 << 24;

/// A value on the stack or in a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Word(Word),
    Real(Real),
    /// Index into the module's string pool.
    Text(u32),
    Ptr(Ptr),
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
    cells: Vec<Word>,
    half: i64,
    generation: u32,
    live: bool,
}

struct Frame {
    func: usize,
    pc: usize,
    locals: Vec<Value>,
}

pub struct Vm<'m, R, W> {
    module: &'m Module,
    /// String pool split into chars, for `len` and `s[i]`.
    texts: Vec<Vec<char>>,
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
            b.cells.fill(Word::ZERO);
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
        self.frames.push(Frame { func, pc: 0, locals });
        Ok(())
    }

    fn step(&mut self) -> Step {
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

            Op::Add => self.int2(|a, b| a + b)?,
            Op::Sub => self.int2(|a, b| a - b)?,
            Op::Mul => self.int2(|a, b| a * b)?,
            Op::Div => self.int2(|a, b| a / b)?,
            Op::Mod => self.int2(|a, b| a % b)?,
            Op::Pow => self.int2(Word::pow)?,
            Op::Shl3 => self.int2(Word::shl3)?,
            Op::Shr3 => self.int2(Word::shr3)?,
            Op::Cmp3 => self.int2(Word::cmp3)?,
            Op::KAnd => self.int2(Word::kand)?,
            Op::KOr => self.int2(Word::kor)?,
            Op::Neg => self.int1(|a| -a)?,
            Op::Narrow6 => self.int1(Word::narrow_tryte)?,
            Op::Narrow1 => self.int1(Word::narrow_trit)?,

            Op::FAdd => self.real2(|a, b| a + b)?,
            Op::FSub => self.real2(|a, b| a - b)?,
            Op::FMul => self.real2(|a, b| a * b)?,
            Op::FDiv => self.real2(|a, b| a / b)?,
            Op::FNeg => {
                let a = self.pop_real()?;
                self.push(Value::Real(-a));
            }
            Op::FPow => {
                let e = self.pop_word()?;
                let a = self.pop_real()?;
                self.push(Value::Real(a.pow(e)));
            }
            Op::FCmp3 => {
                let b = self.pop_real()?;
                let a = self.pop_real()?;
                self.push(Value::Word(a.cmp3(b)));
            }
            Op::IToF => {
                let a = self.pop_word()?;
                self.push(Value::Real(Real::from_word(a)));
            }
            Op::FToI => {
                let a = self.pop_real()?;
                self.push(Value::Word(a.to_word()));
            }

            Op::TLen => {
                let t = self.pop_text()?;
                self.push(Value::Word(Word::new(self.texts[t].len() as i64)));
            }
            Op::TIdx => {
                let i = self.pop_word()?;
                let t = self.pop_text()?;
                let chars = &self.texts[t];
                let i = i.get().ok_or("text index is NaT")?;
                let c = usize::try_from(i)
                    .ok()
                    .and_then(|i| chars.get(i))
                    .ok_or_else(|| format!("text index {i} outside 0..{}", chars.len()))?;
                let v = char_to_tryte(*c).ok_or_else(|| format!("character {c:?} is not in the tryte table"))?;
                self.push(Value::Word(Word::new(v)));
            }

            Op::Alloc => {
                let n = self.pop_word()?.get().ok_or("claim size is NaT")?;
                if n <= 0 || n % 2 == 0 {
                    return Err(format!("claim {n}: size must be odd and positive (2k+1)"));
                }
                if n > MAX_CELLS {
                    return Err(format!("claim {n}: out of memory (limit {MAX_CELLS} cells)"));
                }
                let p = self.alloc(n);
                self.push(Value::Ptr(p));
            }
            Op::LdIdx => {
                let i = self.pop_word()?;
                let p = self.pop_ptr()?;
                let b = self.block(p)?;
                let v = b.cells[cell_index(b, i)?];
                self.push(Value::Word(v));
            }
            Op::StIdx => {
                let v = self.pop_word()?;
                let i = self.pop_word()?;
                let p = self.pop_ptr()?;
                let b = self.block(p)?;
                let at = cell_index(b, i)?;
                b.cells[at] = v;
            }
            Op::Purge(s) => {
                let slot = self.frame().locals[usize::from(s)];
                let Value::Ptr(p) = slot else {
                    return Err("burn of a buffer that was already burned or given away".into());
                };
                let b = self.block(p)?;
                b.cells.fill(Word::ZERO);
                b.cells = Vec::new();
                b.live = false;
                b.generation = b.generation.wrapping_add(1);
                self.free.push(p.block);
                self.frame().locals[usize::from(s)] = Value::NAT;
            }

            Op::Jmp(a) => self.frame().pc = a as usize,
            Op::JNat(a) => {
                if self.stack.last().is_some_and(|v| v.is_nat()) {
                    self.stack.pop();
                    self.frame().pc = a as usize;
                }
            }
            Op::Br3 { neg, zero, pos } => {
                let t = self.pop_word()?.get().ok_or("tri on NaT")?;
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

            Op::Input => {
                self.output.flush().map_err(io_err)?;
                let mut line = String::new();
                self.input.read_line(&mut line).map_err(io_err)?;
                self.push(Value::Word(parse_input(line.trim())));
            }
            Op::Emit => {
                let c = self.pop_word()?.get().and_then(tryte_to_char).unwrap_or('#');
                write!(self.output, "{c}").map_err(io_err)?;
            }
            Op::Print => {
                let t = self.pop_text()?;
                let s: String = self.texts[t].iter().collect();
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
            }
            Op::Show => {
                let v = self.pop()?;
                let s = match v {
                    Value::Word(w) => w.to_string(),
                    Value::Real(r) => r.to_string(),
                    Value::Text(t) => self.texts[t as usize].iter().collect(),
                    Value::Ptr(_) => "<buffer>".to_string(),
                };
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
            }
            Op::ShowT | Op::ShowV => {
                let w = self.pop_word()?;
                let s = match (w.is_nat(), op) {
                    (true, _) => "NaT".to_string(),
                    (false, Op::ShowT) => Word27::from(w).to_trits_short(),
                    (false, _) => Word27::from(w).to_hepta_short(),
                };
                self.output.write_all(s.as_bytes()).map_err(io_err)?;
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

    fn pop_text(&mut self) -> Result<usize, String> {
        match self.pop()? {
            Value::Text(t) => Ok(t as usize),
            other => Err(format!("expected a text, found {other:?}")),
        }
    }

    fn pop_ptr(&mut self) -> Result<Ptr, String> {
        match self.pop()? {
            Value::Ptr(p) => Ok(p),
            v if v.is_nat() => Err("use of a buffer that was burned or given away".into()),
            other => Err(format!("expected a buffer, found {other:?}")),
        }
    }

    fn int1(&mut self, f: impl FnOnce(Word) -> Word) -> Result<(), String> {
        let a = self.pop_word()?;
        self.push(Value::Word(f(a)));
        Ok(())
    }

    fn int2(&mut self, f: impl FnOnce(Word, Word) -> Word) -> Result<(), String> {
        let b = self.pop_word()?;
        let a = self.pop_word()?;
        self.push(Value::Word(f(a, b)));
        Ok(())
    }

    fn real2(&mut self, f: impl FnOnce(Real, Real) -> Real) -> Result<(), String> {
        let b = self.pop_real()?;
        let a = self.pop_real()?;
        self.push(Value::Real(f(a, b)));
        Ok(())
    }

    fn alloc(&mut self, n: i64) -> Ptr {
        let cells = vec![Word::ZERO; n as usize];
        let half = n / 2;
        if let Some(idx) = self.free.pop() {
            let b = &mut self.heap[idx as usize];
            b.cells = cells;
            b.half = half;
            b.live = true;
            Ptr { block: idx, generation: b.generation }
        } else {
            self.heap.push(Block { cells, half, generation: 0, live: true });
            Ptr { block: (self.heap.len() - 1) as u32, generation: 0 }
        }
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
                Value::Text(t) => format!("{:?}", self.module.strings[t as usize]),
                Value::Ptr(p) => format!("-> heap #{}", p.block),
            };
            writeln!(out, "  {reg:<4} {name:<10} = {shown}")?;
        }
        for (i, b) in self.heap.iter().enumerate().filter(|(_, b)| b.live) {
            write!(out, "  heap #{i} ({} cells):", b.cells.len())?;
            for (k, c) in b.cells.iter().enumerate() {
                write!(out, " [{:+}]={c}", k as i64 - b.half)?;
            }
            writeln!(out)?;
        }
        writeln!(out, "===")
    }
}

fn cell_index(b: &Block, i: Word) -> Result<usize, String> {
    let i = i.get().ok_or("buffer index is NaT")?;
    if i.abs() > b.half {
        Err(format!("index {i} outside -{h}..+{h}", h = b.half))
    } else {
        Ok((i + b.half) as usize)
    }
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
        // main: print(sum(8)); yield 0     sum(v): yield v + v
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

        let tri_nat = module(vec![func("main", 0, 0, &[c(1), c(0), Op::Div, Op::Br3 { neg: 0, zero: 0, pos: 0 }])], &[]);
        assert_eq!(trap_reason(&tri_nat), "tri on NaT");

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
        assert!(vm.heap.iter().all(|b| !b.live && b.cells.iter().all(|c| *c == Word::ZERO)));
        assert!(vm.stack.is_empty() && vm.frames.is_empty());
    }

    #[test]
    fn nat_guard_and_input() {
        // x = input(); on NaT { print "bad"; yield -1 }; print(x)
        // Input(1) + JNat(5) + Show(1) + PushC(5) + Ret(1): the handler starts at byte 13.
        let m = module(
            vec![func(
                "main",
                0,
                0,
                &[Op::Input, Op::JNat(13), Op::Show, c(0), Op::Ret, Op::PushS(0), Op::Print, c(-1), Op::Ret],
            )],
            &["bad"],
        );
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
