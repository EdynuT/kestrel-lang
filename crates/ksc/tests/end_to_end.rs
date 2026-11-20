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
        print(len(t)); print(" "); emit(t[2]); print(" " + toText(1.0 / 4.0) + " " + toText(NaT));"#;
    assert_eq!(main_out(body), "n = -5 = 0t-++ = 0ve\n5 - 0.25 NaT");
}

#[test]
fn print_concatenation() {
    assert_eq!(main_out(r#"name: text = "Kestrel"; print("Hello, " + name + "!" + "\n");"#), "Hello, Kestrel!\n");
}

#[test]
fn spec_example() {
    let src = r#"
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k { acc += v[i]; }
    return acc;
}
proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 { b[i] = i * i; }
    s: int = sum(lend b, 2) on NaT { burn b; return -1; };
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
    let why = main_trap("x: int = NaT; print(x + 1);");
    assert!(why.contains("got a NaT value") && why.contains("handle it with `on NaT`"), "{why}");
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
    // A proc can still report failure on purpose by returning NaT.
    let src = "proc safe(a: int, b: int) -> int { if b == 0 { return NaT; } return a / b; }
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
proc fill(lent sym v[]: int, k: int) { walk i in span k { v[i] = i * 10; } }
proc total(lent sym v[]: int, k: int) -> int { s: int = 0; walk i in span k { s += v[i]; } return s; }
proc make(n: int) -> sym int[] { sym b[]: int = claim 2 * n + 1; return b; }
proc consume(own sym b[]: int) -> int { x: int = b[0]; burn b; return x; }
proc main() {
    sym b[]: int = make(2);
    fill(lend b, 2);
    print(b[-2]); print(" "); print(b[2]); print(" "); print(total(lend b, 2)); print(" ");
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
        printf("%d %d ", toInt("123") + toInt("-7"), toInt("0t+0-"));
        printf("%f %f\n", toReal("2.5") * 2.0, toReal("-0.75"));
        printf("%s%s%s%s%s %s%s\n", isDigit('7'), isLetter('ç'), isUpper('Á'), isLower('a'), isSpace('\n'), isDigit('a'), isLetter(' '));
        acc: text = ""; for i: int = 0; i < 3; i++ { acc = acc + toText(i) + ","; } print(acc);"#;
    assert_eq!(main_out(body), "Olá, Edynu!\n[Kes][trel][] true true\n116 8 5.0 -0.75\ntruetruetruetruetrue falsefalse\n0,1,2,");
    assert!(main_trap(r#"x: int = toInt("12a");"#).contains(r#"toInt("12a"): not an integer"#));
    assert!(main_trap(r#"x: real = toReal("abc");"#).contains(r#"toReal("abc"): not a number"#));
    let (r, _) = run_with(r#"proc main() { x: text = sub("abc", 2, 1); }"#, "");
    assert!(r.unwrap_err().reason.contains("needs 0 <= start <= end <= 3"));
    let e = ksc::compile(r#"proc main() { x: text = "a" + 1; }"#).unwrap_err();
    assert!(e.message.contains("joins text with text only"), "{}", e.message);
    let e = ksc::compile(r#"proc main() { t: trit = "a" < "b"; }"#).unwrap_err();
    assert!(e.note.unwrap().contains("`==` and `!=`"));
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
    // the same array cannot be lent twice to one call
    let e = ksc::compile("proc f(lent a[]: int, lent b[]: int) {} proc main() { x[]: int = claim 1; f(lend x, lend x); burn x; }").unwrap_err();
    assert!(e.message.contains("lent twice"), "{}", e.message);
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
        ("a: P = P { x: 1, y: 2 }; if a == a { } }", "expected a number, found P"),
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
        proc soma(lent ps[]: Ponto) -> Ponto {
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
            printl(ps[2]); printl(soma(lend ps));
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
        proc encher(lent v[]: int, n: int) { for i: int = 0; i < n; i++ { append(v, i * 10); } }
        proc main() {
            v[]: int = {};
            append(v, 4); append(v, 7); printl(v, len(v));
            x: int = pop(v); printl(x, v);
            encher(lend v, 3); printl(v);
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
    let defs = "proc g(own v[]: int) -> int { burn v; return 1; } proc f(lent v[]: int, x: int) { }";
    let e = ksc::compile(&format!("{defs} proc main() {{ v[]: int = {{1}}; append(v, g(give v)); }}")).unwrap_err();
    assert!(e.message.contains("given away inside a call that uses it"), "{}", e.message);
    let e = ksc::compile(&format!("{defs} proc main() {{ v[]: int = {{1}}; f(lend v, g(give v)); }}")).unwrap_err();
    assert!(e.message.contains("lent and given away in the same call"), "{}", e.message);
}

#[test]
fn regressions_from_breaking_0_1_to_0_4() {
    let take = "proc take(own v[]: int) -> int { burn v; return 1; }";
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

    public proc nova(nome: text) -> Conta {
        return Conta { nome: nome, saldo: 0.0, hist: {}, pos: Ponto { x: 1, y: 2 } };
    }
    public proc depositar(lent self, v: real) {
        self.saldo += v;
        append(self.hist, v);
        self.pos.x++;
    }
    public proc saldoAtual(lent self) -> real { return self.saldo; }
    public proc movimentos(lent self) -> int { return len(self.hist); }
    proc segredo(lent self) -> int { return 42; }
    public proc fechar(own self) -> real {
        s: real = self.saldo + (real) self.segredo();
        burn self;
        return s;
    }
}

class Banco {
    principal: Conta;
    public proc novo() -> Banco {
        c: Conta = Conta.nova("Caixa");
        return Banco { principal: give c };
    }
    public proc depositar(lent self, v: real) { self.principal.depositar(v); }
    public proc total(lent self) -> real { return self.principal.saldoAtual(); }
}

proc mostrar(lent c: Conta) {
    printl(c.nome, c.saldoAtual());
}

proc main() {
    c: Conta = Conta.nova("Ana");
    c.depositar(10.0);
    c.depositar(5.5);
    mostrar(lend c);
    printl(c);
    printl(c.movimentos());
    c.nome = "Ana Maria";
    total: real = c.fechar();
    printl(total);
    b: Banco = Banco.novo();
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
             public proc novo() -> C { return C.mk(); } public proc ler(lent self) -> int { return self.n; } \
             public proc fim(own self) { burn self; } public proc f(lent self, lent w[]: int) { } }";
    for (body, why) in [
        ("c: C = C.novo();", "`c` is never burned or given away"),
        ("c: C = C.novo(); x: int = c.v; burn c;", "`v` is private in `C`"),
        ("c: C = C.novo(); d: C = c; burn c;", "object `c` cannot be copied"),
        ("c: C = C { n: 1, v: {} }; burn c;", "`C` objects are created inside the class"),
        ("c: C = C.novo(); c.fim(); x: int = c.ler();", "`c` is used after it was given away"),
        ("c: C = C.novo(); x: int = C.ler(); burn c;", "`ler` needs an object"),
        ("c: C = C.novo(); c.novo(); burn c;", "`novo` is static"),
        ("x: int = C.novo().n;", "the object made here would be lost"),
        ("c: C = C.mk(); burn c;", "method `mk` is private in `C`"),
        ("c: C = C.novo(); c.f(lend c.n); burn c;", "only arrays and objects are lent"),
    ] {
        let e = ksc::compile(&format!("{c} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    let e = ksc::compile("class A { b: B; } class B { a: A; } proc main() {}").unwrap_err();
    assert!(e.message.contains("class `A` contains itself"), "{}", e.message);
    let e = ksc::compile(
        "class C { public v[]: int; public proc novo() -> C { return C { v: {} }; } public proc f(lent self, lent w[]: int) { } } \
         proc main() { c: C = C.novo(); c.f(lend c.v); burn c; }",
    )
    .unwrap_err();
    assert!(e.message.contains("`c` is lent twice"), "{}", e.message);
}

#[test]
fn every_variable_has_its_type_written() {
    let e = ksc::compile("proc main() { x := 5; }").unwrap_err();
    assert!(e.message.contains("expected a type, found `=`"), "{}", e.message);
    assert_eq!(e.note.as_deref(), Some("every variable has its type written: `x: int = 5;`"));
    // Declared without a value: it holds NaT (Kestrel's null) until assigned.
    let src = r#"class C { public n: int; public proc novo() -> C { return C { n: 7 }; } }
        struct P { x: int; r: real; t: text; }
        proc main() {
            total: int; nome: text; p: P; ok: bool;
            printl(total, nome, p, ok);
            total = 10; printl(total + 1);
            c: C;
            if total > 5 { c = C.novo(); printl(c.n); burn c; }
            y: int;
            z: int = y + 1 on NaT { printl("y has no value"); return; };
        }"#;
    assert_eq!(out(src), "NaT NaT P { x: NaT, r: NaT, t: NaT } NaT\n11\n7\ny has no value\n");
    let c = "class C { public n: int; public proc novo() -> C { return C { n: 7 }; } }";
    for (body, why) in [
        ("c: C; printl(c.n);", "`c` has no value yet"),
        ("c: C; burn c;", "`c` has no value to burn"),
        ("final k: int;", "final `k` needs a value"),
    ] {
        let e = ksc::compile(&format!("{c} proc main() {{ {body} }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    assert!(main_trap("y: int; z: int = y * 2;").contains("an operation got a NaT value"));
    assert!(main_trap("s: text; t: text = s + \"a\";").contains("use of a text that has no value"));
}

#[test]
fn is_nat() {
    let body = r#"a: int; b: int = 1; t: text; r: real = 1.5; ok: bool;
        printl(isNaT(a), isNaT(b), isNaT(t), isNaT(r), isNaT(ok), a == a);
        c: int = a + b on NaT {
            if isNaT(a) { printl("a has no value"); } else { printl("b has no value"); }
            return;
        };"#;
    assert_eq!(main_out(body), "true false true false true NaT\na has no value\n");
    let e = ksc::compile("struct P { x: int; } proc main() { p: P; b: bool = isNaT(p); }").unwrap_err();
    assert!(e.note.unwrap().contains("isNaT(p.x)"));
}

#[test]
fn interfaces() {
    let src = r#"interface Forma {
    proc area(lent self) -> real;
    proc nome(lent self) -> text;
    proc escalar(lent self, k: real);
}

@impl(Forma)
class Circulo {
    raio: real;
    public proc novo(r: real) -> Circulo { return Circulo { raio: r }; }
    @override
    public proc area(lent self) -> real { return 3.0 * self.raio * self.raio; }
    @override
    public proc nome(lent self) -> text { return "circulo"; }
    @override
    public proc escalar(lent self, k: real) { self.raio *= k; }
    public proc diametro(lent self) -> real { return 2.0 * self.raio; }
}

@impl(Forma)
class Quadrado {
    lado: real;
    public proc novo(l: real) -> Quadrado { return Quadrado { lado: l }; }
    @override
    public proc area(lent self) -> real { return self.lado * self.lado; }
    @override
    public proc nome(lent self) -> text { return "quadrado"; }
    @override
    public proc escalar(lent self, k: real) { self.lado *= k; }
}

proc mostrar(lent f: Forma) {
    printf("{f.nome()}: {f.area()}\n");
}

proc dobrarEMostrar(lent f: Forma) {
    f.escalar(2.0);
    mostrar(lend f);
}

proc main() {
    c: Circulo = Circulo.novo(1.0);
    q: Quadrado = Quadrado.novo(3.0);
    mostrar(lend c);
    mostrar(lend q);
    dobrarEMostrar(lend c);
    dobrarEMostrar(lend q);
    printl(c.diametro());
    burn c;
    burn q;
}
"#;
    assert_eq!(out(src), "circulo: 3.0\nquadrado: 9.0\ncirculo: 12.0\nquadrado: 36.0\n4.0\n");
    let i = "interface F { proc area(lent self) -> real; }";
    for (decls, why) in [
        ("@impl(F) class A { x: real; }", "`A` implements `F` but has no method `area`"),
        ("@impl(F) class A { x: real; @override public proc area(lent self) -> int { return 1; } }", "`A.area` does not match `F.area`"),
        ("@impl(F) class A { x: real; public proc area(lent self) -> real { return 1.0; } }", "mark it `@override`"),
        (
            "@impl(F) class A { x: real; @override public proc area(lent self) -> real { return 1.0; } \
             @override public proc aera(lent self) -> real { return 1.0; } }",
            "`aera` overrides nothing",
        ),
        ("@impl(F) class A { x: real; @override proc area(lent self) -> real { return 1.0; } }", "must be public"),
        ("proc f(lent g: F) { g.voar(); }", "interface `F` has no method `voar`"),
        ("@impl(G) class A { x: real; }", "`G` is not an interface"),
        ("proc f(g: F) { }", "an object parameter needs `lent` or `own`"),
        ("interface G { proc criar() -> int; }", "interface method `criar` needs `self`"),
    ] {
        let e = ksc::compile(&format!("{i} {decls} proc main() {{ }}")).unwrap_err();
        assert!(e.message.contains(why), "{decls}: {}", e.message);
    }
    let e = ksc::compile(&format!(
        "{i} class A {{ x: real; public proc novo() -> A {{ return A {{ x: 1.0 }}; }} }} proc f(lent g: F) {{ }} \
         proc main() {{ a: A = A.novo(); f(lend a); burn a; }}"
    ))
    .unwrap_err();
    assert!(e.message.contains("`A` does not implement `F`"), "{}", e.message);
}

#[test]
fn arrays_of_objects() {
    let src = r#"interface Nomeado {
    proc nome(lent self) -> text;
}

@impl(Nomeado)
class Conta {
    dono: text;
    saldo: real;
    public proc nova(dono: text) -> Conta { return Conta { dono: dono, saldo: 0.0 }; }
    @override
    public proc nome(lent self) -> text { return self.dono; }
    public proc depositar(lent self, v: real) { self.saldo += v; }
    public proc saldoAtual(lent self) -> real { return self.saldo; }
}

class Banco {
    contas[]: Conta;
    public proc novo() -> Banco {
        vazio[]: Conta = {};
        return Banco { contas: give vazio };
    }
    public proc abrir(lent self, dono: text) { append(self.contas, Conta.nova(dono)); }
    public proc total(lent self) -> real {
        t: real = 0.0;
        for i: int = 0; i < len(self.contas); i++ { t += self.contas[i].saldoAtual(); }
        return t;
    }
}

proc mostrar(lent n: Nomeado) { printl(n.nome()); }

proc main() {
    a: Conta = Conta.nova("Ana");
    cs[]: Conta = {give a, Conta.nova("Bia")};
    insert(cs, 1, Conta.nova("Caio"));
    cs[0].depositar(2.5);
    mostrar(lend cs[1]);
    printl(cs[2].nome(), len(cs));
    printl(cs[0]);
    x: Conta = remove(cs, 1);
    printl(x.nome(), len(cs));
    burn x;
    b: Banco = Banco.novo();
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

    let cls = "class C { x: int; public proc novo() -> C { return C { x: 1 }; } \
               public proc pega(lent self, own o: C) { burn o; } }
               proc par(lent c: C, own d: C) { burn d; }";
    for (body, why) in [
        ("x: C = cs[0];", "take it out with `remove(v, i)` or `pop(v)`"),
        ("cs[0] = C.novo();", "an object in an array cannot be replaced"),
        ("ds[]: C = claim 3; burn ds;", "`claim` would make empty `C` elements"),
        ("resize(cs, 3);", "`resize` does not work on an array of `C` objects"),
        ("clear(cs);", "`clear` does not work"),
        ("pop(cs);", "would be lost"),
        ("remove(cs, 0);", "would be lost"),
        ("c: C = C.novo(); append(cs, c); burn c;", "object `c` cannot be copied"),
        ("par(lend cs[0], remove(cs, 0));", "has an element taken out in the same call"),
        ("cs[0].pega(pop(cs));", "has an element taken out in the same call"),
    ] {
        let e = ksc::compile(&format!("{cls} proc main() {{ cs[]: C = {{C.novo()}}; {body} burn cs; }}")).unwrap_err();
        assert!(e.message.contains(why) || e.note.as_ref().is_some_and(|n| n.contains(why)), "{body}: {} {:?}", e.message, e.note);
    }
}

#[test]
fn owned_interfaces() {
    let src = r#"interface Forma {
    proc area(lent self) -> real;
    proc nome(lent self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;
    public proc novo(raio: real) -> Circulo { return Circulo { raio: raio }; }
    @override
    public proc area(lent self) -> real { return 3.0 * self.raio * self.raio; }
    @override
    public proc nome(lent self) -> text { return "circulo"; }
}

@impl(Forma)
class Poligono {
    lados[]: real;
    public proc novo(a: real, b: real) -> Poligono {
        ls[]: real = {a, b};
        return Poligono { lados: give ls };
    }
    @override
    public proc area(lent self) -> real { return self.lados[0] * self.lados[1]; }
    @override
    public proc nome(lent self) -> text { return "poligono"; }
}

class Desenho {
    principal: Forma;
    public proc novo(own f: Forma) -> Desenho { return Desenho { principal: give f }; }
    public proc mostrar(lent self) { mostrar(lend self.principal); }
}

proc mostrar(lent f: Forma) { printf("{f.nome()}: {f.area()}\n"); }

proc criar(quadrado: bool) -> Forma {
    if quadrado { return Poligono.novo(2.0, 2.0); }
    return Circulo.novo(1.0);
}

proc main() {
    c: Circulo = Circulo.novo(2.0);
    f: Forma = give c;
    printl(f.area());
    formas[]: Forma = {give f, Poligono.novo(3.0, 4.0)};
    append(formas, criar(false));
    for i: int = 0; i < len(formas); i++ {
        mostrar(lend formas[i]);
    }
    g: Forma = remove(formas, 1);
    d: Desenho = Desenho.novo(give g);
    d.mostrar();
    vazia: Forma;
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

    let decls = "interface F { proc n(lent self) -> int; }
        @impl(F) class A { x: int; public proc novo() -> A { return A { x: 1 }; }
                           @override public proc n(lent self) -> int { return self.x; } }
        class B { y: int; public proc novo() -> B { return B { y: 1 }; } }";
    for (body, why) in [
        ("a: A = A.novo(); f: F = a; burn f;", "object `a` cannot be copied"),
        ("f: F = B.novo(); burn f;", "`B` does not implement `F`"),
        ("fs[]: F = {A.novo()}; x: F = fs[0]; burn x; burn fs;", "take it out with `remove(v, i)`"),
        ("f: F = A.novo(); printl(f.x); burn f;", "`x` is hidden"),
        ("f: F = A.novo(); print(f); burn f;", "cannot be printed"),
        ("f: F = A.novo();", "`f` is never burned"),
        ("printl(A.novo().n());", "would be lost"),
        ("fs[]: F = claim 2; burn fs;", "`claim` would make empty `F` elements"),
        ("g: F; printl(g.n());", "`g` has no value yet"),
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
    public proc nova() -> Velha { return Velha { x: 1 }; }
    @deprecated("use ver")
    public proc olhar(lent self) -> int { return self.ver(); }
    public proc ver(lent self) -> int { return self.x; }
}

@test
proc passa() { assert(somar(2, 3) == 5); }

@test
proc falha() { assert(somar(2, 2) == 5, "conta errada"); }

proc main() {
    printl(soma(1, 2));
    v: Velha = Velha.nova();
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
        ("class A { x: int; @test public proc t(lent self) { } } proc main() { }", "unknown annotation `@test` on a method"),
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
    public proc novo(v: int) -> No { f[]: No = {}; return No { valor: v, filhos: give f }; }
    public proc add(lent self, own n: No) { append(self.filhos, give n); }
    public proc soma(lent self) -> int {
        s: int = self.valor;
        for i: int = 0; i < len(self.filhos); i++ { s += self.filhos[i].soma(); }
        return s;
    }
}
proc main() {
    r: No = No.novo(1);
    a: No = No.novo(2);
    a.add(No.novo(4));
    r.add(give a);
    r.add(No.novo(3));
    printl(r.soma());
    burn r;
    dump("fim");
}
"#;
    let o = out(tree);
    assert!(o.starts_with("10\n") && !o.contains("heap #"), "{o}");

    // A linked list: `proximo: No?` may be empty and is checked before use.
    let list = r#"interface F { proc nome(lent self) -> text; }
@impl(F) class A { x: int; public proc novo() -> A { return A { x: 1 }; }
    @override public proc nome(lent self) -> text { return "a"; } }

class No {
    public valor: int;
    public proximo: No?;
    public proc novo(v: int) -> No { return No { valor: v, proximo: NaT }; }
    public proc anexar(lent self, own n: No) {
        if isNaT(self.proximo) {
            self.proximo = give n;
            return;
        }
        self.proximo.anexar(give n);
    }
}

proc soma(lent n: No?) -> int {
    if isNaT(n) { return 0; }
    return n.valor + soma(lend n.proximo);
}

proc main() {
    lista: No = No.novo(1);
    lista.anexar(No.novo(2));
    lista.anexar(No.novo(3));
    printl(soma(lend lista));
    if !isNaT(lista.proximo) && lista.proximo.valor == 2 { printl("segundo"); }
    resto: No? = give lista.proximo;
    printl(soma(lend lista), soma(lend resto), isNaT(lista.proximo));
    v: No?;
    printl(isNaT(v));
    v = No.novo(9);
    if isNaT(v) || v.valor == 9 { printl("nove"); }
    f: F? = A.novo();
    g: F?;
    if !isNaT(f) { printl(f.nome(), isNaT(g)); }
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
        public proc novo(v: int) -> No { return No { valor: v, proximo: NaT }; }
        public proc mexer(lent self) { } }
        proc ver(lent n: No) { }";
    let run = |body: &str| run_with(&format!("{decls} proc main() {{ n: No = No.novo(1); {body} burn n; }}"), "").0;
    assert!(run("n.proximo = No.novo(2); n.proximo = No.novo(3);").unwrap_err().reason.contains("`n.proximo` still holds an object"));
    assert!(run("v: No? = No.novo(5); v = No.novo(6); burn v;").unwrap_err().reason.contains("`v` still holds an object"));
    assert!(run("v: No?; burn v; v = No.novo(6); burn v;").is_ok());

    for (body, why) in [
        ("printl(n.proximo.valor);", "it may be empty"),
        ("if !isNaT(n.proximo) { n.mexer(); printl(n.proximo.valor); }", "it may be empty"),
        ("if !isNaT(n.proximo) { while true { printl(n.proximo.valor); n.mexer(); } }", "it may be empty"),
        ("if !isNaT(n.proximo) { x: No = give n.proximo; printl(n.proximo.valor); burn x; }", "it may be empty"),
        ("v: No?; x: No = give v; burn x;", "it may be empty"),
        ("v: No?; ver(lend v); burn v;", "it may be empty"),
        ("x: int = give n.valor;", "only a field that may be empty can be given away"),
        ("v: No?;", "`v` is never burned"),
        ("x: int? = 1;", "`?` is for classes and interfaces"),
    ] {
        let e = ksc::compile(&format!("{decls} proc main() {{ n: No = No.novo(1); {body} burn n; }}")).unwrap_err();
        assert!(e.message.contains(why), "{body}: {}", e.message);
    }
    // Checked, then nothing that could empty it: fine.
    assert!(ksc::compile(&format!(
        "{decls} proc main() {{ n: No = No.novo(1); if !isNaT(n.proximo) {{ n.proximo.mexer(); ver(lend n.proximo); }} burn n; }}"
    ))
    .is_ok());
}
