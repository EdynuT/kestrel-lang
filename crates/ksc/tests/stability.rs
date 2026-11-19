//! Stability tests with generated input: the compiler and the VM must never
//! panic, arithmetic must match an independent reference, and number formats
//! must round-trip. Everything uses a fixed seed, so failures are repeatable.

use std::panic::{AssertUnwindSafe, catch_unwind};

use kestrel_core::bet::{parse_hepta, parse_trits};
use kestrel_core::word::WORD_MAX;
use kestrel_core::{Module, Word, Word27};
use ksvm::Vm;

/// xorshift64*: small, fast and good enough to drive tests.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

// ---- 1. mutation fuzzing: no panics, ever ----

/// Valid programs covering most of the language (no file writes).
const SEEDS: &[&str] = &[
    include_str!("../../../examples/tour.ks"),
    r#"enum Cor { Vermelho, Verde, Azul }
proc nome(c: Cor) -> text { match c { Cor.Vermelho => yield "r"; Cor.Verde => yield "g"; Cor.Azul => yield "b"; } }
proc main() {
    a[]: int = {3, 1, 2};
    sym s[]: int = {-8, 0, 8};
    for i: int = 0; i < len(a); i++ { if a[i] > 1 && s[0] == 0 { printf("%d ", a[i]); } else { continue; } }
    walk i in span 1 { s[i] += i * 2; }
    t := "Kes" + "trel"; printl(sub(t, 0, 3) + to_text(len(t)));
    n := 0; while n < 5 { n++; if n == 4 { break; } }
    printf("{n} %s %t %v\n", nome(Cor.Azul), n, n);
    x := to_int("12") on NaT { burn a; burn s; yield; };
    r := (real) x / 3.0;
    printl(r);
    burn a; burn s;
}"#,
    r#"proc fib(n: int) -> int { if n < 2 { yield n; } yield fib(n - 1) + fib(n - 2); }
proc fill(own v[]: int) -> int[] { for i: int = 0; i < len(v); i++ { v[i] = fib(i); } yield v; }
proc soma(lent v[]: int) -> int { s := 0; walk i in 0..len(v) - 1 { s += v[i]; } yield s; }
proc main() -> int {
    b[]: int = claim 8;
    c[]: int = fill(give b);
    tri soma(lend c) <=> 33 { - => print("<"); 0 => print("="); + => print(">"); }
    final k: tryte = 'A';
    match k { 'A' | 'B' => printl(-k); else => printl(k); }
    burn c;
    yield 0;
}"#,
];

/// Pieces a mutation may insert.
const VOCAB: &[&str] = &[
    "proc", "yield", "if", "else", "while", "for", "walk", "in", "span", "tri", "match", "burn", "give", "lend", "claim",
    "sym", "final", "own", "lent", "int", "tryte", "trit", "real", "text", "enum", "NaT", "on", "break", "continue",
    "{", "}", "(", ")", "[", "]", ";", ",", ".", "..", ":", ":=", "=", "==", "!=", "<", ">", "<=", ">=", "<=>", "+",
    "-", "*", "/", "%", "**", "<<", ">>", "&&", "||", "!", "|", "=>", "->", "++", "--", "+=", "0", "1", "-1", "364",
    "3812798742493", "0t+-0", "0vM", "1.5", "'a'", "\"s\"", "x", "a", "i", "main", "print", "printf", "len", "sub",
    "to_text", "to_int", "exit",
];

/// The source text of each token, using the real lexer (comments are dropped).
fn pieces(src: &str) -> Vec<String> {
    let tokens = ksc::lexer::lex(src).expect("seed programs lex");
    tokens.iter().filter(|t| t.span.end > t.span.start).map(|t| src[t.span.start..t.span.end].to_string()).collect()
}

fn mutate(rng: &mut Rng, base: &[String]) -> String {
    let mut p = base.to_vec();
    for _ in 0..1 + rng.below(2) {
        if p.is_empty() {
            break;
        }
        let i = rng.below(p.len());
        // Swapping a number for another keeps most programs valid, so the
        // back end and the VM see odd values (0, negatives, NaT, limits).
        if p[i].chars().all(|c| c.is_ascii_digit()) && rng.below(2) == 0 {
            p[i] = ["0", "1", "2", "3", "7", "364", "365", "-1", "-364", "NaT", "3812798742493"][rng.below(11)].to_string();
            continue;
        }
        match rng.below(4) {
            0 => {
                p.remove(i);
            }
            1 => {
                let d = p[i].clone();
                p.insert(i, d);
            }
            2 => {
                let j = rng.below(p.len());
                p.swap(i, j);
            }
            _ => p[i] = VOCAB[rng.below(VOCAB.len())].to_string(),
        }
    }
    p.join(" ")
}

