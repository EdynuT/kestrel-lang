//! Native mode: the program is x86-64 code made by `ksc` (its `native`
//! module), put into a copy of this executable as one more segment.
//!
//! That code does the jumps and calls itself, on the machine stack. For
//! every other instruction it calls [`krt_op`], which runs it on the VM's
//! frames and stack, as the interpreter would. When one fails or throws,
//! the code calls [`krt_unwind`], which finds the handler (as the
//! interpreter does) and says where to go: an address and the stack
//! pointer of that frame's code.

use std::collections::HashMap;
use std::io::{self, BufWriter, StdinLock, StdoutLock};

use kestrel_core::isa::Op;
use kestrel_core::Module;

use crate::{Trap, Value, Vm};

/// What `ksc` writes into [`KRT_PROGRAM`]: where its code, the program's
/// `.ksb` and the table of addresses are, as distances from `KRT_PROGRAM`.
#[repr(C)]
pub struct Program {
    pub present: u64,
    pub code: i64,
    pub ksb: i64,
    pub ksb_len: u64,
    pub table: i64,
    pub table_len: u64,
}

/// `present` of a plain `ksvm` (no native program).
pub const ABSENT: u64 = u64::from_le_bytes(*b"KRT-NONE");
/// `present` once `ksc` put a program in.
pub const PRESENT: u64 = u64::from_le_bytes(*b"KRT-PROG");

/// Found by `ksc` in the symbol table and filled in the executable it
/// writes. Not zero, so it is stored in the file (not in `.bss`).
#[unsafe(no_mangle)]
#[used]
pub static mut KRT_PROGRAM: Program = Program { present: ABSENT, code: 1, ksb: 1, ksb_len: 1, table: 1, table_len: 1 };

/// The top of the VM's stack, while native code runs: the address of the
/// next free value. Native code keeps it in `rbx`, and stores it here
/// before calling in (and loads it back after).
#[unsafe(no_mangle)]
pub static mut KRT_SP: usize = 0;
/// The end of the stack's room: native code pushes on its own only below it.
#[unsafe(no_mangle)]
pub static mut KRT_LIMIT: usize = 0;
/// The frames (see `Frame`): where they are (they never move) and how many.
#[unsafe(no_mangle)]
pub static mut KRT_FRAMES: usize = 0;
#[unsafe(no_mangle)]
pub static mut KRT_NFRAMES: usize = 0;
/// The variables of every frame: where they are (they never move) and how many.
#[unsafe(no_mangle)]
pub static mut KRT_LOCALS: usize = 0;
#[unsafe(no_mangle)]
pub static mut KRT_NLOCALS: usize = 0;
/// The heap's blocks (see `Block`): where they are (they move when there
/// are more) and how many.
#[unsafe(no_mangle)]
pub static mut KRT_HEAP: usize = 0;
#[unsafe(no_mangle)]
pub static mut KRT_NHEAP: usize = 0;
/// Where the stack starts (it may move when it grows).
#[unsafe(no_mangle)]
pub static mut KRT_STACK: usize = 0;
/// What `ksc` must agree with: this layout's version, the size of a
/// [`Value`] and of a frame, and the limits on frames and variables.
#[unsafe(no_mangle)]
#[used]
pub static KRT_ABI: [u64; 6] = [
    ABI,
    std::mem::size_of::<Value>() as u64,
    std::mem::size_of::<crate::Frame>() as u64,
    crate::MAX_DEPTH as u64,
    crate::MAX_LOCALS_ALIVE as u64,
    std::mem::size_of::<crate::Block>() as u64,
];
/// Bumped when the native code's view of the runtime changes.
pub const ABI: u64 = 6;
/// Room kept free on the stack past its top, for native pushes.
const SLACK: usize = 64;

