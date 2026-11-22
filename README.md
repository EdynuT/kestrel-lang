# Kestrel

An experimental programming language for a **balanced-ternary** virtual machine.

Kestrel targets the BET-VM, a simulated 27-trit processor where every digit is `-1`, `0` or `+1`. The language has three-valued Kleene logic, a poison value (`NaT`) for invalid results instead of silent wrap-around, strict linear ownership with no garbage collector, and arrays that can be indexed from the center (`sym`).

> **Status:** hobby project. The [specification](SPEC.md), the compiler (`ksc`) and the VM (`ksvm`) work end to end. See [`crates/ksc/tests/tour.ks`](crates/ksc/tests/tour.ks).

## A taste

```
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {             // i goes from -k to +k
        acc += v[i];
    }
    return acc;
}

proc main() -> int {
    sym b[]: int = claim 5;       // indices -2..+2
    walk i in span 2 { b[i] = i * i; }

    s: int = sum(&b, 2) on NaT { burn b; return -1; };

    tri s <=> 10 {                 // three-way branch
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;                        // zero the memory and free it, exactly once
    return 0;
}
```

## Key ideas

| Concept | Kestrel | Closest C / Rust equivalent |
|---|---|---|
| Digit | `trit`: `-`, `0`, `+` | `bit` |
| Integers | `tryte` (6 trits, ±364), `int` (27 trits, ±3.8 × 10¹²) | `i16`, `i64` |
| Floating point | `real` (21-trit mantissa, 6-trit exponent) | `float` / `double` |
| Strings | `text` | `const char*` |
| Missing value | `int?`, `Conta?` hold `null`; `x?.f()`, `x ?: 0` | Kotlin `T?`, Rust `Option<T>` |
| Overflow | Produces `NaT`, never wraps around | Wraps or is undefined behavior |
| Branching | `tri x <=> y { - 0 + }` | `match x.cmp(&y)` |
| Memory | an array `x[]: int` must be `burn`ed or `give`n exactly once | `Box<[T]>` without `Drop` |
| Borrowing | `&v[]: int`, `c: &Conta`, caller writes `&x` | `&mut [T]`, `&mut T` |
| Arrays | `a[]: int = {1, 2, 3}` (0-based) or `sym s[]: int = claim 5` (`-2..+2`) | `int[]`, or `malloc` with the pointer in the middle |
| Errors | `proc f() throws NotFound`, `try { } catch e: NotFound { }`; checked by the compiler; VM failures are a catchable `Trap` | Java checked exceptions |
| Files | `use shapes.Circle;` reads `src/shapes/Circle.ks`, then `Circle.area(2.0)`; only `public` items are visible; each file compiles to its own `.ksb`, linked into one executable | C `#include` + `ld`, without writing headers |

Negation is free in balanced ternary: flip every trit (`0t+0-` = 8, `0t-0+` = −8). There is no sign bit and no two's complement.

## Layout

```
crates/
├── kestrel-core/   ternary int and real types, instruction set, .ksb format, linker
├── ksc/            compiler: lexer, parser, modules, type checker, checked errors, ownership checker, codegen, native x86-64 backend (own assembler and ELF writer)
└── ksvm/           virtual machine, runs .ksb; the runtime of native executables
std/                the standard library (Kestrel), built into ksc
SPEC.md             language and VM specification (Portuguese)
```

## Building

Requires a recent stable Rust toolchain.

```
cargo build --release   # ksc and ksvm in target/release (keep them together: ksvm is the runtime of every native executable)
ksc new Game         # creates Game/ with src/Main.ks
cd Game
ksc run              # compiles src/ into build/obj/ and the executable build/Game, then runs it
ksc test             # runs every @test proc in src/
ksc check File.ks    # only reports errors (for editors; `--stdin` reads the text)
ksc build --ksb      # writes build/Game.ksb instead, to run with ksvm
ksc File.ks          # one file, gcc style: writes the executable File
ksc File.ks --disasm # shows the bytecode
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

Copyright (C) 2026 Edynu

Kestrel is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version. See [LICENSE](LICENSE).
