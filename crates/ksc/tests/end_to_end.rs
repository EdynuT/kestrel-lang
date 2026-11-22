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

/// Runs `main` with `body`, expects a trap, returns its reason.
fn main_trap(body: &str) -> String {
    let (r, o) = run_with(&format!("proc main() {{ {body} }}"), "");
    r.err().unwrap_or_else(|| panic!("expected a trap, got output {o:?}")).reason
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
    let body = r#"sym b[]: int = claim 5; walk i in -2..2 { b[i] = i * i * i; }
        printl(b); printf("{b:t} %v\n", b);
        sym t[]: tryte = claim 3; t[-1] = 'O'; t[0] = 'l'; t[1] = 'á';
        printf("{t:c} {t}\n"); print(toText(b) + "!"); burn b; burn t;"#;
    let e = ksc::compile("proc main() { sym b[]: int = claim 3; burn b; print(b); }").unwrap_err();
    assert!(e.message.contains("after it was burned"), "{}", e.message);
    assert_eq!(main_out(body), "[-8, -1, 0, 1, 8]\n[0t-0+, 0t-, 0t0, 0t+, 0t+0-] [0vh, 0va, 0v0, 0vA, 0vH]\nOlá [15, -12, -54]\n[-8, -1, 0, 1, 8]!");
}

#[test]
fn to_text_values() {
    let body = r#"n: int = -5; t: text = toText(n, 't'); printl("n = " + toText(n) + " = " + t + " = " + toText(n, 'v'));
        print(len(t)); print(" "); emit(t[2]); print(" " + toText(1.0 / 4.0));"#;
    assert_eq!(main_out(body), "n = -5 = 0t-++ = 0ve\n5 - 0.25");
    // Characters: a tryte, or an array of trytes, becomes a text.
    let body = r#"cs[]: tryte = {'K', 's'}; append(cs, '!'); empty[]: tryte = {};
        printl(toText(cs, 'c') + toText('?', 'c') + "[" + toText(empty, 'c') + "]"); burn cs; burn empty;"#;
    assert_eq!(main_out(body), "Ks!?[]\n");
    assert!(main_trap("c: tryte = 300; printl(toText(c, 'c'));").contains("not a character"));
    let e = ksc::compile("proc main() { x: int = 5; printl(toText(x, 'c')); }").unwrap_err();
    assert!(e.message.contains("takes a tryte or an array of trytes"));
}