/// The functions the native code calls, kept by the linker through this table.
#[used]
static KEEP: [extern "C" fn(); 5] = unsafe {
    [
        std::mem::transmute::<extern "C" fn(u32, i64, i64, i64, i64) -> Pair, extern "C" fn()>(krt_math),
        std::mem::transmute::<extern "C" fn(u32), extern "C" fn()>(krt_grow),
        std::mem::transmute::<extern "C" fn(u32, u32) -> u64, extern "C" fn()>(krt_op),
        std::mem::transmute::<extern "C" fn() -> Landing, extern "C" fn()>(krt_unwind),
        std::mem::transmute::<extern "C" fn() -> *mut Value, extern "C" fn()>(krt_locals),
    ]
};

/// Takes the stack's top, the frames and the variables from native code
/// (it may have pushed, popped or called).
fn sync_in(vm: &mut NativeVm) {
    let base = vm.stack.as_ptr() as usize;
    // What native code wrote up to these is plain data (`Value`, `Frame`),
    // within the room `sync_out` (or `Vm::new`) kept.
    unsafe {
        vm.stack.set_len((KRT_SP - base) / std::mem::size_of::<Value>());
        vm.frames.set_len(KRT_NFRAMES);
        vm.locals.set_len(KRT_NLOCALS);
    }
}

/// Gives native code the stack's top and room (the stack may have moved).
fn sync_out(vm: &mut NativeVm) {
    vm.stack.reserve(SLACK);
    let size = std::mem::size_of::<Value>();
    let base = vm.stack.as_ptr() as usize;
    unsafe {
        KRT_SP = base + vm.stack.len() * size;
        KRT_LIMIT = base + vm.stack.capacity() * size;
        KRT_STACK = base;
        KRT_FRAMES = vm.frames.as_ptr() as usize;
        KRT_NFRAMES = vm.frames.len();
        KRT_LOCALS = vm.locals.as_ptr() as usize;
        KRT_NLOCALS = vm.locals.len();
        KRT_HEAP = vm.heap.as_ptr() as usize;
        KRT_NHEAP = vm.heap.len();
    }
}

type NativeVm = Vm<'static, StdinLock<'static>, BufWriter<StdoutLock<'static>>>;

struct Runtime {
    vm: NativeVm,
    /// Each function's instructions in order: the instruction, its offset and size.
    ops: Vec<Vec<(Op, usize, usize)>>,
    /// Where the native code of a handler is: by function and offset.
    landings: HashMap<(usize, usize), usize>,
    /// Why the last instruction failed.
    pending: Option<String>,
    /// The last instruction was a `throw` that found its handler already.
    thrown: bool,
}

static mut RT: *mut Runtime = std::ptr::null_mut();

#[allow(static_mut_refs)]
fn rt() -> &'static mut Runtime {
    // Only the native program's thread runs, one call at a time.
    unsafe { &mut *RT }
}

/// Runs the native program, if this executable has one.
pub fn run_if_present(args: Vec<String>) -> Option<u8> {
    let p = unsafe { std::ptr::read_volatile(&raw const KRT_PROGRAM) };
    std::hint::black_box(&KEEP);
    if p.present != PRESENT {
        return None;
    }
    let base = (&raw const KRT_PROGRAM) as usize;
    let at = |off: i64| base.wrapping_add_signed(off as isize);
    let ksb = unsafe { std::slice::from_raw_parts(at(p.ksb) as *const u8, p.ksb_len as usize) };
    let module: &'static Module = match Module::from_bytes(ksb) {
        Ok(m) => Box::leak(Box::new(m)),
        Err(e) => {
            eprintln!("ksvm: the native program is damaged: {e}");
            return Some(2);
        }
    };
    let table = unsafe { std::slice::from_raw_parts(at(p.table) as *const u8, p.table_len as usize) };
    let (entries, landings) = read_table(table, at);
    let ops = module
        .functions
        .iter()
        .map(|f| {
            let mut out = Vec::new();
            let mut pc = 0;
            while pc < f.code.len() {
                let (op, size) = Op::decode(&f.code, pc).expect("verified");
                out.push((op, pc, size));
                pc += size;
            }
            out
        })
        .collect();
    let vm = Vm::new(module, io::stdin().lock(), BufWriter::new(io::stdout().lock())).with_args(args);
    let runtime = Box::new(Runtime { vm, ops, landings, pending: None, thrown: false });
    unsafe { RT = Box::into_raw(runtime) };
    let entry = module.entry as usize;
    if let Err(reason) = rt().vm.enter(entry, 0) {
        finish(Err(Trap { func: module.functions[entry].name.clone(), at: 0, reason }));
    }
    sync_out(&mut rt().vm);
    let main: extern "C" fn() = unsafe { std::mem::transmute(entries[entry]) };
    main();
    unreachable!("main ends the program in krt_op")
}

