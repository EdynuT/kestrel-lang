//! The linker: object files (one per `.kstl` file) -> one program.
//!
//! Like `ld`: every function is found by name. A call to a function of
//! another object (an [`Kind::Import`]) is pointed at its code, shared
//! copies of generics are kept once, the constant and string pools are
//! merged, and the functions only the whole program can write are written
//! here: each interface call (`$dispatch.I.m`) and each interface free
//! (`$burnI.I`) tries the classes of every object.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::isa::Op;
use crate::ksb::{Function, Impl, Kind, Module};
use crate::{Word, Word27};

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    /// Two objects define the same function.
    Twice(String),
    /// A function is used but no object defines it.
    Missing(String),
    /// Two classes got the same number.
    SameTag(String, String),
    /// The entry function is not in the first object.
    NoEntry(String),
    /// An input is not an object file.
    NotObject,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Twice(n) => write!(f, "`{n}` is defined in two files"),
            LinkError::Missing(n) => write!(f, "`{n}` is used but no file defines it"),
            LinkError::SameTag(a, b) => write!(f, "classes `{a}` and `{b}` got the same number; rename one of them"),
            LinkError::NoEntry(n) => write!(f, "the program has no `{n}`"),
            LinkError::NotObject => f.write_str("not an object file"),
        }
    }
}

impl std::error::Error for LinkError {}

#[derive(Default)]
struct Pools {
    consts: Vec<Word27>,
    const_ids: HashMap<u64, u32>,
    strings: Vec<String>,
    string_ids: HashMap<String, u32>,
}

impl Pools {
    fn constant(&mut self, w: Word27) -> u32 {
        *self.const_ids.entry(w.bits()).or_insert_with(|| {
            self.consts.push(w);
            (self.consts.len() - 1) as u32
        })
    }

    fn string(&mut self, s: &str) -> u32 {
        if let Some(&k) = self.string_ids.get(s) {
            return k;
        }
        self.strings.push(s.to_string());
        let k = (self.strings.len() - 1) as u32;
        self.string_ids.insert(s.to_string(), k);
        k
    }
}

/// Links `objects` into a program whose entry is the function `entry` of
/// the first object (`None`: no entry, the first function; for tests).
pub fn link(objects: &[Module], entry: Option<&str>) -> Result<Module, LinkError> {
    if objects.iter().any(|o| o.link.is_none()) {
        return Err(LinkError::NotObject);
    }
    // Which copy of each function the program keeps.
    let mut index: HashMap<&str, u32> = HashMap::new();
    let mut kept: Vec<(usize, usize)> = Vec::new();
    for (o, m) in objects.iter().enumerate() {
        for (i, f) in m.functions.iter().enumerate() {
            match f.kind {
                Kind::Code if index.contains_key(f.name.as_str()) => return Err(LinkError::Twice(f.name.clone())),
                Kind::Code | Kind::Shared if !index.contains_key(f.name.as_str()) => {
                    index.insert(&f.name, kept.len() as u32);
                    kept.push((o, i));
                }
                _ => {}
            }
        }
    }
    // The functions written here, with what each needs.
    let mut dispatch: BTreeMap<&str, (u8, u16)> = BTreeMap::new();
    let mut burn_dispatch: BTreeMap<&str, ()> = BTreeMap::new();
    for f in objects.iter().flat_map(|o| &o.functions) {
        match f.kind {
            Kind::Dispatch { ret } => {
                dispatch.insert(&f.name, (f.params, ret));
            }
            Kind::BurnDispatch => {
                burn_dispatch.insert(&f.name, ());
            }
            _ => {}
        }
    }
    for (k, name) in (kept.len() as u32..).zip(dispatch.keys().chain(burn_dispatch.keys())) {
        index.insert(name, k);
    }
    // Class numbers must be unique; the same class in two objects (a copy of
    // a generic) has the same number.
    let mut tags: HashMap<i64, &str> = HashMap::new();
    for (name, tag) in objects.iter().flat_map(|o| &o.link.as_ref().expect("checked").classes) {
        match tags.get(tag) {
            Some(other) if other != name => return Err(LinkError::SameTag(other.to_string(), name.clone())),
            _ => {
                tags.insert(*tag, name);
            }
        }
    }

    let mut pools = Pools::default();
    let mut functions = Vec::new();
    for &(o, i) in &kept {
        let m = &objects[o];
        let f = &m.functions[i];
        let code = relocate(m, f, &index, &mut pools)?;
        functions.push(Function { code, kind: Kind::Code, ..f.clone() });
    }
    let mut impls: BTreeMap<&str, Vec<&Impl>> = BTreeMap::new();
    let mut burns: BTreeMap<&str, Vec<(i64, &str)>> = BTreeMap::new();
    for link in objects.iter().map(|o| o.link.as_ref().expect("checked")) {
        for i in &link.impls {
            let list = impls.entry(&i.dispatch).or_default();
            if !list.iter().any(|j| j.tag == i.tag) {
                list.push(i);
            }
        }
        for (d, tag, f) in &link.burns {
            let list = burns.entry(d).or_default();
            if !list.iter().any(|(t, _)| t == tag) {
                list.push((*tag, f));
            }
        }
    }
    let debug = functions.iter().all(|f| f.local_names.len() == usize::from(f.locals));
    for (name, &(params, ret)) in &dispatch {
        let list = impls.get(name).map(Vec::as_slice).unwrap_or_default();
        let mut f = dispatch_fn(name, params, ret, list, &index, &mut pools)?;
        if !debug {
            f.local_names.clear();
        }
        functions.push(f);
    }
    for name in burn_dispatch.keys() {
        let list = burns.get(name).map(Vec::as_slice).unwrap_or_default();
        let mut f = burn_fn(name, list, &index, &mut pools)?;
        if !debug {
            f.local_names.clear();
        }
        functions.push(f);
    }
    let entry = match entry {
        None => 0,
        Some(e) => match objects[0].functions.iter().find(|f| f.name == e && f.kind == Kind::Code) {
            Some(_) => index[e],
            None => return Err(LinkError::NoEntry(e.to_string())),
        },
    };
    Ok(Module { entry, consts: pools.consts, strings: pools.strings, functions, link: None })
}

