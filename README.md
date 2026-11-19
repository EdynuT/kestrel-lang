# Kestrel

An experimental programming language for a **balanced-ternary** virtual machine.

Kestrel targets the BET-VM, a simulated 27-trit processor where every digit is `-1`, `0` or `+1`. The language has three-valued Kleene logic, a poison value (`NaT`) for invalid results instead of silent wrap-around, strict linear ownership with no garbage collector, and arrays indexed from the center.

> **Status:** hobby project. The [specification](SPEC.md), the compiler (`ksc`) and the VM (`ksvm`) work end to end. See [`examples/tour.ks`](examples/tour.ks).

## A taste

```
proc sum(v: lent [int], k: int) -> int {
    acc: int = 0;
    walk i in span k {             // i goes from -k to +k
        acc += v[i];
    }
    yield acc;
}

proc main() -> int {
    b: own [int] = claim 5;       // indices -2..+2
    walk i in span 2 { b[i] = i * i; }

    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };

    tri s <=> 10 {                 // three-way branch
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;                        // zero the memory and free it, exactly once
    yield 0;
}
```

## Key ideas

| Concept | Kestrel | Closest C / Rust equivalent |
|---|---|---|
| Digit | `trit`: `-`, `0`, `+` | `bit` |
| Integers | `tryte` (6 trits, ±364), `int` (27 trits, ±3.8 × 10¹²) | `i16`, `i64` |
| Floating point | `real` (21-trit mantissa, 6-trit exponent) | `float` / `double` |
| Strings | `text` | `const char*` |
| Invalid value | `NaT`, which poisons every later operation | `NaN`, `None` |
| Overflow | Produces `NaT`, never wraps around | Wraps or is undefined behavior |
| Branching | `tri x <=> y { - 0 + }` | `match x.cmp(&y)` |
| Memory | `own [T]` must be `burn`ed or `give`n exactly once | `Box<[T]>` without `Drop` |
| Borrowing | `lent [T]` | `&mut [T]` |
| Arrays | `claim 5` gives indices `-2..+2` | `malloc`, but the pointer is in the middle |

Negation is free in balanced ternary: flip every trit (`0t+0-` = 8, `0t-0+` = −8). There is no sign bit and no two's complement.

## Layout

```
crates/
├── kestrel-core/   ternary int and real types, instruction set, .ksb format
├── ksc/            compiler: lexer, parser, type checker, ownership checker, codegen
└── ksvm/           virtual machine, runs .ksb
examples/           sample programs
SPEC.md             language and VM specification (Portuguese)
```

## Building

Requires a recent stable Rust toolchain.

```
cargo build --release
./target/release/ksc examples/tour.ks          # writes examples/tour.ksb
./target/release/ksvm examples/tour.ksb        # runs it
./target/release/ksc examples/tour.ks --disasm # shows the bytecode
```

The compiler rejects ownership mistakes before anything runs:

```
error: the arms of this `tri` leave `data` in different states
 --> leak.ks:4:5
  |
4 |     tri t {
  |     ^^^^^^^
  = note: `-` arm: burned, `0` arm: burned, `+` arm: still owned (every arm that continues must agree)
```

## License

[MIT](LICENSE)