/// The table `ksc` writes: for each function, its address and the
/// addresses of its handlers (by offset in the `.ksb` code).
fn read_table(t: &[u8], at: impl Fn(i64) -> usize) -> (Vec<usize>, HashMap<(usize, usize), usize>) {
    let mut pos = 0;
    let mut take = |n: usize| {
        let s = &t[pos..pos + n];
        pos += n;
        s
    };
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().expect("4 bytes")) as usize;
    let i64_ = |s: &[u8]| i64::from_le_bytes(s.try_into().expect("8 bytes"));
    let n = u32_(take(4));
    let mut entries = Vec::with_capacity(n);
    let mut landings = HashMap::new();
    for f in 0..n {
        entries.push(at(i64_(take(8))));
        for _ in 0..u32_(take(4)) {
            let pc = u32_(take(4));
            landings.insert((f, pc), at(i64_(take(8))));
        }
    }
    (entries, landings)
}

/// Ends the program as `ksvm` does after `main`.
fn finish(r: Result<Value, Trap>) -> ! {
    let code = {
        let vm = &mut rt().vm;
        let _ = std::io::Write::flush(&mut vm.output);
        if r.is_err() {
            vm.scrub();
        }
        crate::exit_code(r)
    };
    std::process::exit(i32::from(code))
}

/// Makes room for `n` more values on the stack (it may move).
#[unsafe(no_mangle)]
extern "C" fn krt_grow(n: u32) {
    let vm = &mut rt().vm;
    sync_in(vm);
    vm.stack.reserve(n as usize + SLACK);
    sync_out(vm);
}

/// The variables of the frame running now (after a jump to a handler).
#[unsafe(no_mangle)]
extern "C" fn krt_locals() -> *mut Value {
    let vm = &mut rt().vm;
    let start = vm.frame().locals;
    vm.locals[start..].as_mut_ptr()
}

/// Runs instruction `idx` of function `func`. Gives 0, or for `br3` the arm
/// (0, 1, 2), for `jnat` 1 if it jumps; `u64::MAX` if it failed or threw.
#[unsafe(no_mangle)]
extern "C" fn krt_op(func: u32, idx: u32) -> u64 {
    let rt = rt();
    let vm = &mut rt.vm;
    sync_in(vm);
    let Some(&(op, pc, size)) = rt.ops[func as usize].get(idx as usize) else {
        // Past the last instruction: the code fell off its end.
        vm.op_at = vm.module.functions[func as usize].code.len();
        rt.pending = Some("reached the end of the function without ret".into());
        return u64::MAX;
    };
    vm.op_at = pc;
    vm.frame().pc = pc + size;
    let op = match op {
        Op::Br3 { .. } => Op::Br3 { neg: 0, zero: 1, pos: 2 },
        Op::JNat(_) => {
            vm.frame().pc = 0;
            Op::JNat(1)
        }
        op => op,
    };
    let r = vm.exec(op);
    sync_out(vm);
    match r {
        Ok(Some(v)) => finish(Ok(v)),
        Ok(None) => match op {
            Op::Br3 { .. } | Op::JNat(_) => vm.frame().pc as u64,
            Op::Throw => {
                rt.thrown = true;
                u64::MAX
            }
            _ => 0,
        },
        Err(reason) => {
            rt.pending = Some(reason);
            u64::MAX
        }
    }
}