#[test]
fn print_concatenation() {
    assert_eq!(main_out(r#"name: text = "Kestrel"; print("Hello, " + name + "!" + "\n");"#), "Hello, Kestrel!\n");
}

#[test]
fn spec_example() {
    let src = r#"
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k { acc += v[i]; }
    return acc;
}
proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 { b[i] = i * i; }
    s: int = sum(&b, 2) on NaT { burn b; return -1; };
    s = s << 1;
    tri s <=> 10 { - => print("less"); 0 => print("equal"); + => print("greater"); }
    burn b;
    return 0;
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
    // A tryte overflows at 364, a word only at 3.8 trillion. Both trap...
    assert_eq!(main_out("w: int = 300; w = w + 100; print(w);"), "400");
    assert!(main_trap("t: tryte = 300; t = t + 100;").contains("overflow: 400 does not fit in a tryte"));
    assert!(main_trap("x: int = 3812798742493; print(x + 1);").contains("overflow: 3812798742493 + 1 does not fit in an int"));
    assert!(main_trap("print(1 / 0);").contains("division by zero"));
    assert!(main_trap("print(1 % 0);").contains("remainder of a division by zero"));
    assert!(main_trap("print(2 ** -1);").contains("exponent cannot be negative"));
    // A null cannot reach arithmetic: the compiler asks for a check first.
    let e = ksc::compile("proc main() { x: int? = null; print(x + 1); }").unwrap_err();
    assert!(e.message.contains("it may be null"), "{}", e.message);
    // ...unless the line has `on NaT`: then the whole line gives NaT to the handler.
    let body = r#"for d: int = -1; d <= 1; d++ {
            q: int = (10 + 2) / d * 3 on NaT { print("[/0]"); continue; };
            printf("%d ", q);
        }
        t: tryte = 300;
        u: tryte = t + 100 on NaT { print(" tryte"); return; };"#;
    assert_eq!(main_out(body), "-36 [/0]36  tryte");
    // Soft mode covers only that line, not the procs it calls.
    let src = "proc div(a: int, b: int) -> int { return a / b; }
               proc main() { x: int = div(1, 0) on NaT { print(\"caught\"); return; }; }";
    assert!(run_with(src, "").0.unwrap_err().reason.contains("division by zero"));
    // A proc reports "no result" by returning null (`-> int?`); `on NaT` handles it.
    let src = "proc safe(a: int, b: int) -> int? { if b == 0 { return null; } return a / b; }
               proc main() { x: int = safe(1, 0) on NaT { print(\"no result\"); return; }; print(x); }";
    assert_eq!(out(src), "no result");
}

#[test]
fn reals_and_conversions() {
    assert_eq!(main_out(r#"print(0.2 + 0.1); print(" "); print(1.0 / 3.0); print(" "); print(2.0 ** 10);"#), "0.3 0.333333333 1024.0");
    assert_eq!(main_out(r#"print((int) 6.9); print(" "); print((int) -6.9); print(" "); print((real) 7 / 2.0);"#), "6 -6 3.5");
    assert!(main_trap("print((tryte) 500);").contains("500 does not fit in a tryte (-364..364)"));
    assert!(main_trap("print(1.0 / 0.0);").contains("division by zero"));
    assert_eq!(main_out(r#"r: real = 1.0 / 0.0 on NaT { print("caught"); return; };"#), "caught");
}

#[test]
fn shadowing_does_not_clobber_outer_variable() {
    // Prototype bug 6: an inner `let` overwrote the outer variable's slot.
    assert_eq!(main_out("x: int = 1; t: trit = 0; tri t { - => {} 0 => { x: int = 5; print(x); } + => {} } print(\" \"); print(x);"), "5 1");
}

#[test]
fn tri_and_logic() {
    let src = r#"
proc sign(x: int) { tri x <=> 0 { - => print("-"); 0 => print("0"); + => print("+"); } }
proc main() { sign(-5); sign(0); sign(9); a: trit = +; b: trit = 0; print(a && b); print(a || b); print(!a); }"#;
    assert_eq!(out(src), "-0+01-1");
}

#[test]
fn walks() {
    assert_eq!(main_out("walk i in -2..2 { print(i); }"), "-2-1012");
    assert_eq!(main_out("walk i in span 1 { print(i); }"), "-101");
    assert_eq!(main_out("walk i in 3..1 { print(i); } print(\"empty\");"), "empty");
    assert_eq!(main_out("walk i in 0..9 { tri i <=> 2 { - => continue; 0 => continue; + => {} } tri i <=> 5 { - => {} 0 => break; + => {} } print(i); }"), "34");
    // The loop compares before incrementing, so the top of the word range is fine.
    assert_eq!(main_out("walk i in 3812798742492..3812798742493 { print(\"x\"); }"), "xx");
}

#[test]
fn procs_buffers_and_ownership() {
    let src = r#"
proc fill(sym &v[]: int, k: int) { walk i in span k { v[i] = i * 10; } }
proc total(sym &v[]: int, k: int) -> int { s: int = 0; walk i in span k { s += v[i]; } return s; }
proc make(n: int) -> sym int[] { sym b[]: int = claim 2 * n + 1; return b; }
proc consume(sym b[]: int) -> int { x: int = b[0]; burn b; return x; }
proc main() {
    sym b[]: int = make(2);
    fill(&b, 2);
    print(b[-2]); print(" "); print(b[2]); print(" "); print(total(&b, 2)); print(" ");
    print(consume(give b));
}"#;
    assert_eq!(out(src), "-20 20 0 0");
}

#[test]
fn recursion() {
    let src = "proc fact(n: int) -> int { tri n <=> 1 { - => return 1; 0 => return 1; + => return n * fact(n - 1); } }
               proc main() { print(fact(15)); f: int = fact(16) on NaT { print(\" big\"); return; }; print(f); }";
    // 15! = 1307674368000 fits in a word; 16! = 20922789888000 overflows.
    let (r, o) = run_with(src, "");
    assert_eq!(o, "1307674368000");
    assert!(r.unwrap_err().reason.contains("overflow: 16 * 1307674368000"));
}

#[test]
fn text_and_characters() {
    assert_eq!(main_out(r#"s: text = "Kestrel"; print(len(s)); print(" "); emit(s[0]); emit(-s[1]); emit(-'A'); emit('\n');"#), "7 KEa\n");
    assert_eq!(out(r#"const NAME: text = "Hi"; proc main() { print(NAME); print(NAME); }"#), "HiHi");
}

#[test]
fn input_and_on_nat() {
    let src = r#"
proc main() -> int {
    x: int = input() on NaT { print("bad input"); return 1; };
    print(x * 2);
    return 0;
}"#;
    assert_eq!(run_with(src, "21\n"), (Ok(Value::Word(Word::ZERO)), "42".to_string()));
    assert_eq!(run_with(src, "0t+0-\n").1, "16");
    assert_eq!(run_with(src, "banana\n"), (Ok(Value::Word(Word::ONE)), "bad input".to_string()));
}

#[test]
fn traps() {
    let (r, _) = run_with("proc main() { sym b[]: int = claim 3; i: int = 2; b[i] = 1; burn b; }", "");
    assert!(r.unwrap_err().reason.contains("index 2 outside -1..+1"));
    let (r, _) = run_with("proc main() { n: int = 4; sym b[]: int = claim n; burn b; }", "");
    assert!(r.unwrap_err().reason.contains("odd"));
    let (r, _) = run_with("proc main() { t: trit = 0; if t { print(1); } }", "");
    assert_eq!(r.unwrap_err().reason, "if condition is 0 (unknown)");
    // A null condition is caught by the compiler.
    for body in ["t: trit? = null; tri t { - => {} 0 => {} + => {} }", "x: int? = null; if x > 0 { print(1); }"] {
        let e = ksc::compile(&format!("proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains("it may be null"), "{body}: {}", e.message);
    }
}

#[test]
fn if_and_comparisons() {
    let body = r#"walk i in -1..1 {
            if i < 0 { print("neg "); } else if i == 0 { print("zero "); } else { print("pos "); }
        }
        a: int = 3; b: int = 5; r: real = 2.5;
        printf("%s %s %s %s %s %s ", a == b, a != b, a < b, a > b, a <= 3, a >= 4);
        printf("%s %s ", r < 3.0, r == 2.5);
        t: trit = +; u: trit = 0;
        printf("%d%d%d%d", !t, !u, t && u, !(t || u));"#;
    assert_eq!(main_out(body), "neg zero pos false true true false true false true true -100-1");
}

#[test]
fn while_loops() {
    let body = r#"i: int = 0; while i < 5 { print(i); i = i + 1; }
        n: int = 0; while + { n = n + 1; if n == 3 { continue; } if n > 5 { break; } print(" "); print(n); }
        while i > 100 { print("never"); }"#;
    assert_eq!(main_out(body), "01234 1 2 4 5");
    let (r, _) = run_with("proc main() { t: trit = 0; while t { } }", "");
    assert_eq!(r.unwrap_err().reason, "loop condition is 0 (unknown)");
    // collatz(27) takes 111 steps
    let src = r#"proc steps(n: int) -> int { k: int = 0; while n != 1 { if n % 2 == 0 { n = n / 2; } else { n = 3 * n + 1; } k = k + 1; } return k; }
        proc main() { print(steps(27)); }"#;
    assert_eq!(out(src), "111");
}

#[test]
fn compound_assignment() {
    let body = r#"i: int = 0; while i < 3 { print(i); i++; }
        x: int = 10; x += 5; x -= 3; x *= 2; x /= 5; x %= 3; printf(" %d", x);
        n: int = 5; n--; printf(" %d", n);
        r: real = 1.5; r *= 2.0; r -= 0.5; printf(" %f", r);
        sym b[]: int = claim 3; b[1] += 7; b[1]++; b[-1] -= 1; printf(" {b}", ); burn b;"#;
    assert_eq!(main_out(body), "012 1 4 2.5 [-1, 0, 8]");
    let e = ksc::compile("proc main() { i: int = 0; final f: int = 1; f++; }").unwrap_err();
    assert!(e.message.contains("is final"), "{}", e.message);
    let e = ksc::compile("proc main() { i: int = 0; x: int = i++; }").unwrap_err();
    assert!(e.message.contains("expected `;`"), "{}", e.message);
}

#[test]
fn enums_and_match() {
    let src = r#"enum Cor { Vermelho, Verde, Azul }
        enum Http { Ok = 200, NotFound = 404, Teapot }
        proc acao(c: Cor) -> text {
            match c { Cor.Vermelho => return "pare"; Cor.Verde => return "siga"; Cor.Azul => return "?"; }
        }
        proc tipo(c: tryte) -> text {
            match c { 'a' | 'e' | 'i' | 'o' | 'u' => return "vogal"; '0' | '1' => { return "digito"; } else => return "outro"; }
        }
        proc main() {
            c: Cor = Cor.Verde;
            printf("{c} %d %s/", (int) c, acao(Cor.Vermelho));
            h: Http = Http.Teapot;
            printl(toText(h) + " " + toText((int) h));
            printf("%s %s %s %s/", tipo('e'), tipo('1'), tipo('z'), c != Cor.Azul);
            match 7 % 3 { 0 => print("div"); 1 | -1 => print("r1"); else => print("r2"); }
        }"#;
    assert_eq!(out(src), "Verde 1 pare/Teapot 405\nvogal digito outro true/r1");
    let e = ksc::compile("enum A { X, Y } proc main() { match A.X { A.X => print(1); } }").unwrap_err();
    assert!(e.message.contains("does not cover A.Y"), "{}", e.message);
}

#[test]
fn files_args_stdin_exit() {
    let dir = std::env::temp_dir().join(format!("kestrel-io-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.txt");
    std::fs::write(&input, "a | b\nc & d\n").unwrap();
    let src = r#"proc main() -> int {
        if !fileExists(arg(0)) { exit(3); }
        s: text = readFile(arg(0));
        n: int = 0;
        for i: int = 0; i < len(s); i++ { if s[i] == '|' || s[i] == '&' { n++; } }
        writeFile(arg(1), "n=" + toText(n));
        b[]: int = {75, 51, 66, 1};
        writeBytes(arg(1) + ".bin", b);
        burn b;
        printf("{n} %s %s %s", readLine(), readLine(), eof());
        return argCount();
    }"#;
    let m = build(src);
    let out_path = dir.join("out.txt");
    let args = vec![input.display().to_string(), out_path.display().to_string()];
    let mut printed = Vec::new();
    let r = Vm::new(&m, "um\ndois\n".as_bytes(), &mut printed).with_args(args).run();
    assert_eq!(r, Ok(Value::Word(Word::new(2))));
    assert_eq!(String::from_utf8(printed).unwrap(), "2 um dois true");
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "n=2");
    assert_eq!(std::fs::read(dir.join("out.txt.bin")).unwrap(), b"K3B\x01");
    // a missing file: exit(3) when checked, Trap when read directly
    let r = Vm::new(&m, "".as_bytes(), Vec::new()).with_args(vec!["/no/such/file".into(), "x".into()]).run();
    assert_eq!(r, Ok(Value::Word(Word::new(3))));
    let (r, _) = run_with(r#"proc main() { s: text = readFile("/no/such/file"); }"#, "");
    assert!(r.unwrap_err().reason.contains("cannot read file"));
    let (r, _) = run_with("proc main() { b[]: int = {300}; writeBytes(\"x\", b); burn b; }", "");
    assert!(r.unwrap_err().reason.contains("not a byte"));
    let (r, _) = run_with("proc main() { a: text = arg(0); }", "");
    assert!(r.unwrap_err().reason.contains("there are 0 arguments"));
    // exit ends a proc like return does
    assert_eq!(out("proc f() -> int { exit(0); } proc main() { print(f()); }"), "");
    let e = ksc::compile("proc main() { x: int = exit(1); }").unwrap_err();
    assert!(e.message.contains("is a statement"), "{}", e.message);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_operations() {
    let body = r#"nome: text = "Edynu"; printl("Olá, " + nome + "!");
        s: text = "Kestrel";
        printf("[%s][%s][%s] %s %s\n", sub(s, 0, 3), sub(s, 3, 7), sub(s, 2, 2), s == "Kestrel", s != "kestrel");
        a: int = toInt("123") on NaT { return; }; b: int = toInt("-7") on NaT { return; };
        c: int? = toInt("0t+0-"); r: real = toReal("2.5") on NaT { return; };
        printf("%d %s ", a + b, c);
        printf("%f %s\n", r * 2.0, toReal("-0.75"));
        printf("%s%s%s%s%s %s%s\n", isDigit('7'), isLetter('ç'), isUpper('Á'), isLower('a'), isSpace('\n'), isDigit('a'), isLetter(' '));
        acc: text = ""; for i: int = 0; i < 3; i++ { acc = acc + toText(i) + ","; } print(acc);"#;
    assert_eq!(main_out(body), "Olá, Edynu!\n[Kes][trel][] true true\n116 8 5.0 -0.75\ntruetruetruetruetrue falsefalse\n0,1,2,");
    // Not a number: null (`toInt` gives `int?`), and a plain `int` cannot take it.
    assert_eq!(main_out(r#"printl(toInt("12a"), toReal("abc"));"#), "null null\n");
    assert!(ksc::compile(r#"proc main() { x: int = toInt("12a"); }"#).unwrap_err().message.contains("it may be null"));
    let (r, _) = run_with(r#"proc main() { x: text = sub("abc", 2, 1); }"#, "");
    assert!(r.unwrap_err().reason.contains("needs 0 <= start <= end <= 3"));
    let e = ksc::compile(r#"proc main() { x: text = "a" + 1; }"#).unwrap_err();
    assert!(e.message.contains("joins text with text only"), "{}", e.message);
    // Texts have an order: alphabetical, character by character.
    assert_eq!(main_out(r#"printl("a" < "b", "b" <= "a", "ab" <=> "ab");"#), "true false 0\n");
}

#[test]
fn limits_and_messages() {
    // deep nesting is an error, not a compiler crash
    let deep = format!("proc main() {{ x: int = {}1{}; }}", "(".repeat(5000), ")".repeat(5000));
    assert!(ksc::compile(&deep).unwrap_err().message.contains("nested too deeply"));
    let long = format!("proc main() {{ x: int = 1{}; }}", " + 1".repeat(100_000));
    assert!(ksc::compile(&long).unwrap_err().message.contains("nested too deeply"));
    let ifs = format!("proc main() {{ {}{} }}", "if 1 > 0 { ".repeat(3000), "} ".repeat(3000));
    assert!(ksc::compile(&ifs).unwrap_err().message.contains("nested too deeply"));
    // but reasonable depth is fine
    let ok = format!("proc main() {{ x: int = {}1{}; print(x); }}", "(".repeat(200), ")".repeat(200));
    assert_eq!(out(&ok), "1");
    // too many variables in one proc
    let many: String = (0..66_000).map(|i| format!("v{i}: int = 1; ")).collect();
    let e = ksc::compile(&format!("proc main() {{ {many} }}")).unwrap_err();
    assert!(e.message.contains("at most 59049"), "{}", e.message);
    // text index message shows the last valid index
    let (r, _) = run_with(r#"proc main() { s: text = "abc"; i: int = 3; print(s[i]); }"#, "");
    assert_eq!(r.unwrap_err().reason, "text index 3 outside 0..2");
    // the same array cannot be borrowed twice by one call
    let e = ksc::compile("proc f(&a[]: int, &b[]: int) {} proc main() { x[]: int = claim 1; f(&x, &x); burn x; }").unwrap_err();
    assert!(e.message.contains("borrowed twice"), "{}", e.message);
}

#[test]
fn for_loops() {
    let body = r#"arr[]: int = {10, 20, 30, 40};
        for i: int = 0; i < len(arr); i++ { printf("%d ", arr[i]); }
        sym s[]: int = {-8, 0, 8};
        for i: int = -1; i <= 1; i++ { printf("%d ", s[i]); }
        for i: int = 10; i > 0; i -= 3 { printf("%d ", i); }
        for i: int = 0; i < 10; i++ { if i % 2 == 0 { continue; } if i > 7 { break; } printf("%d ", i); }
        burn arr; burn s;"#;
    assert_eq!(main_out(body), "10 20 30 40 -8 0 8 10 7 4 1 1 3 5 7 ");
    let e = ksc::compile("proc main() { for i: int = 0; i < 3; i++ { } print(i); }").unwrap_err();
    assert!(e.message.contains("unknown name `i`"), "{}", e.message);
    let e = ksc::compile("proc main() { for i: int = 0; i < 3; j: int = 1 { } }").unwrap_err();
    assert!(e.message.contains("must change a variable"), "{}", e.message);
    for (src, note) in [("j = 0", "cannot reuse"), ("i++", "e.g."), ("j", "`j` alone declares nothing")] {
        let e = ksc::compile(&format!("proc main() {{ j: int = 0; for {src}; j < 3; j++ {{ }} }}")).unwrap_err();
        assert!(e.message.contains("declaring its counter"), "{}", e.message);
        assert!(e.note.as_deref().unwrap_or("").contains(note), "{src}: {:?}", e.note);
    }
    // anything declared in the body is gone after the loop, too
    let e = ksc::compile("proc main() { for i: int = 0; i < 3; i++ { t: int = i; } print(t); }").unwrap_err();
    assert!(e.message.contains("unknown name `t`"), "{}", e.message);
}

#[test]
fn short_circuit() {
    // The right side is skipped when the left decides: no index trap on an empty text.
    let body = r#"s: text = ""; if len(s) > 0 && s[0] == 'a' { print("a"); } else { print("empty"); }
        if len(s) == 0 || s[0] == 'a' { print(" ok"); }
        f: trit = -; z: trit = 0; p: trit = +;
        printf(" %d %d %d", z && f, z || p, z && p);"#;
    assert_eq!(main_out(body), "empty ok -1 1 0");
}

#[test]
fn dump_shows_variables() {
    let o = main_out("low: int = -1; acc: int = 8; dump(\"here\");");
    assert!(o.contains("R-4  low"), "{o}");
    assert!(o.contains("R-3  acc        = 8"), "{o}");
}

#[test]
fn structs() {
    let src = r#"enum Cor { Vermelho, Azul }
        struct Ponto { x: int; y: int; }
        struct Pessoa { nome: text; casa: Ponto; cor: Cor; }
        proc mover(p: Ponto, dx: int) -> Ponto { p.x += dx; return p; }
        proc origem() -> Ponto { return Ponto { x: 0, y: 0 }; }
        proc main() {
            a: Ponto = Ponto { y: 2, x: 1 };
            b: Ponto = a; b.x = 10;
            printl(a); printl(b);
            c: Ponto = mover(a, 5);
            printf("{c} %d %d\n", a.x, origem().y);
            ana: Pessoa = Pessoa { nome: "Ana", casa: Ponto { x: 3, y: 4 }, cor: Cor.Azul };
            ana.casa.y++;
            printl(ana);
            printl(mover(ana.casa, -3).x + ana.casa.y);
            mover(a, 1);
        }"#;
    assert_eq!(
        out(src),
        "Ponto { x: 1, y: 2 }\nPonto { x: 10, y: 2 }\nPonto { x: 6, y: 2 } 1 0\n\
         Pessoa { nome: \"Ana\", casa: Ponto { x: 3, y: 5 }, cor: Azul }\n5\n"
    );
    let p = "struct P { x: int; y: int; } proc main() { ";
    for (body, msg) in [
        ("a: P = P { x: 1 }; }", "missing field `y` in `P`"),
        ("a: P = P { x: 1, y: 2, z: 3 }; }", "struct `P` has no field `z`"),
        ("a: P = P { x: 1, x: 2, y: 3 }; }", "field `x` is given twice"),
        ("final a: P = P { x: 1, y: 2 }; a.x = 3; }", "`a` is final"),
        ("x: int = 1; x.y = 2; }", "int has no fields"),
    ] {
        let e = ksc::compile(&format!("{p}{body}")).unwrap_err();
        assert!(e.message.contains(msg), "{body}: {}", e.message);
    }
    let e = ksc::compile("struct A { b: B; } struct B { a: A; } proc main() {}").unwrap_err();
    assert!(e.message.contains("struct `A` contains itself"), "{}", e.message);
    let e = ksc::compile("struct A { v[]: int; } proc main() {}").unwrap_err();
    assert!(e.message.contains("cannot hold an array"), "{}", e.message);
    let e = ksc::compile("struct P { x: int; } proc main() { p: P = P { x: 1 }; printf(\"%t\", p); }").unwrap_err();
    assert!(e.note.unwrap().contains("with `%s` or `{x}`, or format one field"));
    let e = ksc::compile("struct A { x: int, y: int } proc main() {}").unwrap_err();
    assert!(e.note.unwrap().contains("end with `;`"));
}

#[test]
fn old_snake_case_names_point_to_camel_case() {
    let e = ksc::compile("proc main() { t: text = to_text(5); }").unwrap_err();
    assert!(e.message.contains("unknown proc `to_text`"), "{}", e.message);
    assert_eq!(e.note.as_deref(), Some("built-ins are written in camelCase: `toText`"));
}

#[test]
fn arrays_of_any_type() {
    let src = r#"enum Cor { Vermelho = 1, Azul }
        struct Ponto { x: int; y: int; }
        struct Aluno { nome: text; nota: real; pos: Ponto; }
        proc soma(&ps[]: Ponto) -> Ponto {
            t: Ponto = Ponto { x: 0, y: 0 };
            for i: int = 0; i < len(ps); i++ { t.x += ps[i].x; t.y += ps[i].y; }
            return t;
        }
        proc criar(n: int) -> Ponto[] {
            v[]: Ponto = claim n;
            for i: int = 0; i < n; i++ { v[i] = Ponto { x: i, y: i * i }; }
            return v;
        }
        proc main() {
            nomes[]: text = {"Ana", "Bia"}; nomes[1] = nomes[1] + "triz"; printl(nomes);
            notas[]: real = claim 2; notas[0] = 9.5; printl(notas);
            cores[]: Cor = claim 2; cores[1] = Cor.Azul; printf("%s %s\n", cores[0], cores[1]);
            ps[]: Ponto = {Ponto { x: 1, y: 2 }, Ponto { x: 3, y: 4 }, Ponto { x: 5, y: 6 }};
            ps[0].y = 40; ps[1].x++;
            printl(ps[2]); printl(soma(&ps));
            sym s[]: Ponto = {Ponto { x: 7, y: 7 }, Ponto { x: 8, y: 8 }, Ponto { x: 9, y: 9 }};
            printl(s[-1]); burn s;
            v[]: Ponto = criar(4); printl(v[3]);
            turma[]: Aluno = claim 2;
            turma[1] = Aluno { nome: "Bia", nota: 8.5, pos: Ponto { x: 5, y: 6 } };
            turma[1].pos.y = 60;
            printl(turma[0]); printl(turma[1]); printl(len(turma));
            burn nomes; burn notas; burn cores; burn ps; burn v; burn turma;
        }"#;
    assert_eq!(
        out(src),
        "[\"Ana\", \"Biatriz\"]\n[9.5, 0.0]\n0 Azul\nPonto { x: 5, y: 6 }\nPonto { x: 10, y: 50 }\nPonto { x: 7, y: 7 }\nPonto { x: 3, y: 9 }\n\
         Aluno { nome: \"\", nota: 0.0, pos: Ponto { x: 0, y: 0 } }\n\
         Aluno { nome: \"Bia\", nota: 8.5, pos: Ponto { x: 5, y: 60 } }\n2\n"
    );
    let (r, _) = run_with("struct P { x: int; } proc main() { v[]: P = claim 2; i: int = 2; printl(v[i].x); burn v; }", "");
    assert!(r.unwrap_err().reason.contains("index 2 outside 0..1"));
    let e = ksc::compile("struct P { x: int; } proc main() { v[]: P = claim 2; printl(v); burn v; }").unwrap_err();
    assert!(e.message.contains("an array of P cannot be printed whole"), "{}", e.message);
    let e = ksc::compile("proc main() { v[]: int[] = claim 2; }").unwrap_err();
    assert!(e.message.contains("an array cannot hold arrays"), "{}", e.message);
}

#[test]
fn print_several_values() {
    let body = r#"n: int = 3; nomes[]: text = {"Ana Maria", "Bia"};
        printl("total:", n, nomes);
        print(1, 2.5); print("|"); printl(); printl("a" + "b", 'c' == 'c');
        burn nomes;"#;
    assert_eq!(main_out(body), "total: 3 [\"Ana Maria\", \"Bia\"]\n1 2.5|\nab true\n");
    let e = ksc::compile("proc main() { print(); }").unwrap_err();
    assert!(e.message.contains("at least 1 argument"), "{}", e.message);
}

#[test]
fn yield_became_return() {
    assert_eq!(out("proc dobro(x: int) -> int { return x * 2; } proc main() { print(dobro(21)); }"), "42");
    let e = ksc::compile("proc f() -> int { yield 1; } proc main() {}").unwrap_err();
    assert!(e.message.contains("`yield` is now `return`"), "{}", e.message);
}

#[test]
fn printf_braces_take_expressions() {
    let src = r#"struct Aluno { nome: text; n1: real; n2: real; }
        proc media(a: Aluno) -> real { return (a.n1 + a.n2) / 2.0; }
        proc main() {
            t[]: Aluno = {Aluno { nome: "Ana", n1: 6.0, n2: 4.5 }};
            a: int = 5;
            printf("{t[0].nome}: {media(t[0])} | {a + 1:t} {a * 2:v} {a > 1} {{a}}\n");
            burn t;
        }"#;
    assert_eq!(out(src), "Ana: 5.25 | 0t+-0 0vJ true {a}\n");
    let e = ksc::compile("proc main() { a: int = 1; printf(\"{a +}\"); }").unwrap_err();
    assert!(e.message.contains("expected an expression"), "{}", e.message);
    assert_eq!(e.span.start, 38);
}

#[test]
fn arrays_change_size() {
    let src = r#"struct Ponto { x: int; y: int; }
        proc encher(&v[]: int, n: int) { for i: int = 0; i < n; i++ { append(v, i * 10); } }
        proc main() {
            v[]: int = {};
            append(v, 4); append(v, 7); printl(v, len(v));
            x: int = pop(v); printl(x, v);
            encher(&v, 3); printl(v);
            insert(v, 0, 99); insert(v, len(v), 5); printl(v);
            remove(v, 1); printl(v);
            replace(v, 0..1, 3..4); printl(v);
            resize(v, 7); printl(v);
            resize(v, 5); clear(v); printl(v, len(v));
            nomes[]: text = {"Ana"}; append(nomes, "Bia"); resize(nomes, 3); resize(nomes, 2); printl(nomes);
            ps[]: Ponto = {};
            append(ps, Ponto { x: 1, y: 2 }); append(ps, Ponto { x: 3, y: 4 });
            p: Ponto = pop(ps); pop(ps); printl(p, len(ps));
            a[]: int = {1, 2}; append(a, 3); printl(a[2]);
            burn v; burn nomes; burn ps; burn a;
        }"#;
    assert_eq!(
        out(src),
        "[4, 7] 2\n7 [4]\n[4, 0, 10, 20]\n[99, 4, 0, 10, 20, 5]\n[99, 0, 10, 20, 5]\n[20, 5, 10, 99, 0]\n\
         [20, 5, 10, 99, 0, 0, 0]\n[] 0\n[\"Ana\", \"Bia\"]\nPonto { x: 3, y: 4 } 0\n3\n"
    );
    for (body, why) in [
        ("v[]: int = {1, 2, 3}; resize(v, 1); burn v;", "element 1 still holds a value; clear elements 1..2 first"),
        ("v[]: int = {}; x: int = pop(v); burn v;", "pop: the array is empty"),
        ("v[]: int = {1, 2, 3, 4}; replace(v, 0..2, 1..3); burn v;", "overlap"),
        ("v[]: int = {1}; insert(v, 5, 2); burn v;", "insert: index 5 outside 0..1"),
    ] {
        assert!(main_trap(body).contains(why), "{body}");
    }
    let e = ksc::compile("proc main() { sym s[]: int = {1, 2, 3}; append(s, 4); burn s; }").unwrap_err();
    assert!(e.message.contains("does not work on a sym array"), "{}", e.message);
}

#[test]
fn regressions_from_breaking_0_5() {
    // Diamond-shaped structs double in size per level: checked once per struct, with a cap.
    let mut src: String = (0..40).map(|i| format!("struct S{i} {{ a: S{n}; b: S{n}; }}\n", n = i + 1)).collect();
    src += "struct S40 { x: int; } proc main() { }";
    let e = ksc::compile(&src).unwrap_err();
    assert!(e.message.contains("is too big: it takes more than 59049 slots"), "{}", e.message);
    // A struct of more than 255 fields comes back whole from `return`.
    let fields: String = (0..300).map(|i| format!("f{i}: int; ")).collect();
    let vals: String = (0..300).map(|i| format!("f{i}: {i}, ")).collect();
    let src = format!("struct W {{ {fields} }} proc mk() -> W {{ return W {{ {vals} }}; }} proc main() {{ w: W = mk(); print(w.f0, w.f299); }}");
    assert_eq!(out(&src), "0 299");
    // An array given away inside the call that uses it.
    let defs = "proc g(v[]: int) -> int { burn v; return 1; } proc f(&v[]: int, x: int) { }";
    let e = ksc::compile(&format!("{defs} proc main() {{ v[]: int = {{1}}; append(v, g(give v)); }}")).unwrap_err();
    assert!(e.message.contains("given away inside a call that uses it"), "{}", e.message);
    let e = ksc::compile(&format!("{defs} proc main() {{ v[]: int = {{1}}; f(&v, g(give v)); }}")).unwrap_err();
    assert!(e.message.contains("borrowed and given away in the same call"), "{}", e.message);
}

#[test]
fn regressions_from_breaking_0_1_to_0_4() {
    let take = "proc take(v[]: int) -> int { burn v; return 1; }";
    for (body, why) in [
        ("while take(give v) > 0 { }", "the loop condition gives away `v`"),
        ("if 1 > 2 && take(give v) > 0 { }", "`give` on the right of `&&` may never run"),
        ("t: trit = 1 < 2 || take(give v) > 0;", "`give` on the right of `||` may never run"),
    ] {
        let e = ksc::compile(&format!("{take} proc main() {{ v[]: int = {{1}}; {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    // Texts made at run time are collected once nothing refers to them,
    // and texts kept in an array survive the collections.
    let body = r#"keep[]: text = {};
        walk i in 0..300000 { s: text = toText(i) + "abcdef"; if i % 1000 == 0 { append(keep, s); } }
        printl(len(keep), keep[0], keep[300]); clear(keep); burn keep;"#;
    assert_eq!(main_out(body), "301 0abcdef 300000abcdef\n");
    // Decimals with more digits than an exact conversion handles.
    let zeros = "0".repeat(49);
    let body = format!(r#"printl(toReal("0.{zeros}1"), toReal("99999999999999999999999999999999999999999999.5"), toReal("0.{zeros}{zeros}{zeros}{zeros}1"));"#);
    assert_eq!(main_out(&body), "1e-50 1e44 0.0\n");
}

#[test]
fn bools() {
    let src = r#"const DEBUG: bool = false;
        struct Item { nome: text; ativo: bool; }
        proc par(n: int) -> bool { return n % 2 == 0; }
        proc main() {
            ok: bool = 18 >= 18;
            printl(ok, !ok, ok && false, ok || DEBUG, ok == true, ok != false);
            printf("{ok} %s {par(3)}\n", par(4));
            t: trit = ok; u: trit = 0;
            printl(t, (int) ok, (bool) -1, 1 <=> 2, ok && u);
            flags[]: bool = claim 2; flags[1] = true;
            printl(flags[0], flags[1], toText(par(2)) + "!", Item { nome: "x", ativo: true });
            n: int = 0; while true { n++; if n > 2 { break; } }
            b: bool = (bool) (n - 3) on NaT { printl("not a bool"); burn flags; return; };
            burn flags;
        }"#;
    assert_eq!(
        out(src),
        "true false false true true true\ntrue true false\n1 1 false -1 0\nfalse true true! Item { nome: \"x\", ativo: true }\nnot a bool\n"
    );
    for (body, why) in [
        ("b: bool = 1;", "expected bool, found int"),
        ("x: int = true + 1;", "expected a number, found bool"),
        ("x: bool = true < false;", "`<` does not work on bools"),
        ("printf(\"%d\", true);", "`%d` prints an integer, found bool"),
    ] {
        let e = ksc::compile(&format!("proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    assert!(main_trap("x: int = 0; b: bool = (bool) x;").contains("(bool) 0: a bool is only 1 (true) or -1 (false)"));
}


#[test]
fn classes() {
    let src = r#"struct Ponto { x: int; y: int; }

class Conta {
    public nome: text;
    saldo: real;
    hist[]: real;
    pos: Ponto;

    public proc Conta(nome: text) -> Conta {
        return Conta { nome: nome, saldo: 0.0, hist: {}, pos: Ponto { x: 1, y: 2 } };
    }
    public proc depositar(&self, v: real) {
        self.saldo += v;
        append(self.hist, v);
        self.pos.x++;
    }
    public proc saldoAtual(&self) -> real { return self.saldo; }
    public proc movimentos(&self) -> int { return len(self.hist); }
    proc segredo(&self) -> int { return 42; }
    public proc fechar(self) -> real {
        s: real = self.saldo + (real) self.segredo();
        burn self;
        return s;
    }
}

class Banco {
    principal: Conta;
    public proc Banco() -> Banco {
        c: Conta = Conta("Caixa");
        return Banco { principal: give c };
    }
    public proc depositar(&self, v: real) { self.principal.depositar(v); }
    public proc total(&self) -> real { return self.principal.saldoAtual(); }
}

proc mostrar(c: &Conta) {
    printl(c.nome, c.saldoAtual());
}

proc main() {
    c: Conta = Conta("Ana");
    c.depositar(10.0);
    c.depositar(5.5);
    mostrar(&c);
    printl(c);
    printl(c.movimentos());
    c.nome = "Ana Maria";
    total: real = c.fechar();
    printl(total);
    b: Banco = Banco();
    b.depositar(3.0);
    printl(b.total(), b);
    burn b;
}
"#;
    assert_eq!(
        out(src),
        "Ana 15.5\nConta { nome: \"Ana\", saldo: 15.5, hist: [10.0, 5.5], pos: Ponto { x: 3, y: 2 } }\n2\n57.5\n\
         3.0 Banco { principal: Conta { nome: \"Caixa\", saldo: 3.0, hist: [3.0], pos: Ponto { x: 2, y: 2 } } }\n"
    );
    let c = "class C { public n: int; v[]: int; proc mk() -> C { return C { n: 1, v: {} }; } \
             public proc C() -> C { return C.mk(); } public proc ler(&self) -> int { return self.n; } \
             public proc fim(self) { burn self; } public proc f(&self, &w[]: int) { } }";
    for (body, why) in [
        ("c: C = C();", "`c` is never burned or given away"),
        ("c: C = C(); x: int = c.v; burn c;", "`v` is private in `C`"),
        ("c: C = C(); d: C = c; burn c;", "object `c` cannot be copied"),
        ("c: C = C { n: 1, v: {} }; burn c;", "`C` objects are created inside the class"),
        ("c: C = C(); c.fim(); x: int = c.ler();", "`c` is used after it was given away"),
        ("c: C = C(); x: int = C.ler(); burn c;", "`ler` needs an object"),
        ("c: C = C(); c.C(); burn c;", "`C` is static"),
        ("x: int = C().n;", "the object made here would be lost"),
        ("c: C = C.mk(); burn c;", "method `mk` is private in `C`"),
        ("c: C = C(); c.f(&c.n); burn c;", "only arrays and objects are borrowed"),
    ] {
        let e = ksc::compile(&format!("{c} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    let e = ksc::compile("class A { b: B; } class B { a: A; } proc main() {}").unwrap_err();
    assert!(e.message.contains("class `A` contains itself"), "{}", e.message);
    let e = ksc::compile(
        "class C { public v[]: int; public proc C() -> C { return C { v: {} }; } public proc f(&self, &w[]: int) { } } \
         proc main() { c: C = C(); c.f(&c.v); burn c; }",
    )
    .unwrap_err();
    assert!(e.message.contains("`c` is borrowed twice"), "{}", e.message);
}

#[test]
fn every_variable_has_its_type_written() {
    let e = ksc::compile("proc main() { x := 5; }").unwrap_err();
    assert!(e.message.contains("expected a type, found `=`"), "{}", e.message);
    assert_eq!(e.note.as_deref(), Some("every variable has its type written: `x: int = 5;`"));
    // Declared without a value: only a `T?`, and it starts as null.
    let src = r#"class C { public n: int; public proc C() -> C { return C { n: 7 }; } }
        struct P { x: int?; r: real?; t: text; }
        proc main() {
            total: int?; nome: text?; ok: bool?;
            p: P = P { x: null, r: 2.5, t: "a" };
            printl(total, nome, ok, p);
            total = 10;
            printl(total + 1);
            c: C?;
            if total > 5 { c = C(); printl(c.n); }
            burn c;
            y: int?;
            z: int = y on NaT { printl("y has no value"); return; };
        }"#;
    assert_eq!(out(src), "null null null P { x: null, r: 2.5, t: \"a\" }\n11\n7\ny has no value\n");
    let c = "class C { public n: int; public proc C() -> C { return C { n: 7 }; } }";
    for (body, why) in [
        ("x: int;", "`x` needs a value"),
        ("c: C;", "`c` needs a value"),
        ("v[]: int;", "`v` needs a value"),
        ("c: C?; printl(c.n); burn c;", "it may be null"),
        ("final k: int?;", "final `k` needs a value"),
        ("x: int = null;", "int is never null"),
    ] {
        let e = ksc::compile(&format!("{c} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
}

#[test]
fn null_safety() {
    let src = r#"class No {
    public valor: int;
    public proximo: No?;
    public proc No(v: int) -> No { return No { valor: v, proximo: null }; }
    public proc dobro(&self) -> int { return 2 * self.valor; }
    public proc avisar(&self) { printl("aviso", self.valor); }
}

proc metade(x: int) -> int? {
    if x % 2 != 0 { return null; }
    return x / 2;
}

proc mostrar(n: &No?) {
    printl(n?.dobro(), n?.proximo?.valor);
    n?.avisar();
}

proc main() {
    a: int? = toInt("42");
    if a != null { printl(a + 1); }
    printl(toInt("x"), a);
    if metade(7) == null { printl("odd"); }
    k: int? = metade(8);
    if k == null { return; }
    printl(k * 10);
    assert(a != null);
    printl(a - 2);
    s: text? = null;
    if s == null || len(s) == 0 { printl("empty"); }
    s = "Ana";
    printl(s + "!");
    s = null;
    printl(s);
    lista: No = No(1);
    lista.proximo = No(2);
    printl(lista.proximo.valor);
    mostrar(&lista.proximo.proximo);
    mostrar(&lista);
    burn lista;
}
"#;
    assert_eq!(out(src), "43\nnull 42\nodd\n40\n40\nempty\nAna!\nnull\n2\nnull null\n2 2\naviso 1\n");
    for (body, why) in [
        ("x: int? = 1; y: int = x + 1;", "it may be null"),
        ("x: int? = 1; if x != null { x = null; printl(x + 1); }", "it may be null"),
        ("x: int? = 1; if x == null { return; } if true { x = null; } printl(x + 1);", "it may be null"),
        ("x: int? = 1; while true { printl(x + 1); x = null; }", "it may be null"),
        ("x: int = 1; if x == null { }", "int, so it is never null"),
        ("b: bool = isNaT(1);", "is written `x == null`"),
        ("x: int = 1; y: int? = x?.foo();", "`?.` is for values that may be null"),
    ] {
        let e = ksc::compile(&format!("proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    let e = ksc::compile("proc main() { x: int? = NaT; }").unwrap_err();
    assert!(e.message.contains("`NaT` is written `null`"), "{}", e.message);
}

#[test]
fn interfaces() {
    let src = r#"interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
    proc escalar(&self, k: real);
}

@impl(Forma)
class Circulo {
    raio: real;
    public proc Circulo(r: real) -> Circulo { return Circulo { raio: r }; }
    @override
    public proc area(&self) -> real { return 3.0 * self.raio * self.raio; }
    @override
    public proc nome(&self) -> text { return "circulo"; }
    @override
    public proc escalar(&self, k: real) { self.raio *= k; }
    public proc diametro(&self) -> real { return 2.0 * self.raio; }
}

@impl(Forma)
class Quadrado {
    lado: real;
    public proc Quadrado(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override
    public proc area(&self) -> real { return self.lado * self.lado; }
    @override
    public proc nome(&self) -> text { return "quadrado"; }
    @override
    public proc escalar(&self, k: real) { self.lado *= k; }
}

proc mostrar(f: &Forma) {
    printf("{f.nome()}: {f.area()}\n");
}

proc dobrarEMostrar(f: &Forma) {
    f.escalar(2.0);
    mostrar(&f);
}

proc main() {
    c: Circulo = Circulo(1.0);
    q: Quadrado = Quadrado(3.0);
    mostrar(&c);
    mostrar(&q);
    dobrarEMostrar(&c);
    dobrarEMostrar(&q);
    printl(c.diametro());
    burn c;
    burn q;
}
"#;
    assert_eq!(out(src), "circulo: 3.0\nquadrado: 9.0\ncirculo: 12.0\nquadrado: 36.0\n4.0\n");
    let i = "interface F { proc area(&self) -> real; }";
    for (decls, why) in [
        ("@impl(F) class A { x: real; }", "`A` implements `F` but has no method `area`"),
        ("@impl(F) class A { x: real; @override public proc area(&self) -> int { return 1; } }", "`A.area` does not match `F.area`"),
        ("@impl(F) class A { x: real; public proc area(&self) -> real { return 1.0; } }", "mark it `@override`"),
        (
            "@impl(F) class A { x: real; @override public proc area(&self) -> real { return 1.0; } \
             @override public proc aera(&self) -> real { return 1.0; } }",
            "`aera` overrides nothing",
        ),
        ("@impl(F) class A { x: real; @override proc area(&self) -> real { return 1.0; } }", "must be public"),
        ("proc f(g: &F) { g.voar(); }", "interface `F` has no method `voar`"),
        ("@impl(G) class A { x: real; }", "`G` is not an interface"),
        ("proc f(g: &F) { burn g; }", "cannot burn a borrowed object"),
        ("interface G { proc criar() -> int; }", "interface method `criar` needs `self`"),
    ] {
        let e = ksc::compile(&format!("{i} {decls} proc main() {{ }}")).unwrap_err();
        assert!(e.message.contains(why), "{decls}: {}", e.message);
    }
    let e = ksc::compile(&format!(
        "{i} class A {{ x: real; public proc A() -> A {{ return A {{ x: 1.0 }}; }} }} proc f(g: &F) {{ }} \
         proc main() {{ a: A = A(); f(&a); burn a; }}"
    ))
    .unwrap_err();
    assert!(e.message.contains("`A` does not implement `F`"), "{}", e.message);
}

#[test]
fn arrays_of_objects() {
    let src = r#"interface Nomeado {
    proc nome(&self) -> text;
}

@impl(Nomeado)
class Conta {
    dono: text;
    saldo: real;
    public proc Conta(dono: text) -> Conta { return Conta { dono: dono, saldo: 0.0 }; }
    @override
    public proc nome(&self) -> text { return self.dono; }
    public proc depositar(&self, v: real) { self.saldo += v; }
    public proc saldoAtual(&self) -> real { return self.saldo; }
}

class Banco {
    contas[]: Conta;
    public proc Banco() -> Banco {
        vazio[]: Conta = {};
        return Banco { contas: give vazio };
    }
    public proc abrir(&self, dono: text) { append(self.contas, Conta(dono)); }
    public proc total(&self) -> real {
        t: real = 0.0;
        for i: int = 0; i < len(self.contas); i++ { t += self.contas[i].saldoAtual(); }
        return t;
    }
}

proc mostrar(n: &Nomeado) { printl(n.nome()); }

proc main() {
    a: Conta = Conta("Ana");
    cs[]: Conta = {give a, Conta("Bia")};
    insert(cs, 1, Conta("Caio"));
    cs[0].depositar(2.5);
    mostrar(&cs[1]);
    printl(cs[2].nome(), len(cs));
    printl(cs[0]);
    x: Conta = remove(cs, 1);
    printl(x.nome(), len(cs));
    burn x;
    b: Banco = Banco();
    b.abrir("Davi");
    b.abrir("Eva");
    printl(b.total());
    dump("antes");
    burn cs;
    burn b;
    dump("depois");
    v[]: int = {7, 8, 9};
    n: int = remove(v, 0);
    remove(v, 0);
    printl(n, v);
    burn v;
}
"#;
    let o = out(src);
    assert!(o.starts_with("Caio\nBia 3\nConta { dono: \"Ana\", saldo: 2.5 }\nCaio 2\n0.0\n"), "{o}");
    assert!(o.ends_with("7 [9]\n"), "{o}");
    // `burn` frees the objects inside the arrays too.
    let after = &o[o.find("dump: depois").unwrap()..];
    assert!(o.contains("heap #") && !after.contains("heap #"), "{o}");

    let cls = "class C { x: int; public proc C() -> C { return C { x: 1 }; } \
               public proc pega(&self, o: C) { burn o; } }
               proc par(c: &C, d: C) { burn d; }";
    for (body, why) in [
        ("x: C = cs[0];", "take it out with `remove(v, i)` or `pop(v)`"),
        ("cs[0] = C();", "an object in an array cannot be replaced"),
        ("ds[]: C = claim 3; burn ds;", "`claim` would make empty `C` elements"),
        ("resize(cs, 3);", "`resize` does not work on an array of `C` objects"),
        ("clear(cs);", "`clear` does not work"),
        ("pop(cs);", "would be lost"),
        ("remove(cs, 0);", "would be lost"),
        ("c: C = C(); append(cs, c); burn c;", "object `c` cannot be copied"),
        ("par(&cs[0], remove(cs, 0));", "has an element taken out in the same call"),
        ("cs[0].pega(pop(cs));", "has an element taken out in the same call"),
    ] {
        let e = ksc::compile(&format!("{cls} proc main() {{ cs[]: C = {{C()}}; {body} burn cs; }}")).unwrap_err();
        assert!(e.message.contains(why) || e.note.as_ref().is_some_and(|n| n.contains(why)), "{body}: {} {:?}", e.message, e.note);
    }
}

#[test]
fn owned_interfaces() {
    let src = r#"interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;
    public proc Circulo(raio: real) -> Circulo { return Circulo { raio: raio }; }
    @override
    public proc area(&self) -> real { return 3.0 * self.raio * self.raio; }
    @override
    public proc nome(&self) -> text { return "circulo"; }
}

@impl(Forma)
class Poligono {
    lados[]: real;
    public proc Poligono(a: real, b: real) -> Poligono {
        ls[]: real = {a, b};
        return Poligono { lados: give ls };
    }
    @override
    public proc area(&self) -> real { return self.lados[0] * self.lados[1]; }
    @override
    public proc nome(&self) -> text { return "poligono"; }
}

class Desenho {
    principal: Forma;
    public proc Desenho(f: Forma) -> Desenho { return Desenho { principal: give f }; }
    public proc mostrar(&self) { mostrar(&self.principal); }
}

proc mostrar(f: &Forma) { printf("{f.nome()}: {f.area()}\n"); }

proc criar(quadrado: bool) -> Forma {
    if quadrado { return Poligono(2.0, 2.0); }
    return Circulo(1.0);
}

proc main() {
    c: Circulo = Circulo(2.0);
    f: Forma = give c;
    printl(f.area());
    formas[]: Forma = {give f, Poligono(3.0, 4.0)};
    append(formas, criar(false));
    for i: int = 0; i < len(formas); i++ {
        mostrar(&formas[i]);
    }
    g: Forma = remove(formas, 1);
    d: Desenho = Desenho(give g);
    d.mostrar();
    vazia: Forma?;
    vazia = criar(true);
    printl(vazia.nome(), formas[1].area());
    dump("antes");
    burn vazia;
    burn d;
    burn formas;
    dump("depois");
}
"#;
    let o = out(src);
    assert!(
        o.starts_with("12.0\ncirculo: 12.0\npoligono: 12.0\ncirculo: 3.0\npoligono: 12.0\npoligono 3.0\n"),
        "{o}"
    );
    // `burn` through an interface frees what that object's class owns.
    let after = &o[o.find("dump: depois").unwrap()..];
    assert!(o.contains("heap #") && !after.contains("heap #"), "{o}");

    let decls = "interface F { proc n(&self) -> int; }
        @impl(F) class A { x: int; public proc A() -> A { return A { x: 1 }; }
                           @override public proc n(&self) -> int { return self.x; } }
        class B { y: int; public proc B() -> B { return B { y: 1 }; } }";
    for (body, why) in [
        ("a: A = A(); f: F = a; burn f;", "object `a` cannot be copied"),
        ("f: F = B(); burn f;", "`B` does not implement `F`"),
        ("fs[]: F = {A()}; x: F = fs[0]; burn x; burn fs;", "take it out with `remove(v, i)`"),
        ("f: F = A(); printl(f.x); burn f;", "`x` is hidden"),
        ("f: F = A();", "`f` is never burned"),
        ("printl(A().n());", "would be lost"),
        ("fs[]: F = claim 2; burn fs;", "`claim` would make empty `F` elements"),
        ("g: F; printl(g.n());", "`g` needs a value"),
        ("g: F?; printl(g.n()); burn g;", "it may be null"),
    ] {
        let e = ksc::compile(&format!("{decls} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why) || e.note.as_ref().is_some_and(|n| n.contains(why)), "{body}: {} {:?}", e.message, e.note);
    }
}

#[test]
fn assert_tests_and_deprecated() {
    // A failed assert traps with its line and the condition (or the message).
    assert!(main_trap("x: int = 2;\n    assert(x == 3);").contains("assert failed (line 2): x == 3"));
    assert!(main_trap("assert(1 > 2, \"nunca\");").contains("assert failed (line 1): nunca"));
    assert_eq!(main_out("assert(1 < 2); printl(\"ok\");"), "ok\n");

    let src = r#"@deprecated("use somar")
proc soma(a: int, b: int) -> int { return a + b; }
proc somar(a: int, b: int) -> int { return a + b; }

@deprecated
class Velha {
    x: int;
    public proc Velha() -> Velha { return Velha { x: 1 }; }
    @deprecated("use ver")
    public proc olhar(&self) -> int { return self.ver(); }
    public proc ver(&self) -> int { return self.x; }
}

@test
proc passa() { assert(somar(2, 3) == 5); }

@test
proc falha() { assert(somar(2, 2) == 5, "conta errada"); }

proc main() {
    printl(soma(1, 2));
    v: Velha = Velha();
    printl(v.olhar());
    burn v;
}
"#;
    let o = ksc::compile_full(src, true).unwrap();
    let w: Vec<(String, Option<String>)> = o.warnings.iter().map(|w| (w.message.clone(), w.note.clone())).collect();
    assert_eq!(
        w,
        [
            ("`soma` is deprecated".to_string(), Some("use somar".to_string())),
            ("class `Velha` is deprecated".to_string(), None),
            ("`Velha.olhar` is deprecated".to_string(), Some("use ver".to_string())),
        ]
    );
    assert!(o.warnings.iter().all(|w| w.warning));
    let names: Vec<&str> = o.tests.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["passa", "falha"]);
    // Each test runs as the entry of its own VM.
    let run = |id: u32| {
        let mut m = o.module.clone();
        m.entry = id;
        Vm::new(&m, &b""[..], Vec::new()).run()
    };
    assert!(run(o.tests[0].1).is_ok());
    assert!(run(o.tests[1].1).unwrap_err().reason.contains("assert failed (line 18): conta errada"));

    // Without `main`, only `ksc --test` accepts the file.
    let only_tests = "@test proc t() { assert(true); }";
    assert!(ksc::compile(only_tests).unwrap_err().message.contains("no `proc main`"));
    assert_eq!(ksc::compile_full(only_tests, false).unwrap().tests.len(), 1);

    for (src, why) in [
        ("@test proc t(x: int) { } proc main() { }", "cannot take parameters"),
        ("class A { x: int; @test public proc t(&self) { } } proc main() { }", "unknown annotation `@test` on a method"),
        ("@foo proc f() { } proc main() { }", "unknown annotation `@foo` on a proc"),
        ("proc main() { x: bool = assert(true); }", "is a statement, not a value"),
        ("proc main() { assert(1.5); }", "`assert` needs a bool"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn recursive_and_optional_objects() {
    // A tree: a class holding an array of itself. `burn` frees it by recursion.
    let tree = r#"class No {
    public valor: int;
    filhos[]: No;
    public proc No(v: int) -> No { f[]: No = {}; return No { valor: v, filhos: give f }; }
    public proc add(&self, n: No) { append(self.filhos, give n); }
    public proc soma(&self) -> int {
        s: int = self.valor;
        for i: int = 0; i < len(self.filhos); i++ { s += self.filhos[i].soma(); }
        return s;
    }
}
proc main() {
    r: No = No(1);
    a: No = No(2);
    a.add(No(4));
    r.add(give a);
    r.add(No(3));
    printl(r.soma());
    burn r;
    dump("fim");
}
"#;
    let o = out(tree);
    assert!(o.starts_with("10\n") && !o.contains("heap #"), "{o}");

    // A linked list: `proximo: No?` may be empty and is checked before use.
    let list = r#"interface F { proc nome(&self) -> text; }
@impl(F) class A { x: int; public proc A() -> A { return A { x: 1 }; }
    @override public proc nome(&self) -> text { return "a"; } }

class No {
    public valor: int;
    public proximo: No?;
    public proc No(v: int) -> No { return No { valor: v, proximo: null }; }
    public proc anexar(&self, n: No) {
        if self.proximo == null {
            self.proximo = give n;
            return;
        }
        self.proximo.anexar(give n);
    }
}

proc soma(n: &No?) -> int {
    if n == null { return 0; }
    return n.valor + soma(&n.proximo);
}

proc main() {
    lista: No = No(1);
    lista.anexar(No(2));
    lista.anexar(No(3));
    printl(soma(&lista));
    if lista.proximo != null && lista.proximo.valor == 2 { printl("segundo"); }
    resto: No? = give lista.proximo;
    printl(soma(&lista), soma(&resto), lista.proximo == null);
    v: No?;
    printl(v == null);
    v = No(9);
    if v.valor == 9 { printl("nove"); }
    f: F? = A();
    g: F?;
    if f != null { printl(f.nome(), g == null); }
    burn f;
    burn g;
    burn v;
    burn resto;
    burn lista;
    dump("fim");
}
"#;
    let o = out(list);
    assert!(o.starts_with("6\nsegundo\n1 5 true\ntrue\nnove\na true\n") && !o.contains("heap #"), "{o}");

    // Putting an object into a `T?` that still holds one traps (it would be lost).
    let decls = "class No { public valor: int; public proximo: No?;
        public proc No(v: int) -> No { return No { valor: v, proximo: null }; }
        public proc mexer(&self) { } }
        proc ver(n: &No) { }";
    let run = |body: &str| run_with(&format!("{decls} proc main() {{ n: No = No(1); {body} burn n; }}"), "").0;
    assert!(run("n.proximo = No(2); n.proximo = No(3);").unwrap_err().reason.contains("`n.proximo` still holds an object"));
    assert!(run("v: No? = No(5); v = No(6); burn v;").unwrap_err().reason.contains("`v` still holds an object"));
    assert!(run("v: No?; burn v; v = No(6); burn v;").is_ok());

    for (body, why) in [
        ("printl(n.proximo.valor);", "it may be null"),
        ("if n.proximo != null { n.mexer(); printl(n.proximo.valor); }", "it may be null"),
        ("if n.proximo != null { while true { printl(n.proximo.valor); n.mexer(); } }", "it may be null"),
        ("if n.proximo != null { x: No = give n.proximo; printl(n.proximo.valor); burn x; }", "it may be null"),
        ("v: No?; x: No = give v; burn x;", "it may be null"),
        ("v: No?; ver(&v); burn v;", "it may be null"),
        ("x: int = give n.valor;", "only a field that may be empty can be given away"),
        ("v: No?;", "`v` is never burned"),
        ("x: Ponto? = null;", "unknown"),
    ] {
        let e = ksc::compile(&format!("{decls} proc main() {{ n: No = No(1); {body} burn n; }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    // Checked, then nothing that could empty it: fine.
    assert!(ksc::compile(&format!(
        "{decls} proc main() {{ n: No = No(1); if n.proximo != null {{ n.proximo.mexer(); ver(&n.proximo); }} burn n; }}"
    ))
    .is_ok());
}

#[test]
fn is_and_match_by_class() {
    let decls = r#"interface Forma { proc nome(&self) -> text; }
@impl(Forma) class Circulo {
    public raio: real;
    public proc Circulo(r: real) -> Circulo { return Circulo { raio: r }; }
    @override public proc nome(&self) -> text { return "circulo"; }
}
@impl(Forma) class Quadrado {
    public lado: real;
    public proc Quadrado(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override public proc nome(&self) -> text { return "quadrado"; }
}
@impl(Forma) class Triangulo {
    b: real;
    public proc Triangulo(b: real) -> Triangulo { return Triangulo { b: b }; }
    @override public proc nome(&self) -> text { return "triangulo"; }
}
class Desenho {
    public principal: Forma;
    public proc Desenho(f: Forma) -> Desenho { return Desenho { principal: give f }; }
}
class Conta { x: int; }
"#;
    let src = format!(
        r#"{decls}
proc descrever(f: &Forma) {{
    if f is Circulo {{
        printl("raio", f.raio);
    }} else if f is Quadrado && f.lado > 1.0 {{
        printl("lado", f.lado);
    }} else {{
        printl(f.nome());
    }}
}}

proc tipo(f: &Forma) -> text {{
    match f {{
        Circulo => return "C";
        Quadrado | Triangulo => return f.nome();
    }}
}}

proc main() {{
    fs[]: Forma = {{Circulo(1.0), Quadrado(2.0), Triangulo(3.0)}};
    n: int = 0;
    for i: int = 0; i < len(fs); i++ {{
        descrever(&fs[i]);
        printl(tipo(&fs[i]));
        if fs[i] is Circulo {{ n++; }}
    }}
    printl(n);
    f: Forma = remove(fs, 0);
    if !(f is Circulo) {{
        burn f;
        burn fs;
        return;
    }}
    c: Circulo = give f;
    printl(c.raio);
    d: Desenho = Desenho(Quadrado(5.0));
    if d.principal is Quadrado {{ printl(d.principal.lado); }}
    o: Forma?;
    printl(o is Circulo);
    burn o;
    burn d;
    burn c;
    burn fs;
}}
"#
    );
    assert_eq!(
        out(&src),
        "raio 1.0\nC\nlado 2.0\nquadrado\ntriangulo\ntriangulo\n1\n1.0\n5.0\nfalse\n"
    );

    for (body, why) in [
        ("b: bool = f is Conta;", "`Conta` does not implement `Forma`"),
        ("x: int = 1; b: bool = x is Circulo;", "`is` tests an interface value"),
        ("match f { Circulo => printl(1); Quadrado => printl(2); }", "`match` does not cover `Triangulo`"),
        ("match fs[0] { Circulo => printl(1); else => printl(2); }", "works on a variable or a field"),
        ("printl(f.raio);", "`raio` is hidden"),
        ("match f { Circulo | Quadrado => printl(f.raio); else => printl(0); }", "`raio` is hidden"),
        ("match f { Circulo => printl(1); Circulo => printl(2); else => printl(3); }", "already appears"),
        ("match f { 5 => printl(1); else => printl(3); }", "a pattern here is a class name"),
    ] {
        let e = ksc::compile(&format!("{decls} proc t(f: &Forma, &fs[]: Forma) {{ {body} }} proc main() {{ }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
}

#[test]
fn lent_variables() {
    let decls = r#"class No {
    public valor: int;
    public proximo: No?;
    public proc No(v: int) -> No { return No { valor: v, proximo: null }; }
    public proc dobrar(&self) { self.valor *= 2; }
}
proc mexe(n: &No) { }
proc pega(n: No) { burn n; }
"#;
    let src = format!(
        r#"{decls}
proc tamanho(lista: &No) -> int {{
    n: int = 1;
    atual: &No? = &lista.proximo;
    while atual != null {{
        n++;
        atual = &atual.proximo;
    }}
    return n;
}}

proc anexar(lista: &No, novo: No) {{
    ultimo: &No = &lista;
    while ultimo.proximo != null {{
        ultimo = &ultimo.proximo;
    }}
    ultimo.proximo = give novo;
}}

proc main() {{
    // Built from the front: each new node takes the list as its `proximo`.
    lista: No? = No(1);
    for i: int = 2; i <= 7000; i++ {{
        n: No = No(i);
        n.proximo = give lista;
        lista = give n;
    }}
    if lista == null {{ burn lista; return; }}
    anexar(&lista, No(0));
    printl(tamanho(&lista), lista.valor);
    vs[]: No = {{No(5), No(6)}};
    c: &No = &vs[1];
    c.dobrar();
    printl(c.valor);
    burn vs;
    burn lista;
}}
"#
    );
    // Longer than the call stack allows: walked by a loop, and burned by one.
    assert_eq!(out(&src), "7001 7000\n12\n");

    for (body, why) in [
        ("x: &No = &vs[0]; burn vs; printl(x.valor);", "`vs` cannot be burned while `x` borrows from it"),
        ("x: &No = &vs[0]; y: No = pop(vs); printl(x.valor); burn y;", "cannot be emptied of an element"),
        ("x: &No = &vs[1]; mexe(&vs[0]); printl(x.valor);", "cannot be borrowed by a call"),
        ("x: &No? = &a.proximo; p: No? = give a.proximo; printl(x?.valor); burn p;", "`a` cannot be emptied"),
        ("x: &No = &a; y: &No? = &a.proximo; mexe(&x); printl(y?.valor);", "while `y` borrows"),
        ("x: &No = &a; burn x;", "cannot burn a borrowed object"),
        ("x: &No = a;", "give it `&...`"),
        ("x: &int = 5;", "only arrays and objects are borrowed with `&`"),
    ] {
        let e = ksc::compile(&format!(
            "{decls} proc main() {{ a: No = No(1); a.proximo = No(2); vs[]: No = {{No(3), No(4)}}; {body} burn vs; burn a; }}"
        ))
        .unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    // After the last use of the `lent` variable, the owner is free again.
    assert!(ksc::compile(&format!(
        "{decls} proc main() {{ vs[]: No = {{No(3)}}; x: &No = &vs[0]; x.dobrar(); mexe(&x); burn vs; }}"
    ))
    .is_ok());
}

#[test]
fn default_methods() {
    let src = r#"interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
    proc descrever(&self) -> text {
        return self.nome() + " de área " + toText(self.area());
    }
    proc dobro(&self, k: real) -> real {
        return self.area() * k;
    }
}

@impl(Forma)
class Circulo {
    raio: real;
    public proc Circulo(r: real) -> Circulo { return Circulo { raio: r }; }
    @override public proc area(&self) -> real { return 3.0 * self.raio * self.raio; }
    @override public proc nome(&self) -> text { return "círculo"; }
    public proc resumo(&self) -> text { return "[" + self.descrever() + "]"; }
}

@impl(Forma)
class Quadrado {
    lado: real;
    public proc Quadrado(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override public proc area(&self) -> real { return self.lado * self.lado; }
    @override public proc nome(&self) -> text { return "quadrado"; }
    @override public proc descrever(&self) -> text { return "um quadrado de lado " + toText(self.lado); }
}

proc main() {
    c: Circulo = Circulo(1.0);
    printl(c.descrever());
    printl(c.resumo());
    printl(c.dobro(2.0));
    fs[]: Forma = {Circulo(2.0), Quadrado(3.0)};
    for i: int = 0; i < len(fs); i++ {
        printl(fs[i].descrever(), fs[i].dobro(10.0));
    }
    burn fs;
    burn c;
}
"#;
    assert_eq!(out(src), "círculo de área 3.0\n[círculo de área 3.0]\n6.0\ncírculo de área 12.0 120.0\num quadrado de lado 3.0 90.0\n");
    for (src, why) in [
        ("interface F { proc a(&self) -> int; proc b(&self) -> int { return self.x; } } proc main() { }", "`x` is hidden"),
        ("interface F { proc b(self) { burn self; } } proc main() { }", "must take `&self`"),
        ("interface F { proc b(&self) -> int { return 1; } } @impl(F) class A { x: int; public proc b(&self) -> int { return 2; } } proc main() { }", "mark it `@override`"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn generics() {
    let src = r#"class Box<T> {
    public value: T?;
    public proc Box(v: T) -> Box<T> { return Box<T> { value: give v }; }
    public proc take(&self) -> T? { return give self.value; }
    public proc put(&self, v: T) { self.value = give v; }
}

class Pair<A, B> {
    public first: A;
    public second: B;
    public proc of(a: A, b: B) -> Pair<A, B> { return Pair<A, B> { first: give a, second: give b }; }
}

proc pick<T>(a: T, b: T, primeiro: bool) -> T {
    if primeiro {
        burn b;
        return a;
    }
    burn a;
    return b;
}

class Conta {
    public saldo: real;
    public proc Conta(s: real) -> Conta { return Conta { saldo: s }; }
}

proc main() {
    b: Box<int> = Box<int>(5);
    printl(b.value);
    old: int? = b.take();
    b.put(7);
    printl(old, b.value);
    burn b;
    p: Pair<text, real> = Pair<text, real>.of("pi", 3.14);
    printl(p.first, p.second);
    burn p;
    printl(pick<int>(4, 5, false));
    c: Conta = pick<Conta>(Conta(1.0), Conta(2.0), true);
    printl(c.saldo);
    bc: Box<Conta> = Box<Conta>(give c);
    velha: Conta? = bc.take();
    bc.put(Conta(9.0));
    printl(velha?.saldo, bc.value?.saldo);
    burn velha;
    burn bc;
}
"#;
    assert_eq!(out(src), "5\n5 7\npi 3.14\n5\n1.0\n1.0 9.0\n");
    let src = r#"interface Forma {
    proc area(&self) -> real;
}
@impl(Forma)
class Quadrado {
    lado: real;
    public proc Quadrado(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override public proc area(&self) -> real { return self.lado * self.lado; }
}

class List<T> {
    items[]: T;
    public proc List() -> List<T> {
        xs[]: T = {};
        return List<T> { items: give xs };
    }
    public proc add(&self, x: T) { append(self.items, give x); }
    public proc size(&self) -> int { return len(self.items); }
    public proc removeAt(&self, i: int) -> T { return remove(self.items, i); }
}

proc total<T extends Forma>(xs: &List<T>) -> real {
    return 0.0;
}

proc maior<T extends Forma>(a: &T, b: &T) -> real {
    if a.area() > b.area() { return a.area(); }
    return b.area();
}

proc main() {
    ns: List<int> = List<int>();
    ns.add(3);
    ns.add(4);
    printl(ns.size(), ns.removeAt(0), ns.size());
    burn ns;
    qs: List<Quadrado> = List<Quadrado>();
    qs.add(Quadrado(2.0));
    q: Quadrado = qs.removeAt(0);
    printl(q.area());
    p: Quadrado = Quadrado(3.0);
    printl(maior<Quadrado>(&q, &p));
    burn p;
    burn q;
    burn qs;
    nested: List<List<int>> = List<List<int>>();
    nested.add(List<int>());
    printl(nested.size());
    burn nested;
}
"#;
    assert_eq!(out(src), "2 3 1\n4.0\n9.0\n1\n");
    for (src, why) in [
        // Each generic is checked once for every possible `T`: a `T` may be an object.
        ("class B<T> { v: T; public proc get(&self) -> T { return self.v; } } proc main() { }", "expected T, found &T"),
        ("proc f<T>(x: &T) { printl(x.campo); } proc main() { }", "`campo` is hidden"),
        ("interface F { proc a(&self) -> int; } proc f<T extends F>(x: &T) { } proc main() { f<int>(5); }", "`int` does not implement `F`"),
        ("class B<T> { v: T?; } proc main() { b: B<int, text>?; }", "takes 1 type argument, found 2"),
        ("class C { x: int; } proc main() { c: C<int>?; }", "`C` is not generic"),
        ("class T { x: int; } class B<T> { v: T?; } proc main() { }", "has the name of a type"),
        ("class B<T> { v: T?; } proc main() { b: B<int[]>?; }", "cannot be an array"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
    // `<` and `>` are still comparisons where no type arguments fit.
    assert_eq!(main_out("a: int = 3; b: int = 4; printl(a < b, a > b, a < b && b > a);"), "true false true\n");
}

#[test]
fn constructors() {
    let src = r#"class Conta {
    public dono: text;
    saldo: real;
    public proc Conta(dono: text) -> Conta { return Conta { dono: dono, saldo: 0.0 }; }
    public proc vazia() -> Conta { return Conta("?"); }
}
class Box<T> {
    public v: T?;
    public proc Box() -> Box<T> { return Box<T> { v: null }; }
}
proc main() {
    c: Conta = Conta("Ana");
    d: Conta = Conta.vazia();
    b: Box<int> = Box<int>();
    printl(c.dono, d.dono, b.v);
    burn c;
    burn d;
    burn b;
}
"#;
    assert_eq!(out(src), "Ana ? null\n");
    for (src, why) in [
        ("class C { x: int; } proc main() { c: C = C(); burn c; }", "`C` has no constructor"),
        ("interface F { proc a(&self) -> int; } proc main() { f: F = F(); burn f; }", "is an interface: it has no constructor"),
        ("class C { x: int; public proc C(&self) -> C { return C { x: 1 }; } } proc main() { }", "is the constructor"),
        ("class C { x: int; public proc C() -> int { return 1; } } proc main() { }", "is the constructor"),
        ("class C { x: int; proc C() -> C { return C { x: 1 }; } } proc main() { c: C = C(); burn c; }", "is private"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn returning_borrows() {
    let src = r#"class Conta {
    public saldo: real;
    public proc Conta(s: real) -> Conta { return Conta { saldo: s }; }
    public proc depositar(&self, v: real) { self.saldo += v; }
}

class List<T> {
    items[]: T;
    public proc List() -> List<T> {
        xs[]: T = {};
        return List<T> { items: give xs };
    }
    public proc add(&self, x: T) { append(self.items, give x); }
    public proc get(&self, i: int) -> &T { return &self.items[i]; }
    public proc size(&self) -> int { return len(self.items); }
}

proc ver(c: &Conta) { printl("saldo", c.saldo); }

proc primeiro(&v[]: Conta) -> &Conta { return &v[0]; }

proc main() {
    contas: List<Conta> = List<Conta>();
    contas.add(Conta(10.0));
    contas.add(Conta(20.0));
    contas.get(1).depositar(5.0);
    ver(contas.get(1));
    c: &Conta = contas.get(0);
    c.depositar(1.0);
    printl(c.saldo);
    printl(contas.get(0).saldo);
    ns: List<int> = List<int>();
    ns.add(7);
    n: int = ns.get(0);
    printl(n + 1);
    vs[]: Conta = {Conta(3.0)};
    printl(primeiro(&vs).saldo);
    burn vs;
    burn ns;
    burn contas;
}
"#;
    assert_eq!(out(src), "saldo 25.0\n11.0\n11.0\n8\n3.0\n");
    let decls = r#"class Conta {
    public saldo: real;
    public proc Conta(s: real) -> Conta { return Conta { saldo: s }; }
    public proc depositar(&self, v: real) { self.saldo += v; }
}

class List<T> {
    items[]: T;
    public proc List() -> List<T> {
        xs[]: T = {};
        return List<T> { items: give xs };
    }
    public proc add(&self, x: T) { append(self.items, give x); }
    public proc get(&self, i: int) -> &T { return &self.items[i]; }
    public proc size(&self) -> int { return len(self.items); }
}
"#;
    for (body, why) in [
        ("c: &Conta = contas.get(0); burn contas; printl(c.saldo);", "`contas` cannot be burned while `c` borrows from it"),
        ("c: &Conta = contas.get(0); printl(contas.get(0).saldo); printl(c.saldo); burn contas;", "cannot be borrowed by a call while `c`"),
        ("c: Conta = contas.get(0); burn c; burn contas;", "expected Conta, found &Conta"),
    ] {
        let e = ksc::compile(&format!("{decls} proc main() {{ contas: List<Conta> = List<Conta>(); contas.add(Conta(1.0)); {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    for (src, why) in [
        ("class C { x: int; } proc mk() -> &C { } proc main() { }", "must take a `&` parameter"),
        ("class C { x: int; } proc f(a: &C, b: &C) -> &C { return b; } proc main() { }", "must be borrowed from `a`"),
        ("class C { x: int; } proc f(a: &C) -> &C { return C { x: 1 }; } proc main() { }", "returns a borrow"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn reference_parameters() {
    // `x: &int`: the proc changes the caller's value (copied in, written back).
    let src = r#"struct Ponto { x: int; y: int; }

class Conta {
    public saldo: real;
    public proc Conta(s: real) -> Conta { return Conta { saldo: s }; }
}

proc dobrar(x: &int) { x *= 2; }

proc trocar(a: &text, b: &text) {
    t: text = a;
    a = b;
    b = t;
}

proc dividir(a: int, b: int, resto: &int) -> int {
    resto = a % b;
    return a / b;
}

proc mover(p: &Ponto) { p.x += 10; }

proc zerar(v: &int?) { v = null; }

proc main() {
    n: int = 5;
    dobrar(&n);
    printl(n);
    s1: text = "a";
    s2: text = "b";
    trocar(&s1, &s2);
    printl(s1, s2);
    r: int = 0;
    q: int = dividir(17, 5, &r);
    printl(q, r);
    p: Ponto = Ponto { x: 1, y: 2 };
    mover(&p);
    dobrar(&p.y);
    printl(p);
    vs[]: int = {1, 2, 3};
    i: int = 2;
    dobrar(&vs[i]);
    dobrar(&vs[0]);
    printl(vs);
    c: Conta = Conta(1.5);
    k: int = 3;
    dobrar(&k);
    printl(k, c.saldo);
    m: int? = 4;
    zerar(&m);
    printl(m);
    burn vs;
    burn c;
}
"#;
    assert_eq!(out(src), "10\nb a\n3 2\nPonto { x: 11, y: 4 }\n[2, 2, 6]\n6 1.5\nnull\n");
    for (body, why) in [
        ("n: int = 1; dobrar(n);", "pass it with `&`"),
        ("final n: int = 1; dobrar(&n);", "`n` is final"),
        ("t: tryte = 1; dobrar(&t);", "is &int, found tryte"),
        ("n: int = 1; ambos(&n, &n);", "passed with `&` twice"),
        ("vs[]: int = {1}; dobrar(&vs[len(vs) - 1]); burn vs;", "index is a number or a variable"),
        ("walk i in 0..2 { dobrar(&i); }", "read-only"),
    ] {
        let e = ksc::compile(&format!("proc dobrar(x: &int) {{ x *= 2; }} proc ambos(a: &int, b: &int) {{ }} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    let e = ksc::compile("interface F { proc m(&self, x: &int); } proc main() { }").unwrap_err();
    assert!(e.message.contains("cannot take a value with `&` yet"), "{}", e.message);
}

#[test]
fn object_methods() {
    // Every value has `toString`, `hashCode` and `equals`.
    let src = r#"enum Cor { Verde, Azul }
struct Ponto { x: int; y: int?; nome: text; }
proc main() {
    n: int = 42;
    printl(n.toString() + "!", n.hashCode(), n.equals(&n), n.equals(43));
    t: text = "abc";
    printl(t.toString(), t.hashCode() == "abc".hashCode(), t.hashCode() == "abd".hashCode(), t.equals("abc"));
    c: Cor = Cor.Azul;
    printl(c.toString(), c.equals(Cor.Verde));
    p: Ponto = Ponto { x: 1, y: null, nome: "a" };
    q: Ponto = Ponto { x: 1, y: null, nome: "a" };
    r: Ponto = Ponto { x: 1, y: 2, nome: "a" };
    printl(p.toString());
    printl(p.equals(&q), p.equals(&r), p.hashCode() == q.hashCode(), p.hashCode() == r.hashCode());
    m: int? = null;
    printl(m.toString(), m.hashCode(), m.equals(null), m.equals(3));
    printl(hashMix(1, 2), 3.5.toString());
}
"#;
    assert_eq!(
        out(src),
        "42! 42 true false\nabc true false true\nAzul false\nPonto { x: 1, y: null, nome: \"a\" }\ntrue false true false\nnull 0 true false\n33 3.5\n"
    );
    // A class gets them field by field (or writes its own); `==` calls `equals`,
    // `print` calls `toString`, also through an interface.
    let src = r#"enum Cor { Verde, Azul }
interface Forma {
    proc area(&self) -> real;
}
@impl(Forma)
class Circulo {
    raio: real;
    public proc Circulo(r: real) -> Circulo { return Circulo { raio: r }; }
    @override public proc area(&self) -> real { return 3.0 * self.raio * self.raio; }
}
@impl(Forma)
class Quadrado {
    lado: real;
    public proc Quadrado(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override public proc area(&self) -> real { return self.lado * self.lado; }
    @override public proc toString(&self) -> text { return "quadrado " + toText(self.lado); }
}
class No {
    public valor: int;
    public cor: Cor;
    public nome: text;
    public proximo: No?;
    public tags[]: text;
    public proc No(v: int) -> No { t[]: text = {"a"}; return No { valor: v, cor: Cor.Azul, nome: "n", proximo: null, tags: give t }; }
}
proc main() {
    a: No = No(1);
    b: No = No(1);
    printl(a);
    printl(a == b, a.hashCode() == b.hashCode());
    b.proximo = No(2);
    printl(b.toString());
    printl(a == b, a != b);
    f: Forma = Circulo(1.0);
    g: Forma = Circulo(1.0);
    h: Forma = Quadrado(1.0);
    printl(f, h);
    printl(f == g, f == h, f.hashCode() == g.hashCode());
    burn f; burn g; burn h; burn a; burn b;
}
"#;
    assert_eq!(
        out(src),
        "No { valor: 1, cor: Azul, nome: \"n\", proximo: null, tags: [\"a\"] }\ntrue true\nNo { valor: 1, cor: Azul, nome: \"n\", proximo: No { valor: 2, cor: Azul, nome: \"n\", proximo: null, tags: [\"a\"] }, tags: [\"a\"] }\nfalse true\nCirculo { raio: 1.0 } quadrado 1.0\ntrue false true\n"
    );
    for (src, why) in [
        ("class C { x: int; public proc toString(&self) -> text { return \"c\"; } } proc main() { }", "comes from `Object`; mark it `@override`"),
        ("class C { x: int; @override public proc toString(&self) -> int { return 1; } } proc main() { }", "must be `proc toString(&self) -> text`"),
        ("class C { x: int; @override public proc equals(&self, o: &int) -> bool { return true; } } proc main() { }", "must be `proc equals(&self, other: &C) -> bool`"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn comparable() {
    let src = r#"enum Nivel { Baixo, Alto }

@impl(Comparable)
class Conta {
    public saldo: real;
    public proc Conta(s: real) -> Conta { return Conta { saldo: s }; }
    @override public proc compareTo(&self, other: &Conta) -> int {
        if self.saldo < other.saldo { return -1; }
        if self.saldo > other.saldo { return 1; }
        return 0;
    }
}

proc maiorQue<T extends Comparable>(a: &T, b: &T) -> bool {
    return a > b;
}

proc maiorValor<T extends Comparable>(a: T, b: T) -> T {
    if a.compareTo(&b) >= 0 { burn b; return a; }
    burn a;
    return b;
}

proc main() {
    printl("abc" < "abd", "b" > "abc", "x".compareTo("x"), 3.compareTo(7));
    printl(maiorValor<int>(3, 9), maiorValor<text>("pera", "maçã"), maiorValor<Nivel>(Nivel.Alto, Nivel.Baixo));
    a: Conta = Conta(10.0);
    b: Conta = Conta(20.0);
    printl(a < b, a >= b, a <=> b);
    printl(maiorQue<Conta>(&b, &a), maiorQue<int>(1, 2));
    c: Conta = maiorValor<Conta>(give a, give b);
    printl(c.saldo);
    burn c;
}
"#;
    assert_eq!(out(src), "true true 0 -1\n9 pera Alto\ntrue false -1\ntrue false\n20.0\n");
    for (src, why) in [
        ("@impl(Comparable) class C { x: int; } proc main() { }", "has no method `compareTo`"),
        ("@impl(Comparable) class C { x: int; @override public proc compareTo(&self, o: &int) -> int { return 0; } } proc main() { }", "must be `@override public proc compareTo(&self, other: &C) -> int`"),
        ("class C { x: int; public proc C() -> C { return C { x: 1 }; } } proc main() { a: C = C(); b: C = C(); printl(a < b); burn a; burn b; }", "has no method `compareTo`"),
        ("struct P { x: int; } proc m<T extends Comparable>(a: T) { } proc main() { m<P>(P { x: 1 }); }", "`P` does not implement `Comparable`"),
    ] {
        let e = ksc::compile(src).unwrap_err();
        assert!(e.message.contains(why), "{src}: {}", e.message);
    }
}

#[test]
fn elvis_default() {
    // `a ?: b`: `a`, or `b` when `a` is null (as in Kotlin, looser than `+`).
    let src = r#"class C { x: int; public proc C() -> C { return C { x: 1 }; } }
proc main() {
    n: int = toInt("12") ?: 0;
    m: int = toInt("abc") ?: -1;
    printl(n, m, 1 + (toInt("x") ?: 5) * 2);
    a: int? = null;
    b: int? = 7;
    c: int? = a ?: b;
    d: int = a ?: b ?: 0;
    s: text? = null;
    nome: text = s ?: "anônimo";
    printl(c, d, nome);
}
"#;
    assert_eq!(out(src), "12 -1 11\n7 7 anônimo\n");
    let c = "class C { x: int; public proc C() -> C { return C { x: 1 }; } }";
    for (body, why) in [
        ("x: int = 1; y: int = x ?: 2;", "never null"),
        ("c: C? = null; d: C = c ?: C(); burn d; burn c;", "`?:` works on numbers"),
        ("c: C? = C(); printl(c); burn c;", "it may be null"),
        ("x: int = 1 + toInt(\"x\") ?: 2;", "it may be null"),
    ] {
        let e = ksc::compile(&format!("{c} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
}

/// Compiles a program of several files: `main` is `Main.ks`, `files` maps
/// `Shapes.Circle` to the text of `Shapes/Circle.ks`.
fn project(main: &str, files: &[(&str, &str)]) -> Result<Module, String> {
    let files: std::collections::HashMap<String, String> = files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let (map, r) = ksc::compile_program("Main.ks", main, &files, true);
    r.map(|o| o.module).map_err(|d| map.render(&d))
}

fn project_out(main: &str, files: &[(&str, &str)]) -> String {
    let m = project(main, files).unwrap_or_else(|e| panic!("compile error:\n{e}"));
    let mut out = Vec::new();
    Vm::new(&m, "".as_bytes(), &mut out).run().unwrap_or_else(|t| panic!("trapped: {t}"));
    String::from_utf8(out).unwrap()
}

fn project_err(main: &str, files: &[(&str, &str)]) -> String {
    project(main, files).expect_err("expected a compile error")
}

#[test]
fn modules() {
    let geometry = "public const PI: real = 3.0;
        public struct Point { x: int; y: int; }
        public enum Color { Red, Green }
        public interface Shape { proc name(&self) -> text; }
        public proc area(r: real) -> real { return PI * square(r); }
        proc square(x: real) -> real { return x * x; }";
    let circle = "use Geometry;
        @impl(Geometry.Shape) public class Circle {
            r: real;
            public proc Circle(r: real) -> Circle { return Circle { r: r }; }
            public proc radius(&self) -> real { return self.r; }
            @override public proc name(&self) -> text { return \"circle\"; }
        }";
    let list = "public class List<T> {
            items[]: T;
            public proc List() -> List<T> { return List<T> { items: {} }; }
            public proc add(&self, x: T) { append(self.items, give x); }
            public proc size(&self) -> int { return len(self.items); }
        }";
    let files = [("Geometry", geometry), ("shapes.Circle", circle), ("List", list)];
    let main = "use Geometry;
        use shapes.Circle;
        use List;
        proc show(s: &Geometry.Shape) { printl(s.name()); }
        proc main() {
            printl(Geometry.area(2.0));
            c: Circle = Circle(1.5);
            printl(c.radius());
            printl(c);
            show(&c);
            p: Geometry.Point = Geometry.Point { x: 1, y: 2 };
            k: Geometry.Color = Geometry.Color.Green;
            printf(\"{p.x + p.y} {k} {Geometry.PI}\\n\");
            l: List<int> = List<int>();
            l.add(3);
            printl(l.size());
            burn l;
            burn c;
        }";
    assert_eq!(project_out(main, &files), "12.0\n1.5\nCircle { r: 1.5 }\ncircle\n3 Green 3.0\n1\n");

    // Two files may use each other.
    let a = "use B;\npublic proc a(n: int) -> int { if n <= 0 { return 0; } return B.b(n - 1) + 1; }";
    let b = "use A;\npublic proc b(n: int) -> int { if n <= 0 { return 0; } return A.a(n - 1) + 10; }";
    assert_eq!(project_out("use A;\nproc main() { printl(A.a(4)); }", &[("A", a), ("B", b)]), "22\n");
    // Each file has its own private names.
    let x = "public proc get() -> int { return helper(); }\nproc helper() -> int { return 1; }";
    let y = "public proc get() -> int { return helper(); }\nproc helper() -> int { return 2; }";
    assert_eq!(project_out("use X;\nuse Y;\nproc main() { printl(X.get() + Y.get()); }", &[("X", x), ("Y", y)]), "3\n");

    // Interface calls and frees reach classes of files the caller does not
    // know: the linker writes them knowing every class.
    let shapes = "public interface Shape { proc area(&self) -> real; }
        public proc show(s: &Shape) -> real { return s.area(); }
        public proc isSquare(s: &Shape) -> bool { return s is Sq; }
        public proc drop(s: Shape) { burn s; }
        @impl(Shape) class Sq { a: real; @override public proc area(&self) -> real { return self.a; } }
        public proc name(s: &Shape) -> text { match s { Sq => return \"sq\"; } }";
    let circle = "use Shapes;
        @impl(Shapes.Shape) public class Circle {
            r: real;
            marks[]: int;
            public proc Circle(r: real) -> Circle { return Circle { r: r, marks: {1, 2} }; }
            @override public proc area(&self) -> real { return 3.0 * self.r * self.r; }
        }";
    let files = [("Shapes", shapes), ("Circle", circle)];
    let main = "use Shapes;\nuse Circle;
        proc main() { s: Shapes.Shape = Circle(1.0); printl(Shapes.show(&s)); printl(s); printl(Shapes.isSquare(&s)); Shapes.drop(give s); }";
    assert_eq!(project_out(main, &files), "3.0\nCircle { r: 1.0, marks: [1, 2] }\nfalse\n");
    let m = project("use Shapes;\nuse Circle;\nproc main() { s: Shapes.Shape = Circle(1.0); printl(Shapes.name(&s)); burn s; }", &files).unwrap();
    let trap = Vm::new(&m, "".as_bytes(), &mut Vec::new()).run().unwrap_err();
    assert!(trap.reason.contains("no arm of this `match` lists"), "{}", trap.reason);

    let e = project_err("use X;\nproc main() { printl(X.helper()); }", &[("X", x)]);
    assert!(e.contains("`helper` is private to `X`") && e.contains("Main.ks:2:"), "{e}");
    let e = project_err("use X;\nproc main() { printl(X.nope()); }", &[("X", x)]);
    assert!(e.contains("`X` has no item `nope`"), "{e}");
    let e = project_err("use Nope;\nproc main() { }", &[]);
    assert!(e.contains("cannot find `Nope.ks`"), "{e}");
    let e = project_err("use x;\nproc main() { }", &[]);
    assert!(e.contains("PascalCase"), "{e}");
    let e = project_err("use Main;\nproc main() { }", &[]);
    assert!(e.contains("main file"), "{e}");
    let e = project_err("proc main() { printl(X.get()); }", &[("X", x)]);
    assert!(e.contains("add `use X;`"), "{e}");
    // An error in another file is shown in that file.
    let e = project_err("use Bad;\nproc main() { printl(Bad.f()); }", &[("Bad", "public proc f() -> int { return \"x\"; }")]);
    assert!(e.contains("--> Bad.ks:1:"), "{e}");
    let e = project_err("proc main() { }\nuse X;", &[("X", x)]);
    assert!(e.contains("`use` goes at the top"), "{e}");
    let e = project_err("public proc f() { }\nprivate proc g() { }\nproc main() { }", &[]);
    assert!(e.contains("private to their file already"), "{e}");
}

#[test]
fn build_cache() {
    // A folder of its own for this test.
    let dir = std::env::temp_dir().join(format!("kestrel-build-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write("Lib.ks", "public proc f() -> int { return 1; }");
    write("Other.ks", "public proc g() -> int { return 2; }");
    let main = "use Lib;\nuse Other;\nproc main() { printl(Lib.f() + Other.g()); }";
    let cache = ksc::Cache { dir: dir.join("build"), compiler: 7 };
    let build = || {
        let (map, r) = ksc::build("Main.ks", main, &ksc::modules::Disk { src: dir.clone(), lib: None }, true, Some(&cache));
        let o = r.unwrap_or_else(|d| panic!("{}", map.render(&d)));
        let mut out = Vec::new();
        Vm::new(&o.module, "".as_bytes(), &mut out).run().unwrap();
        let mut compiled: Vec<String> = o.compiled.iter().map(|p| p.rsplit('/').next().unwrap().to_string()).collect();
        compiled.sort();
        (String::from_utf8(out).unwrap(), compiled)
    };
    assert_eq!(build(), ("3\n".to_string(), vec!["Lib.ks".to_string(), "Main.ks".into(), "Other.ks".into()]));
    // Nothing changed: nothing is compiled.
    assert_eq!(build().1, Vec::<String>::new());
    // A body changed: only that file.
    write("Lib.ks", "public proc f() -> int { return 10; }");
    assert_eq!(build(), ("12\n".to_string(), vec!["Lib.ks".to_string()]));
    // A comment or the layout changed: only that file too.
    write("Lib.ks", "// the library\npublic proc f()\n    -> int { return 10; }");
    assert_eq!(build().1, vec!["Lib.ks".to_string()]);
    // Its public part changed: the files that use it too.
    write("Lib.ks", "public proc f() -> int { return 10; }\npublic proc h() { }");
    assert_eq!(build().1, vec!["Lib.ks".to_string(), "Main.ks".into()]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn modules_with_generics_and_defaults() {
    // Generics, default methods, `Comparable` and enums across files; a
    // copy of a generic of `Util` calls a private proc of `Util`.
    let util = r#"public enum Color { Red, Green, Blue }
public const LIMIT: int = 10;
public interface Named {
    proc name(&self) -> text;
    proc greet(&self) -> text { return "hi " + self.name(); }
}
public proc bigger<T extends Comparable>(a: &T, b: &T) -> bool { return a > b; }
public class Box<T> {
    item: T;
    public proc Box(x: T) -> Box<T> { return Box<T> { item: give x }; }
    public proc get(&self) -> &T { return &self.item; }
    public proc twice(&self) -> int { return helper() * 2; }
}
proc helper() -> int { printf("limit {LIMIT}\n"); return LIMIT; }
"#;
    let pets = r#"use Util;
@impl(Util.Named, Comparable) public class Dog {
    n: text;
    age: int;
    public proc Dog(n: text, age: int) -> Dog { return Dog { n: n, age: age }; }
    @override public proc name(&self) -> text { return self.n; }
    @override public proc compareTo(&self, other: &Dog) -> int { return self.age - other.age; }
}
"#;
    let main = r#"use Util;
use Pets;
proc main() {
    k: Util.Color = Util.Color.Blue;
    match k {
        Util.Color.Red => printl("red");
        Util.Color.Blue => printl("blue");
        else => printl("other");
    }
    x: int = 3;
    y: int = 9;
    printl(Util.bigger<int>(&y, &x));
    d: Pets.Dog = Pets.Dog("Rex", 3);
    e: Pets.Dog = Pets.Dog("Bo", 5);
    printl(d < e);
    n: Util.Named = give d;
    printl(n.greet());
    printl(n is Pets.Dog);
    b: Util.Box<Pets.Dog> = Util.Box<Pets.Dog>(give e);
    printl(b.get().name());
    printl(b.twice());
    b2: Util.Box<int> = Util.Box<int>(4);
    printl(b2);
    burn n;
    burn b;
    burn b2;
}
"#;
    let out = project_out(main, &[("Util", util), ("Pets", pets)]);
    assert_eq!(out, "blue\ntrue\ntrue\nhi Rex\ntrue\nBo\nlimit 10\n20\nBox { item: 4 }\n");
    // Comparisons after a dotted name still parse.
    assert_eq!(project_out("use Util;\nproc main() { printl(Util.LIMIT < 11 && Util.LIMIT > 9); }", &[("Util", util)]), "true\n");
}

#[test]
fn libraries() {
    // A library compiled in one folder works in another with only its `.ksb`.
    let dir = std::env::temp_dir().join(format!("kestrel-libraries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (lib_src, app) = (dir.join("lib-src"), dir.join("app"));
    std::fs::create_dir_all(lib_src.join("corpos")).unwrap();
    std::fs::write(
        lib_src.join("corpos/Corpo.ks"),
        "public class Corpo {
            m: real;
            public proc Corpo(m: real) -> Corpo { return Corpo { m: m }; }
            public proc peso(&self) -> real { return self.m * g(); }
        }
        public class Box<T> { x: T; public proc Box(x: T) -> Box<T> { return Box<T> { x: give x }; } public proc twice(&self) -> real { return g() * 2.0; } }
        proc g() -> real { return 9.8; }
        @test proc pesoCerto() { c: Corpo = Corpo(2.0); assert(c.peso() == 19.6); burn c; }",
    )
    .unwrap();
    let lib_cache = ksc::Cache { dir: lib_src.join("build/obj"), compiler: 1 };
    let disk = ksc::modules::Disk { src: lib_src.clone(), lib: None };
    let (map, r) = ksc::build("$all.ks", "use corpos.Corpo;", &disk, false, Some(&lib_cache));
    let o = r.unwrap_or_else(|d| panic!("{}", map.render(&d)));
    assert_eq!(o.tests.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["corpos.Corpo.pesoCerto"]);
    std::fs::create_dir_all(app.join("lib/corpos")).unwrap();
    std::fs::copy(lib_src.join("build/obj/corpos/Corpo.ksb"), app.join("lib/corpos/Corpo.ksb")).unwrap();
    std::fs::create_dir_all(app.join("src")).unwrap();

    let run = |main: &str| -> Result<String, String> {
        let cache = ksc::Cache { dir: app.join("build/obj"), compiler: 1 };
        let (map, r) = ksc::build("Main.ks", main, &ksc::modules::Disk::project(&app), true, Some(&cache));
        let o = r.map_err(|d| map.render(&d))?;
        let mut out = Vec::new();
        Vm::new(&o.module, "".as_bytes(), &mut out).run().unwrap();
        Ok(String::from_utf8(out).unwrap())
    };
    // A generic of the library is compiled here and calls its private `g`.
    let main = "use corpos.Corpo;
        proc main() { c: Corpo = Corpo(1.0); printl(c.peso()); burn c; b: Corpo.Box<int> = Corpo.Box<int>(3); printl(b.twice()); burn b; }";
    assert_eq!(run(main).unwrap(), "9.8\n19.6\n");
    let e = run("use corpos.Corpo;\nproc main() { printl(Corpo.g()); }").unwrap_err();
    assert!(e.contains("`g` is private to `corpos.Corpo`"), "{e}");
    let e = run("use corpos.Corpo;\nproc main() { c: Corpo = Corpo(\"x\"); burn c; }").unwrap_err();
    assert!(e.contains("expected real, found text") && e.contains("Main.ks:2:"), "{e}");
    // Names: lowercase folders, PascalCase files, keywords as folders.
    assert!(run("use Corpos.Corpo;\nproc main() { }").unwrap_err().contains("folder names are lowercase"));
    assert!(run("use corpos.corpo;\nproc main() { }").unwrap_err().contains("PascalCase"));
    assert!(run("use std.text.Nope;\nproc main() { }").unwrap_err().contains("standard library has no `std.text.Nope`"));
    // The same file in src/ and lib/ is an error.
    std::fs::create_dir_all(app.join("src/corpos")).unwrap();
    std::fs::write(app.join("src/corpos/Corpo.ks"), "public proc f() { }").unwrap();
    assert!(run("use corpos.Corpo;\nproc main() { }").unwrap_err().contains("keep one"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Error classes for the `try`/`catch` tests.
const ERRORS: &str = r#"
@impl(Error)
class NotFound {
    path: text;
    public proc NotFound(p: text) -> NotFound { return NotFound { path: p }; }
    @override
    public proc message(&self) -> text { return "not found: " + self.path; }
}
@impl(Error)
class Bad {
    code: int;
    public proc Bad(c: int) -> Bad { return Bad { code: c }; }
    @override
    public proc message(&self) -> text { return "bad " + toText(self.code); }
}
proc load(p: text) -> int throws NotFound, Bad {
    v[]: int = {1, 2, 3};
    if p == "x" { throw NotFound(p); }
    if p == "b" { throw Bad(4); }
    burn v;
    return 7;
}
"#;

#[test]
fn try_catch() {
    let src = format!(
        r#"{ERRORS}
proc middle(p: text) -> int throws NotFound {{
    try {{
        return load(p);
    }} catch e: Bad {{
        printl("middle: " + e.message());
        burn e;
    }}
    return -1;
}}
proc divide(a: int, b: int) -> int {{ return a / b; }}
proc main() {{
    try {{
        printl(middle("ok"));
        printl(middle("b"));
        printl(middle("x"));
        printl("not here");
    }} catch e: NotFound {{
        printl("main: " + e.message());
        burn e;
    }}
    try {{
        printl(divide(1, 0));
    }} catch e: Trap {{
        printl("trap: " + e.message());
        burn e;
    }}
    try {{
        printl(load("b"));
    }} catch e: Error {{
        printl("any: " + e.message());
        burn e;
    }}
    printl("end");
}}"#
    );
    assert_eq!(
        out(&src),
        "7\nmiddle: bad 4\n-1\nmain: not found: x\ntrap: division by zero (handle it with `on NaT`)\nany: bad 4\nend\n"
    );
    // Uncaught, a failure still stops the program where it happened.
    let (r, _) = run_with("proc f(a: int) -> int { return 1 / a; } proc main() { printl(f(0)); }", "");
    let t = r.unwrap_err();
    assert_eq!((t.func.as_str(), t.reason.as_str()), ("f", "division by zero (handle it with `on NaT`)"));
    // A rethrow from a catch goes to the next `try` out.
    let src = format!(
        "{ERRORS} proc main() {{ try {{ try {{ load(\"x\"); }} catch e: NotFound {{ throw e; }} }} catch e: Error {{ printl(e.message()); burn e; }} }}"
    );
    assert_eq!(out(&src), "not found: x\n");
}

#[test]
fn try_frees_what_it_owned() {
    // `v` (given to `f`) and `a` (made in the `try`) are freed by the error;
    // only `w` is left in the heap at the `catch`.
    let src = format!(
        "{ERRORS} proc f(v[]: int) throws NotFound {{ throw NotFound(\"a\"); }} \
         proc main() {{ v[]: int = {{1, 2}}; w[]: int = {{9}}; \
         try {{ a[]: int = {{5, 5, 5}}; f(give v); burn a; }} catch e: NotFound {{ burn e; dump(\"c\"); }} burn w; }}"
    );
    let o = out(&src);
    assert_eq!(o.matches(" cells): ").count(), 1, "{o}");
    assert!(o.contains("[0]=9"), "{o}");
}

#[test]
fn checked_errors() {
    for (body, why) in [
        ("proc f() throws NotFound { throw NotFound(\"a\"); } proc main() { f(); }", "which `main` neither catches nor declares"),
        ("proc main() { try { printl(1); } catch e: NotFound { burn e; } }", "nothing in this `try` throws `NotFound`"),
        (
            "proc main() { try { load(\"x\"); } catch e: Error { burn e; } catch e: NotFound { burn e; } }",
            "this `catch` never runs",
        ),
        ("proc main() { try { load(\"x\"); } catch e: Error { burn e; } catch e: Error { burn e; } }", "this `catch` never runs"),
        ("proc main() throws NotFound { throw NotFound(\"a\"); }", "`main` cannot throw"),
        ("proc main() { try { load(\"x\"); } catch e: Error { printl(1); } }", "`e` is never burned"),
        (
            "proc main() { v[]: int = {1}; try { load(\"x\"); burn v; } catch e: Error { burn e; printl(len(v)); } }",
            "`v` is used after it was burned",
        ),
        ("proc main() { throw 5; }", "`throw` needs an error"),
        ("class P { x: int; } proc main() { try { load(\"x\"); } catch e: P { burn e; } }", "`P` is not an error"),
        ("proc main() { try { load(\"x\"); } catch e: Nope { burn e; } }", "unknown name `Nope`"),
        (
            "interface S { proc get(&self) -> int; } @impl(S) class A { x: int; @override public proc get(&self) -> int throws Bad { throw Bad(1); } }",
            "`A.get` throws `Bad`, which `S.get` does not list",
        ),
        ("proc main() { catch e: Error { } }", "`catch` without a `try`"),
        ("proc main() { try { printl(1); } }", "`catch e: SomeError"),
    ] {
        let src = format!("{ERRORS} {body}");
        let src = if src.contains("proc main") { src } else { src + " proc main() {}" };
        let e = ksc::compile(&src).expect_err(body);
        assert!(e.message.contains(why) || e.note.as_ref().is_some_and(|n| n.contains(why)), "{body}: got {}", e.message);
    }
    // Through an interface: the interface's `throws` is what callers handle.
    let src = format!(
        "{ERRORS} interface S {{ proc get(&self) -> int throws Bad; }} \
         @impl(S) class A {{ x: int; public proc A() -> A {{ return A {{ x: 1 }}; }} @override public proc get(&self) -> int throws Bad {{ throw Bad(2); }} }} \
         proc main() {{ s: S = A(); try {{ printl(s.get()); }} catch e: Bad {{ printl(e.message()); burn e; }} burn s; }}"
    );
    assert_eq!(out(&src), "bad 2\n");
}

#[test]
fn traceback_has_lines() {
    let src = "proc divide(a: int, b: int) -> int {\n    return a / b;\n}\nproc deep(n: int) -> int {\n    if n == 0 { return divide(1, n); }\n    return deep(n - 1) + 1;\n}\nproc main() {\n    printl(1);\n    printl(deep(3));\n}\n";
    let (r, _) = run_with(src, "");
    let t = r.unwrap_err();
    let at: Vec<(String, Option<u32>)> = t.trace.iter().map(|c| (c.func.clone(), c.line)).collect();
    let want = [("divide", 2), ("deep", 5), ("deep", 6), ("deep", 6), ("deep", 6), ("main", 10)];
    assert_eq!(at, want.map(|(f, l)| (f.to_string(), Some(l))));
    assert_eq!(
        t.to_string(),
        "TRAP: division by zero (handle it with `on NaT`)\n  at divide (main.ks:2)\n  at deep (main.ks:5)\n  at deep (main.ks:6) (3 times)\n  at main (main.ks:10)"
    );
}
