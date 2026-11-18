//! ksc: the Kestrel compiler (`.ks` -> `.ksb`).

fn main() {
    println!("ksc {} (ksb magic {:?})", env!("CARGO_PKG_VERSION"), kestrel_core::KSB_MAGIC);
}
