//! Shared definitions for the Kestrel toolchain: ternary word types,
//! the BET-VM instruction set and the `.ksb` bytecode format.

/// Magic bytes at the start of every `.ksb` file.
pub const KSB_MAGIC: [u8; 4] = *b"K3B\x01";
