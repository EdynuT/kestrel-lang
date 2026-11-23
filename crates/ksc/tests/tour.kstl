// A short tour of Kestrel: symmetric buffers, ownership, reals, tri and walk.

use std.io.Console;
use std.debug.Debug;

const GREETING: text = "Hello from the BET-VM!\n";

// Sum of a symmetric buffer with indices -k..+k.
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

// Prints a word as "negative", "zero" or "positive".
proc describe(x: int) {
    tri x <=> 0 {
        - => Console.print("negative");
        0 => Console.print("zero");
        + => Console.print("positive");
    }
}

proc main() -> int {
    Console.print(GREETING);

    sym b[]: int = claim 5;       // indices -2..+2
    walk i in span 2 {
        b[i] = i * i * i;          // -8 -1 0 1 8
    }
    Console.print("cubes: ");
    walk i in span 2 { Console.print(b[i]); Console.print(" "); }
    Console.print("\nsum = ");
    s: int = sum(&b, 2) on NaT { burn b; return 1; };
    Console.print(s);
    Console.print(" (");
    describe(s);
    Console.print(")\n");

    Console.print("0t+0- = "); Console.print(0t+0-);
    Console.print(", 2 pow 10 = "); Console.print(2 ** 10);
    Console.print(", 1.0 / 3.0 = "); Console.print(1.0 / 3.0);
    Console.printl();

    t: tryte = 360;
    // Past 364 a tryte overflows. That stops the program, unless the line
    // has `on NaT`: then the handler runs instead.
    u: tryte = t + 10 on NaT { Console.printl("360 + 10 does not fit in a tryte"); burn b; return 1; };
    Console.printl(u);

    Debug.dump("before burn");
    burn b;
    return 0;
}