/// Where the native code goes after a failure: a handler's address and its frame's stack pointer.
#[repr(C)]
pub struct Landing {
    addr: usize,
    sp: usize,
}

/// After `krt_op` gave `u64::MAX`: finds the handler, or ends the program.
#[unsafe(no_mangle)]
extern "C" fn krt_unwind() -> Landing {
    let rt = rt();
    sync_in(&mut rt.vm);
    if !std::mem::take(&mut rt.thrown) {
        let reason = rt.pending.take().unwrap_or_default();
        if let Err(trap) = rt.vm.fail(reason) {
            finish(Err(trap));
        }
    }
    let f = rt.vm.frames.last().expect("a handler's frame");
    let landing = Landing { addr: rt.landings[&(f.func, f.pc)], sp: f.native_sp };
    sync_out(&mut rt.vm);
    landing
}

/// Two 8-byte results (in `rax` and `rdx`).
#[repr(C)]
pub struct Pair {
    a: i64,
    b: i64,
}

/// What [`krt_math`] does, by number (`ksc` uses the same numbers).
pub mod math {
    pub const FADD: u32 = 0;
    pub const FSUB: u32 = 1;
    pub const FMUL: u32 = 2;
    pub const FDIV: u32 = 3;
    pub const FNEG: u32 = 4;
    pub const FCMP: u32 = 5;
    pub const ITOF: u32 = 6;
    pub const FTOI: u32 = 7;
    pub const TEQ: u32 = 8;
    pub const TCMP: u32 = 9;
    pub const TLEN: u32 = 10;
    pub const TIDX: u32 = 11;
}

/// The operations on reals and texts that need no more than their values:
/// `op` (see [`math`]) on `a` (its two 8-byte parts after the kind) and `b`.
/// Gives the result's parts, or `i64::MIN` first when the interpreter must
/// do it (a NaT, an overflow, an index out of bounds: it has the message).
/// Never makes a text, so nothing is collected.
#[unsafe(no_mangle)]
extern "C" fn krt_math(op: u32, a0: i64, a1: i64, b0: i64, b1: i64) -> Pair {
    use kestrel_core::{Real, Word};
    const NO: Pair = Pair { a: i64::MIN, b: 0 };
    let real = |m: i64, e: i64| Real::from_parts(m, e as i16);
    let out = |r: Real| {
        let (m, e) = r.parts();
        if r.is_nat() { NO } else { Pair { a: m, b: i64::from(e) } }
    };
    let int = |w: Word| match w.get() {
        Some(v) => Pair { a: v, b: 0 },
        None => NO,
    };
    let texts = &rt().vm.texts;
    let text = |t: i64| &texts[t as u32 as usize];
    match op {
        math::FADD => out(real(a0, a1) + real(b0, b1)),
        math::FSUB => out(real(a0, a1) - real(b0, b1)),
        math::FMUL => out(real(a0, a1) * real(b0, b1)),
        math::FDIV => out(real(a0, a1) / real(b0, b1)),
        math::FNEG => out(-real(a0, a1)),
        math::FCMP => int(real(a0, a1).cmp3(real(b0, b1))),
        math::ITOF => out(Real::from_word(Word::new(a0))),
        math::FTOI => int(real(a0, a1).to_word()),
        math::TEQ => Pair { a: if text(a0) == text(b0) { 1 } else { -1 }, b: 0 },
        math::TCMP => Pair { a: text(a0).cmp(text(b0)) as i64, b: 0 },
        math::TLEN => Pair { a: text(a0).len() as i64, b: 0 },
        math::TIDX => {
            let c = usize::try_from(b0).ok().and_then(|i| text(a0).get(i));
            match c.and_then(|c| kestrel_core::charset::char_to_tryte(*c)) {
                Some(v) => Pair { a: v, b: 0 },
                None => NO,
            }
        }
        _ => NO,
    }
}
