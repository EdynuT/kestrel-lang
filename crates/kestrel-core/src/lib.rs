//! Shared definitions for the Kestrel toolchain: ternary word types,
//! the BET-VM instruction set and the `.ksb` bytecode format.

pub mod bet;
pub mod real;
pub mod word;

pub use bet::Word27;
pub use real::Real;
pub use word::Word;

/// Magic bytes at the start of every `.ksb` file.
pub const KSB_MAGIC: [u8; 4] = *b"K3B\x01";