#[test]
fn mutated_programs_never_crash() {
    // KESTREL_FUZZ=<count>[,<seed>] runs a longer or different campaign.
    let cfg = std::env::var("KESTREL_FUZZ").unwrap_or_default();
    let mut cfg = cfg.split(',').map(|x| x.trim().parse::<u64>().ok());
    let count = cfg.next().flatten().unwrap_or(20_000);
    let mut rng = Rng(cfg.next().flatten().unwrap_or(0x4b65_7374_7265_6c21));
    let bases: Vec<Vec<String>> = SEEDS.iter().map(|s| pieces(s)).collect();
    for (i, b) in bases.iter().enumerate() {
        ksc::compile(&b.join(" ")).unwrap_or_else(|d| panic!("seed {i} does not compile once re-joined: {}", d.message));
    }
    let (mut compiled, mut crashes) = (0, Vec::new());
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // failures are collected below instead
    for _ in 0..count {
        let base = &bases[rng.below(bases.len())];
        let src = mutate(&mut rng, base);
        let result = catch_unwind(AssertUnwindSafe(|| ksc::compile(&src)));
        let module = match result {
            Err(_) => {
                crashes.push(("compiler", src));
                continue;
            }
            Ok(Err(_)) => continue,
            Ok(Ok(m)) => m,
        };
        compiled += 1;
        if Module::from_bytes(&module.to_bytes()).is_err() {
            crashes.push(("emitted module does not verify", src));
            continue;
        }
        let ran = catch_unwind(AssertUnwindSafe(|| {
            let mut out = Vec::new();
            let _ = Vm::new(&module, "1\n2\n".as_bytes(), &mut out).with_fuel(50_000).run();
        }));
        if ran.is_err() {
            crashes.push(("vm", src));
        }
    }
    std::panic::set_hook(default_hook);
    eprintln!("fuzz: {count} mutants, {compiled} compiled and ran");
    assert!(crashes.is_empty(), "{} crashes, first: {:?}", crashes.len(), crashes.first());
    assert!(count < 20_000 || compiled > 300, "only {compiled} mutants compiled; the fuzzer is not reaching the back end");
}

// ---- 2. differential arithmetic: Kestrel vs. an i128 reference ----

/// `None` is NaT. Mirrors SPEC §3: overflow and division by zero are NaT,
/// `/` truncates toward zero, `%` takes the dividend's sign.
fn checked(v: i128) -> Option<i128> {
    (v.abs() <= i128::from(WORD_MAX)).then_some(v)
}

fn random_expr(rng: &mut Rng, depth: u32) -> (String, Option<i128>) {
    if depth == 0 || rng.below(4) == 0 {
        let v = match rng.below(6) {
            0 => 0,
            1 => rng.range(-WORD_MAX / 2, WORD_MAX / 2),
            2 => rng.range(-3, 3),
            _ => rng.range(-2000, 2000),
        };
        return (format!("({v})"), Some(i128::from(v)));
    }
    let (a, va) = random_expr(rng, depth - 1);
    if rng.below(8) == 0 {
        return (format!("(-{a})"), va.map(|v| -v));
    }
    let op = ["+", "-", "*", "/", "%", "**"][rng.below(6)];
    if op == "**" {
        let e = rng.range(-1, 9);
        let v = match (va, e) {
            (_, e) if e < 0 => None,
            (Some(b), e) => (0..e).try_fold(1i128, |acc, _| checked(acc * b)),
            (None, _) => None,
        };
        return (format!("({a} ** ({e}))"), v);
    }
    let (b, vb) = random_expr(rng, depth - 1);
    let v = match (va, vb) {
        (Some(x), Some(y)) => match op {
            "+" => checked(x + y),
            "-" => checked(x - y),
            "*" => checked(x * y),
            "/" if y == 0 => None,
            "/" => checked(x / y),
            "%" if y == 0 => None,
            _ => checked(x % y),
        },
        _ => None,
    };
    (format!("({a} {op} {b})"), v)
}

#[test]
fn arithmetic_matches_reference() {
    let mut rng = Rng(0x7472_6974_7331_3233);
    for batch in 0..40 {
        let cases: Vec<(String, Option<i128>)> = (0..50).map(|_| random_expr(&mut rng, 4)).collect();
        // Each case runs on an `on NaT` line, so a failure prints NaT and
        // `continue` moves on to the next case instead of trapping.
        let body: String = cases
            .iter()
            .map(|(e, _)| format!("walk k in 0..0 {{ x := {e} on NaT {{ printl(\"NaT\"); continue; }}; printl(x); }}\n"))
            .collect();
        let src = format!("proc main() {{\n{body}}}");
        let module = ksc::compile(&src).unwrap_or_else(|d| panic!("batch {batch}: {}\n{src}", d.message));
        let mut out = Vec::new();
        Vm::new(&module, "".as_bytes(), &mut out).run().unwrap_or_else(|t| panic!("batch {batch}: {t}"));
        let out = String::from_utf8(out).unwrap();
        for ((expr, want), got) in cases.iter().zip(out.lines()) {
            let want = want.map_or("NaT".to_string(), |v| v.to_string());
            assert_eq!(got, want, "{expr}");
        }
    }
}

// ---- 3. round trips of number formats ----

#[test]
fn ternary_formats_round_trip() {
    let mut rng = Rng(0x6865_7074_6176_696e);
    for _ in 0..20_000 {
        let v = match rng.below(3) {
            0 => rng.range(-WORD_MAX, WORD_MAX),
            1 => rng.range(-400, 400),
            _ => [WORD_MAX, -WORD_MAX, 0, 1, -1][rng.below(5)],
        };
        let w = Word::new(v);
        let bet = Word27::from(w);
        assert_eq!(Word::from(bet), w, "BET round trip of {v}");
        let trits = bet.to_trits(27);
        assert_eq!(parse_trits(&trits[2..]).ok(), Some(w), "0t round trip of {v}: {trits}");
        let hepta = bet.to_hepta();
        assert_eq!(parse_hepta(&hepta[2..]).ok(), Some(w), "0v round trip of {v}: {hepta}");
        assert_eq!(Word27::from_bits(bet.bits()), Some(bet));
    }
    assert!(Word::from(Word27::NAT).is_nat());
}
