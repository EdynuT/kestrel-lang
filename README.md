# Kestrel

An experimental programming language for a **balanced-ternary** virtual machine.

Kestrel targets the BET-VM, a simulated 27-trit processor where every digit is `-1`, `0` or `+1`. The language has three-valued Kleene logic, a poison value (`NaT`) for invalid results instead of silent wrap-around, strict linear ownership with no garbage collector, and arrays indexed from the center.

> **Status:** hobby project, early stage. The [specification](SPEC.md) is done. The compiler (`ksc`) and VM (`ksvm`) are still being written.

## A taste

```
proc sum(v: lent [word], k: word) -> word {
    acc: word @R0 = 0;             // pin acc to register R0
    walk i in span k {             // i goes from -k to +k
        add acc, v[i];             // acc = acc + v[i]
    }
    yield acc;
}

proc main() -> word {
    b: own [word] = claim 5;       // indices -2..+2
    walk i in span 2 { b[i] = i * i; }

    s: word = sum(lend b, 2) on NaT { burn b; yield -1; };

    tri s <=> 10 {                 // three-way branch
        - => print("less"),
        0 => print("equal"),
        + => print("greater"),
    }
    burn b;                        // zero the memory and free it, exactly once
    yield 0;
}
```

## Key ideas

| Concept | Kestrel | Closest C / Rust equivalent |
|---|---|---|
| Digit | `trit`: `-`, `0`, `+` | `bit` |
| Integers | `tryte` (6 trits, ±364), `word` (27 trits, ±3.8 × 10¹²) | `i16`, `i64` |
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
├── kestrel-core/   ternary word types, instruction set, .ksb format
├── ksc/            compiler   .ks  -> .ksb
└── ksvm/           virtual machine, runs .ksb
SPEC.md             language and VM specification (Portuguese)
```

## Building

Requires a recent stable Rust toolchain.

```
cargo build --release
cargo run -p ksc  -- program.ks      # compile (work in progress)
cargo run -p ksvm -- program.ksb     # run (work in progress)
```

## License

[MIT](LICENSE)
# kestrel-lang