/// The code of `f` (from object `m`) with every pool and function number
/// changed to the program's. Each instruction keeps its size, so jumps stay put.
fn relocate(m: &Module, f: &Function, index: &HashMap<&str, u32>, pools: &mut Pools) -> Result<Vec<u8>, LinkError> {
    let mut out = Vec::with_capacity(f.code.len());
    let mut at = 0;
    while at < f.code.len() {
        let (op, size) = Op::decode(&f.code, at).expect("objects are verified when read");
        let op = match op {
            Op::PushC(k) => Op::PushC(pools.constant(m.consts[k as usize])),
            Op::PushR(k) => Op::PushR(pools.constant(m.consts[k as usize])),
            Op::PushS(k) => Op::PushS(pools.string(&m.strings[k as usize])),
            Op::Dump(k) => Op::Dump(pools.string(&m.strings[k as usize])),
            Op::Trap(k) => Op::Trap(pools.string(&m.strings[k as usize])),
            Op::Call { f: callee, argc } => {
                let name = &m.functions[callee as usize].name;
                let f = *index.get(name.as_str()).ok_or_else(|| LinkError::Missing(name.clone()))?;
                Op::Call { f, argc }
            }
            op => op,
        };
        op.encode(&mut out);
        at += size;
    }
    Ok(out)
}

/// Code shared by both kinds of written function: `tag == k ? this : next`
/// for each class, `body` making the `this` part.
fn by_class(
    code: &mut Vec<u8>,
    pools: &mut Pools,
    tags: impl Iterator<Item = i64>,
    mut body: impl FnMut(&mut Vec<u8>, usize) -> Result<(), LinkError>,
) -> Result<(), LinkError> {
    for (k, tag) in tags.enumerate() {
        Op::Load(1).encode(code);
        Op::PushC(pools.constant(Word27::from(Word::new(tag)))).encode(code);
        Op::Cmp3.encode(code);
        Op::Test(0b010).encode(code);
        let br = code.len();
        Op::Br3 { neg: 0, zero: 0, pos: 0 }.encode(code);
        let this = code.len() as u32;
        body(code, k)?;
        let next = code.len() as u32;
        let mut patched = Vec::new();
        Op::Br3 { neg: next, zero: next, pos: this }.encode(&mut patched);
        code[br..br + patched.len()].copy_from_slice(&patched);
    }
    Ok(())
}

/// `$dispatch.I.m`: the receiver (pointer, class number) in slots 0 and 1,
/// the arguments after it; calls the method of the object's class.
fn dispatch_fn(
    name: &str,
    params: u8,
    ret: u16,
    impls: &[&Impl],
    index: &HashMap<&str, u32>,
    pools: &mut Pools,
) -> Result<Function, LinkError> {
    let mut code = Vec::new();
    by_class(&mut code, pools, impls.iter().map(|i| i.tag), |code, k| {
        let i = impls[k];
        let f = *index.get(i.function.as_str()).ok_or_else(|| LinkError::Missing(i.function.clone()))?;
        Op::Load(0).encode(code);
        let first = if i.whole { 1 } else { 2 };
        for s in first..u16::from(params) {
            Op::Load(s).encode(code);
        }
        let argc = params - if i.whole { 0 } else { 1 };
        Op::Call { f, argc }.encode(code);
        if ret == 1 { Op::Ret } else { Op::RetN(ret) }.encode(code);
        Ok(())
    })?;
    Op::Trap(pools.string("interface call on an object of an unknown class")).encode(&mut code);
    let mut local_names = vec!["$recv".to_string(), "$class".to_string()];
    local_names.resize(usize::from(params), "$arg".to_string());
    Ok(Function { name: name.to_string(), params, locals: u16::from(params), code, local_names, kind: Kind::Code, ..Default::default() })
}

/// `$burnI.I`: frees an object seen through an interface (pointer, class
/// number): with its class's `$burn.C` if its fields own something.
fn burn_fn(name: &str, burns: &[(i64, &str)], index: &HashMap<&str, u32>, pools: &mut Pools) -> Result<Function, LinkError> {
    let mut code = Vec::new();
    by_class(&mut code, pools, burns.iter().map(|b| b.0), |code, k| {
        let g = burns[k].1;
        let f = *index.get(g).ok_or_else(|| LinkError::Missing(g.to_string()))?;
        Op::Load(0).encode(code);
        Op::Call { f, argc: 1 }.encode(code);
        Op::Ret.encode(code);
        Ok(())
    })?;
    Op::Purge(0).encode(&mut code);
    Op::PushC(pools.constant(Word27::from(Word::ZERO))).encode(&mut code);
    Op::Ret.encode(&mut code);
    let local_names = vec!["$obj".to_string(), "$class".to_string()];
    Ok(Function { name: name.to_string(), params: 2, locals: 2, code, local_names, kind: Kind::Code, ..Default::default() })
}
