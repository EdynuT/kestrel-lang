// A short tour of Kestrel: symmetric buffers, ownership, reals, tri and walk.

const GREETING: text = "Hello from the BET-VM!\n";

// Sum of a symmetric buffer with indices -k..+k.
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    yield acc;
}

// Prints a word as "negative", "zero" or "positive".
proc describe(x: int) {
    tri x <=> 0 {
        - => print("negative");
        0 => print("zero");
        + => print("positive");
    }
}

proc main() -> int {
    print(GREETING);

    sym b[]: int = claim 5;       // indices -2..+2
    walk i in span 2 {
        b[i] = i * i * i;          // -8 -1 0 1 8
    }
    print("cubes: ");
    walk i in span 2 { print(b[i]); print(" "); }
    print("\nsum = ");
    s: int = sum(lend b, 2) on NaT { burn b; yield 1; };
    print(s);
    print(" (");
    describe(s);
    print(")\n");

    print("0t+0- = "); print(0t+0-);
    print(", 2 pow 10 = "); print(2 ** 10);
    print(", 1.0 / 3.0 = "); print(1.0 / 3.0);
    printl();

    t: tryte = 360;
    t = t + 10;                    // past 364: overflow becomes NaT, never wraps
    print("360 + 10 as a tryte = "); printl(t);

    dump("before burn");
    burn b;
    yield 0;
}
