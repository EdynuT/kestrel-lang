//! ksvm: the BET virtual machine (runs `.ksb` files).

fn main() {
    println!("ksvm {} (ksb magic {:?})", env!("CARGO_PKG_VERSION"), kestrel_core::KSB_MAGIC);
}
