//! Compiles Kestrel programs and runs them on the VM.

use kestrel_core::{Module, Word};
use ksvm::{Trap, Value, Vm};

fn build(src: &str) -> Module {
    let m = ksc::compile(src).unwrap_or_else(|d| panic!("compile error: {}\n{}", d.message, d.render("test.ks", src)));
    // Everything the compiler emits must survive a write/read/verify round trip.
    Module::from_bytes(&m.to_bytes()).expect("emitted module verifies")
}

fn run_with(src: &str, input: &str) -> (Result<Value, Trap>, String) {
    let m = build(src);
    let mut out = Vec::new();
    let r = Vm::new(&m, input.as_bytes(), &mut out).run();
    (r, String::from_utf8(out).unwrap())
}

/// Runs `src`, expects success, returns stdout.
fn out(src: &str) -> String {
    let (r, o) = run_with(src, "");
    r.unwrap_or_else(|t| panic!("trapped: {t}\noutput so far: {o}"));
    o
}

fn main_out(body: &str) -> String {
    out(&format!("proc main() {{ {body} }}"))
}

#[test]
fn printf_formats() {
    let body = r#"n: int = -5; t: tryte = 'A'; name: text = "Ana";
        printf("{name} %s/{n} %d/{n:t} %t/{n:v} %v/{t:c}%c/%f\n", name, n * 2, 3, 100, 'b', 1.5);"#;
    assert_eq!(main_out(body), "Ana Ana/-5 -10/0t-++ 0t+0/0ve 0vDh/Ab/1.5\n");
    assert_eq!(main_out(r#"x: int = 0; printf("{x:t} {x:v}");"#), "0t0 0v0");
    assert_eq!(main_out(r#"x: int = 1; printf("{{x}} = {x}, %%d = %d, 100%%\n", x);"#), "{x} = 1, %d = 1, 100%\n");
    assert_eq!(main_out(r#"printl("Ação: \"sim\" e 'não' * <> @ #"); print(-'Á');"#), "Ação: \"sim\" e 'não' * <> @ #\n-54");
    assert_eq!(main_out(r#"n: int = 5; k: text = "t"; printf("exemplo de " + k + " = %t\n", n);"#), "exemplo de t = 0t+--\n");
}

#[test]
fn print_buffers() {
    let body = r#"b: own [int] = claim 5; walk i in -2..2 { b[i] = i * i * i; }
        printl(b); printf("{b:t} %v\n", b);
        t: own [tryte] = claim 3; t[-1] = 'O'; t[0] = 'l'; t[1] = 'á';
        printf("{t:c} {t}\n"); print(to_text(b) + "!"); burn b; burn t;"#;
    let e = ksc::compile("proc main() { b: own [int] = claim 3; burn b; print(b); }").unwrap_err();
    assert!(e.message.contains("after it was burned"), "{}", e.message);
    assert_eq!(main_out(body), "[-8 -1 0 1 8]\n[0t-0+ 0t- 0t0 0t+ 0t+0-] [0vh 0va 0v0 0vA 0vH]\nOlá [15 -12 -54]\n[-8 -1 0 1 8]!");
}

#[test]
fn to_text_values() {
    let body = r#"n: int = -5; t := to_text(n, 't'); printl("n = " + to_text(n) + " = " + t + " = " + to_text(n, 'v'));
        print(len(t)); print(" "); emit(t[2]); print(" " + to_text(1.0 / 4.0) + " " + to_text(NaT));"#;
    assert_eq!(main_out(body), "n = -5 = 0t-++ = 0ve\n5 - 0.25 NaT");
}

#[test]
fn print_concatenation() {
    assert_eq!(main_out(r#"name: text = "Kestrel"; print("Hello, " + name + "!" + "\n");"#), "Hello, Kestrel!\n");
}

#[test]
fn spec_example() {
    let src = r#"
proc sum(v: lent [int], k: int) -> int {
    acc: int = 0;
    walk i in span k { acc += v[i]; }
    yield acc;
}
proc main() -> int {
    b: own [int] = claim 5;
    walk i in span 2 { b[i] = i * i; }
    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
    s = s << 1;
    tri s <=> 10 { - => print("less"), 0 => print("equal"), + => print("greater"), }
    burn b;
    yield 0;
}"#;
    // 4 + 1 + 0 + 1 + 4 = 10, times 3 = 30 > 10.
    let (r, o) = run_with(src, "");
    assert_eq!(o, "greater");
    assert_eq!(r, Ok(Value::Word(Word::ZERO)));
}

#[test]
fn arithmetic_and_overflow() {
    assert_eq!(main_out(r#"print(1 + 2 * 3); print(" "); print(-2 ** 2); print(" "); print(2 ** 3 ** 2); print(" "); print(7 / 2); print(" "); print(-7 % 2);"#), "7 -4 512 3 -1");
    assert_eq!(main_out(r#"print(0t+0-); print(" "); print(0vAm); print(" "); print(2 >> 1); print(" "); print(2 / 3);"#), "8 14 1 0");
    // A tryte overflows into NaT at 364, a word only at 3.8 trillion.
    assert_eq!(main_out(r#"t: tryte = 300; t = t + 100; print(t); print(" "); w: int = 300; w = w + 100; print(w);"#), "NaT 400");
    assert_eq!(main_out("x: int = 3812798742493; print(x + 1); print(\" \"); print(1 / 0);"), "NaT NaT");
}

#[test]
fn reals_and_conversions() {
    assert_eq!(main_out(r#"print(0.2 + 0.1); print(" "); print(1.0 / 3.0); print(" "); print(2.0 ** 10);"#), "0.3 0.333333333 1024.0");
    assert_eq!(main_out(r#"print((int) 6.9); print(" "); print((int) -6.9); print(" "); print((real) 7 / 2.0); print(" "); print((tryte) 500);"#), "6 -6 3.5 NaT");
}

#[test]
fn shadowing_does_not_clobber_outer_variable() {
    // Prototype bug 6: an inner `let` overwrote the outer variable's slot.
    assert_eq!(main_out("x := 1; t: trit = 0; tri t { - => {} 0 => { x := 5; print(x); } + => {} } print(\" \"); print(x);"), "5 1");
}

#[test]
fn tri_and_logic() {
    let src = r#"
proc sign(x: int) { tri x <=> 0 { - => print("-"), 0 => print("0"), + => print("+"), } }
proc main() { sign(-5); sign(0); sign(9); a: trit = +; b: trit = 0; print(a && b); print(a || b); print(!a); }"#;
    assert_eq!(out(src), "-0+01-1");
}

#[test]
fn walks() {
    assert_eq!(main_out("walk i in -2..2 { print(i); }"), "-2-1012");
    assert_eq!(main_out("walk i in span 1 { print(i); }"), "-101");
    assert_eq!(main_out("walk i in 3..1 { print(i); } print(\"empty\");"), "empty");
    assert_eq!(main_out("walk i in 0..9 { tri i <=> 2 { - => continue, 0 => continue, + => {} } tri i <=> 5 { - => {} 0 => break, + => {} } print(i); }"), "34");
    // The loop compares before incrementing, so the top of the word range is fine.
    assert_eq!(main_out("walk i in 3812798742492..3812798742493 { print(\"x\"); }"), "xx");
}

#[test]
fn procs_buffers_and_ownership() {
    let src = r#"
proc fill(v: lent [int], k: int) { walk i in span k { v[i] = i * 10; } }
proc total(v: lent [int], k: int) -> int { s := 0; walk i in span k { s += v[i]; } yield s; }
proc make(n: int) -> own [int] { b: own [int] = claim 2 * n + 1; yield b; }
proc consume(b: own [int]) -> int { x := b[0]; burn b; yield x; }
proc main() {
    b: own [int] = make(2);
    fill(lend b, 2);
    print(b[-2]); print(" "); print(b[2]); print(" "); print(total(lend b, 2)); print(" ");
    print(consume(give b));
}"#;
    assert_eq!(out(src), "-20 20 0 0");
}

#[test]
fn recursion() {
    let src = "proc fact(n: int) -> int { tri n <=> 1 { - => yield 1, 0 => yield 1, + => yield n * fact(n - 1), } }
               proc main() { print(fact(15)); print(\" \"); print(fact(16)); }";
    // 15! = 1307674368000 fits in a word; 16! = 20922789888000 overflows into NaT.
    assert_eq!(out(src), "1307674368000 NaT");
}

#[test]
fn text_and_characters() {
    assert_eq!(main_out(r#"s := "Kestrel"; print(len(s)); print(" "); emit(s[0]); emit(-s[1]); emit(-'A'); emit('\n');"#), "7 KEa\n");
    assert_eq!(out(r#"const NAME: text = "Hi"; proc main() { print(NAME); print(NAME); }"#), "HiHi");
}

#[test]
fn input_and_on_nat() {
    let src = r#"
proc main() -> int {
    x := input() on NaT { print("bad input"); yield 1; };
    print(x * 2);
    yield 0;
}"#;
    assert_eq!(run_with(src, "21\n"), (Ok(Value::Word(Word::ZERO)), "42".to_string()));
    assert_eq!(run_with(src, "0t+0-\n").1, "16");
    assert_eq!(run_with(src, "banana\n"), (Ok(Value::Word(Word::ONE)), "bad input".to_string()));
}

#[test]
fn traps() {
    let (r, _) = run_with("proc main() { b: own [int] = claim 3; b[2] = 1; burn b; }", "");
    assert!(r.unwrap_err().reason.contains("index 2 outside -1..+1"));
    let (r, _) = run_with("proc main() { n := 4; b: own [int] = claim n; burn b; }", "");
    assert!(r.unwrap_err().reason.contains("odd"));
    let (r, _) = run_with("proc main() { t: trit = NaT; tri t { - => {} 0 => {} + => {} } }", "");
    assert_eq!(r.unwrap_err().reason, "tri/if/while condition is NaT");
    let (r, _) = run_with("proc main() { t: trit = 0; if t { print(1); } }", "");
    assert_eq!(r.unwrap_err().reason, "if condition is 0 (unknown)");
    let (r, _) = run_with("proc main() { x: int = NaT; if x > 0 { print(1); } }", "");
    assert_eq!(r.unwrap_err().reason, "tri/if/while condition is NaT");
}

#[test]
fn if_and_comparisons() {
    let body = r#"walk i in -1..1 {
            if i < 0 { print("neg "); } else if i == 0 { print("zero "); } else { print("pos "); }
        }
        a := 3; b := 5; r := 2.5;
        printf("%d%d%d%d%d%d ", a == b, a != b, a < b, a > b, a <= 3, a >= 4);
        printf("%d%d ", r < 3.0, r == 2.5);
        t: trit = +; u: trit = 0;
        printf("%d%d%d%d", !t, !u, t && u, !(t || u));"#;
    assert_eq!(main_out(body), "neg zero pos -111-11-1 11 -100-1");
}

#[test]
fn while_loops() {
    let body = r#"i := 0; while i < 5 { print(i); i = i + 1; }
        n := 0; while + { n = n + 1; if n == 3 { continue; } if n > 5 { break; } print(" "); print(n); }
        while i > 100 { print("never"); }"#;
    assert_eq!(main_out(body), "01234 1 2 4 5");
    let (r, _) = run_with("proc main() { t: trit = 0; while t { } }", "");
    assert_eq!(r.unwrap_err().reason, "while condition is 0 (unknown)");
    // collatz(27) takes 111 steps
    let src = r#"proc steps(n: int) -> int { k := 0; while n != 1 { if n % 2 == 0 { n = n / 2; } else { n = 3 * n + 1; } k = k + 1; } yield k; }
        proc main() { print(steps(27)); }"#;
    assert_eq!(out(src), "111");
}

#[test]
fn compound_assignment() {
    let body = r#"i := 0; while i < 3 { print(i); i++; }
        x := 10; x += 5; x -= 3; x *= 2; x /= 5; x %= 3; printf(" %d", x);
        n := 5; n--; printf(" %d", n);
        r := 1.5; r *= 2.0; r -= 0.5; printf(" %f", r);
        b: own [int] = claim 3; b[1] += 7; b[1]++; b[-1] -= 1; printf(" {b}", ); burn b;"#;
    assert_eq!(main_out(body), "012 1 4 2.5 [-1 0 8]");
    let e = ksc::compile("proc main() { i := 0; final f := 1; f++; }").unwrap_err();
    assert!(e.message.contains("is final"), "{}", e.message);
    let e = ksc::compile("proc main() { i := 0; x := i++; }").unwrap_err();
    assert!(e.message.contains("expected `;`"), "{}", e.message);
}

#[test]
fn short_circuit() {
    // The right side is skipped when the left decides: no index trap on an empty text.
    let body = r#"s: text = ""; if len(s) > 0 && s[0] == 'a' { print("a"); } else { print("empty"); }
        if len(s) == 0 || s[0] == 'a' { print(" ok"); }
        n: trit = NaT; f: trit = -; z: trit = 0; p: trit = +;
        printf(" %d %d %d %d %d", f && n, p || n, n && f, z && f, z || p);"#;
    assert_eq!(main_out(body), "empty ok -1 1 NaT -1 1");
}

#[test]
fn dump_shows_variables() {
    let o = main_out("low: int = -1; acc: int = 8; dump(\"here\");");
    assert!(o.contains("R-4  low"), "{o}");
    assert!(o.contains("R-3  acc        = 8"), "{o}");
}
