//! Shared definitions for the Kestrel toolchain: ternary word types,
//! the BET-VM instruction set and the `.ksb` bytecode format.

pub mod bet;
pub mod charset;
pub mod isa;
pub mod limits;
pub mod link;
pub mod ksb;
pub mod real;
pub mod word;

pub use bet::Word27;
pub use isa::Op;
pub use ksb::Module;
pub use real::Real;
pub use word::Word;

/// A class's number, from its name (FNV-1a), so every file of a program
/// gives a class the same one without talking to the others; never 0.
pub fn class_tag(name: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h % word::WORD_MAX as u64) as i64 + 1
}

/// Magic bytes at the start of every `.ksb` file.
pub const KSB_MAGIC: [u8; 4] = *b"K3B\x01";
