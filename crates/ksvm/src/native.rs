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
/// What `ksc` must agree with: this layout's version and the size of a [`Value`].
#[unsafe(no_mangle)]
#[used]
pub static KRT_ABI: [u64; 2] = [ABI, std::mem::size_of::<Value>() as u64];
/// Bumped when the native code's view of the runtime changes.
pub const ABI: u64 = 3;
/// Room kept free on the stack past its top, for native pushes.
const SLACK: usize = 64;

/// The functions the native code calls, kept by the linker through this table.
#[used]
static KEEP: [extern "C" fn(); 5] = unsafe {
    [
        std::mem::transmute::<extern "C" fn(u32), extern "C" fn()>(krt_grow),
        std::mem::transmute::<extern "C" fn(usize) -> *mut Value, extern "C" fn()>(krt_enter_fn),
        std::mem::transmute::<extern "C" fn(u32, u32) -> u64, extern "C" fn()>(krt_op),
        std::mem::transmute::<extern "C" fn() -> Landing, extern "C" fn()>(krt_unwind),
        std::mem::transmute::<extern "C" fn() -> *mut Value, extern "C" fn()>(krt_locals),
    ]
};

/// Takes the stack's top from native code (it may have pushed or popped).
fn sync_in(vm: &mut NativeVm) {
    let base = vm.stack.as_ptr() as usize;
    // The values up to `KRT_SP` were written by native code (or are the
    // stack's own): `Value` is plain data, and `sync_out` kept the room.
    unsafe { vm.stack.set_len((KRT_SP - base) / std::mem::size_of::<Value>()) };
}

/// Gives native code the stack's top and room (the stack may have moved).
fn sync_out(vm: &mut NativeVm) {
    vm.stack.reserve(SLACK);
    let size = std::mem::size_of::<Value>();
    let base = vm.stack.as_ptr() as usize;
    unsafe {
        KRT_SP = base + vm.stack.len() * size;
        KRT_LIMIT = base + vm.stack.capacity() * size;
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
    if let Err(reason) = rt().vm.enter(entry, Vec::new()) {
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

/// Called first by each native function, with where its code keeps the
/// machine stack; gives its variables (native code keeps them in `r12`).
#[unsafe(no_mangle)]
extern "C" fn krt_enter_fn(sp: usize) -> *mut Value {
    let f = rt().vm.frame();
    f.native_sp = sp;
    f.locals.as_mut_ptr()
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
    rt().vm.frame().locals.as_mut_ptr()
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
