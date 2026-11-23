//! Limits shared by the compiler and the VM. All are powers of three.

/// Nesting of blocks, parentheses and operator chains: 3^6, every value of a tryte.
pub const MAX_NESTING: u32 = 729;
/// Calls active at the same time in the VM: 3^8.
pub const MAX_CALL_DEPTH: usize = 6561;
/// Parameters of one proc: 3^5, fits the `u8` argument count of a `.kstb` call.
pub const MAX_PARAMS: usize = 243;
/// Variables of one proc: 3^10, fits the `u16` slot number of a `.kstb` instruction.
pub const MAX_LOCALS: usize = 59049;
