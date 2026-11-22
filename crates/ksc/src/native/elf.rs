//! Reading and writing ELF64 files (x86-64, little-endian): what the native
//! code needs to be put into a copy of the runtime (`ksvm`).
//!
//! The runtime is left as it is: one more loadable segment is added at
//! its end (the code, the program and a new table of program headers),
//! and the ELF header is pointed at the new table.

pub const PT_LOAD: u32 = 1;
pub const PT_PHDR: u32 = 6;
pub const PF_X: u32 = 1;
pub const PF_R: u32 = 4;
const PHDR_SIZE: usize = 56;
pub const PAGE: u64 = 0x1000;

#[derive(Clone, Copy, Debug)]
pub struct Phdr {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

impl Phdr {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for v in [self.offset, self.vaddr, self.vaddr, self.filesz, self.memsz, self.align] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

pub struct Elf<'a> {
    pub bytes: &'a [u8],
    pub phdrs: Vec<Phdr>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Elf<'a> {
    pub fn read(bytes: &'a [u8]) -> Result<Elf<'a>, String> {
        let bad = || "the runtime is not an x86-64 ELF executable".to_string();
        if bytes.get(..4) != Some(b"\x7fELF") || bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) || u16_at(bytes, 18) != Some(62) {
            return Err(bad());
        }
        let phoff = u64_at(bytes, 32).ok_or_else(bad)? as usize;
        let phnum = usize::from(u16_at(bytes, 56).ok_or_else(bad)?);
        let mut phdrs = Vec::new();
        for k in 0..phnum {
            let at = phoff + k * PHDR_SIZE;
            let f = |o: usize| u64_at(bytes, at + o).ok_or_else(bad);
            phdrs.push(Phdr {
                kind: u32_at(bytes, at).ok_or_else(bad)?,
                flags: u32_at(bytes, at + 4).ok_or_else(bad)?,
                offset: f(8)?,
                vaddr: f(16)?,
                filesz: f(32)?,
                memsz: f(40)?,
                align: f(48)?,
            });
        }
        Ok(Elf { bytes, phdrs })
    }

    /// The address of symbol `name` (from the symbol table, `.symtab`).
    pub fn symbol(&self, name: &str) -> Option<u64> {
        let b = self.bytes;
        let shoff = u64_at(b, 40)? as usize;
        let shnum = usize::from(u16_at(b, 60)?);
        let section = |k: usize| shoff + k * 64;
        for k in 0..shnum {
            let s = section(k);
            if u32_at(b, s + 4)? != 2 {
                continue; // not SHT_SYMTAB
            }
            let (off, size) = (u64_at(b, s + 24)? as usize, u64_at(b, s + 32)? as usize);
            let strtab = section(u32_at(b, s + 40)? as usize);
            let str_off = u64_at(b, strtab + 24)? as usize;
            for sym in (off..off + size).step_by(24) {
                let n = str_off + u32_at(b, sym)? as usize;
                let end = b.get(n..)?.iter().position(|&c| c == 0)?;
                if &b[n..n + end] == name.as_bytes() {
                    return u64_at(b, sym + 8);
                }
            }
        }
        None
    }

    /// Where in the file the byte at `vaddr` is (it must be stored, not zero-filled).
    pub fn file_offset(&self, vaddr: u64) -> Option<usize> {
        let p = self.phdrs.iter().find(|p| p.kind == PT_LOAD && p.vaddr <= vaddr && vaddr < p.vaddr + p.filesz)?;
        Some((vaddr - p.vaddr + p.offset) as usize)
    }

    /// Where a new segment can go: past every loaded byte, on a fresh page.
    pub fn free_vaddr(&self) -> u64 {
        let end = self.phdrs.iter().filter(|p| p.kind == PT_LOAD).map(|p| p.vaddr + p.memsz).max().unwrap_or(0);
        end.div_ceil(PAGE) * PAGE
    }

    /// The runtime with one more read-and-execute segment at `vaddr`
    /// (from [`Elf::free_vaddr`]) holding `contents`; `patch` changes bytes of
    /// the runtime first (offset, new bytes).
    pub fn with_segment(&self, vaddr: u64, mut contents: Vec<u8>, patch: &[(usize, Vec<u8>)]) -> Vec<u8> {
        let mut out = self.bytes.to_vec();
        for (at, b) in patch {
            out[*at..*at + b.len()].copy_from_slice(b);
        }
        let offset = (out.len() as u64).div_ceil(PAGE) * PAGE;
        out.resize(offset as usize, 0);
        // The new table of program headers goes at the end of the segment,
        // so it is loaded too (the loader reads it from memory).
        while !contents.len().is_multiple_of(8) {
            contents.push(0);
        }
        let table = contents.len() as u64;
        let n = self.phdrs.len() + 1;
        let size = table + (n * PHDR_SIZE) as u64;
        let seg = Phdr { kind: PT_LOAD, flags: PF_R | PF_X, offset, vaddr, filesz: size, memsz: size, align: PAGE };
        let last_load = self.phdrs.iter().rposition(|p| p.kind == PT_LOAD).unwrap_or(0);
        let mut phdrs = self.phdrs.clone();
        phdrs.insert(last_load + 1, seg);
        for p in &mut phdrs {
            if p.kind == PT_PHDR {
                p.offset = offset + table;
                p.vaddr = vaddr + table;
                p.filesz = (n * PHDR_SIZE) as u64;
                p.memsz = p.filesz;
            }
        }
        for p in &phdrs {
            p.write(&mut contents);
        }
        out.extend_from_slice(&contents);
        out[32..40].copy_from_slice(&(offset + table).to_le_bytes());
        out[56..58].copy_from_slice(&(n as u16).to_le_bytes());
        out
    }
}
