// A short tour of Kestrel: symmetric buffers, ownership, reals, tri and walk.

const GREETING: text = "Hello from the BET-VM!\n";

// Sum of a symmetric buffer with indices -k..+k.
proc sum(v: lent [int], k: int) -> int {
    acc: int @R0 = 0;
    walk i in span k {
        add acc, v[i];             // acc = acc + v[i]
    }
    yield acc;
}

// Prints a word as "negative", "zero" or "positive".
proc describe(x: int) {
    tri x <=> 0 {
        - => print("negative"),
        0 => print("zero"),
        + => print("positive"),
    }
}

proc main() -> int {
    print(GREETING);

    b: own [int] = claim 5;       // indices -2..+2
    walk i in span 2 {
        b[i] = i * i * i;          // -8 -1 0 1 8
    }
    print("cubes: ");
    walk i in span 2 { show(b[i]); print(" "); }
    print("\nsum = ");
    s: int @R+1 = sum(lend b, 2) on NaT { burn b; yield 1; };
    show(s);
    print(" (");
    describe(s);
    print(")\n");

    print("0t+0- = "); show(0t+0-);
    print(", 2 pow 10 = "); show(2 ** 10);
    print(", 1.0 / 3.0 = "); show(1.0 / 3.0);
    print("\n");

    t: tryte = 360;
    t = t + 10;                    // past 364: overflow becomes NaT, never wraps
    print("360 + 10 as a tryte = "); show(t); print("\n");

    dump("before burn");
    burn b;
    yield 0;
}
