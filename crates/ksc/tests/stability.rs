//! Stability tests with generated input: the compiler and the VM must never
//! panic, arithmetic must match an independent reference, and number formats
//! must round-trip. Everything uses a fixed seed, so failures are repeatable.

use std::panic::{AssertUnwindSafe, catch_unwind};

use kestrel_core::bet::{parse_hepta, parse_trits};
use kestrel_core::word::WORD_MAX;
use kestrel_core::{Module, Word, Word27};
use ksvm::Vm;

/// `src` with a `use` for each file of the standard library it names
/// (`Console.printl` needs `use std.io.Console;`), on its first line.
fn with_uses(src: &str) -> String {
    let files = [
        ("Console", "std.io.Console"),
        ("Files", "std.io.Files"),
        ("Process", "std.os.Process"),
        ("Texts", "std.text.Texts"),
        ("Chars", "std.text.Chars"),
        ("Debug", "std.debug.Debug"),
    ];
    let mut uses = String::new();
    // The seeds are joined token by token: `Console . printl`.
    let flat = src.replace(" .", ".").replace(". ", ".").replace(" ;", ";");
    for (alias, path) in files {
        let named = flat.match_indices(&format!("{alias}.")).any(|(at, _)| {
            !flat[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
        });
        if named && !flat.contains(&format!("use {path};")) {
            uses.push_str(&format!("use {path}; "));
        }
    }
    uses + src
}

fn compile(src: &str) -> Result<kestrel_core::Module, ksc::diag::Diagnostic> {
    ksc::compile(&with_uses(src))
}


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
    include_str!("tour.kstl"),
    r#"enum Cor { Vermelho, Verde, Azul }
proc nome(c: Cor) -> text { match c { Cor.Vermelho => return "r"; Cor.Verde => return "g"; Cor.Azul => return "b"; } }
proc main() {
    a[]: int = {3, 1, 2};
    sym s[]: int = {-8, 0, 8};
    for i: int = 0; i < len(a); i++ { if a[i] > 1 && s[0] == 0 { Console.printf("%d ", a[i]); } else { continue; } }
    walk i in span 1 { s[i] += i * 2; }
    t: text = "Kes" + "trel"; Console.printl(Texts.sub(t, 0, 3) + Texts.toText(len(t)));
    n: int = 0; while n < 5 { n++; if n == 4 { break; } }
    Console.printf("{n} %s %t %v\n", nome(Cor.Azul), n, n);
    x: int = Texts.toInt("12") on NaT { burn a; burn s; return; };
    r: real = (real) x / 3.0;
    Console.printl(r);
    burn a; burn s;
}"#,
    r#"proc fib(n: int) -> int { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }
proc fill(v[]: int) -> int[] { for i: int = 0; i < len(v); i++ { v[i] = fib(i); } return v; }
proc soma(&v[]: int) -> int { s: int = 0; walk i in 0..len(v) - 1 { s += v[i]; } return s; }
proc main() -> int {
    b[]: int = claim 8;
    c[]: int = fill(give b);
    tri soma(&c) <=> 33 { - => Console.print("<"); 0 => Console.print("="); + => Console.print(">"); }
    final k: tryte = 'A';
    match k { 'A' | 'B' => Console.printl(-k); else => Console.printl(k); }
    burn c;
    return 0;
}"#,
    r#"struct Ponto { x: int; y: int; }
struct Linha { a: Ponto; b: Ponto; nome: text; }
proc meio(l: Linha) -> Ponto { return Ponto { x: (l.a.x + l.b.x) / 2, y: (l.a.y + l.b.y) / 2 }; }
proc main() {
    l: Linha = Linha { a: Ponto { x: 0, y: 0 }, b: Ponto { x: 4, y: 8 }, nome: "d" };
    m: Ponto = meio(l); l.b.y -= 2; k: Linha = l;
    Console.printl(m); Console.printf("{l} %d %s\n", meio(k).y, l);
    ps[]: Ponto = {m, Ponto { x: 1, y: 2 }}; ps[1].y += ps[0].x;
    ns[]: text = claim 2; ns[0] = l.nome; rs[]: real = {1.5, 2.5};
    Console.printl(ps[1]); Console.printl(ns); Console.printl(rs);
    append(ps, m); insert(rs, 1, 9.0); remove(ns, 0); replace(rs, 0..0, 2..2);
    q: Ponto = pop(ps); resize(ns, 3); Console.printl(q, len(ps), rs, ns); clear(ns);
    burn ps; burn ns; burn rs;
}"#,
    r#"class Pilha {
    itens[]: int;
    public topo: int;
    public proc Pilha() -> Pilha { return Pilha { itens: {}, topo: 0 }; }
    public proc por(&self, x: int) { append(self.itens, x); self.topo = x; }
    public proc tirar(&self) -> int { return pop(self.itens); }
    public proc vazia(&self) -> bool { return len(self.itens) == 0; }
    public proc fim(self) -> int { n: int = len(self.itens); burn self; return n; }
}
class Dupla { a: Pilha; b: Pilha;
    public proc Dupla() -> Dupla { x: Pilha = Pilha(); y: Pilha = Pilha(); return Dupla { a: give x, b: give y }; }
    public proc mover(&self) { while !self.a.vazia() { self.b.por(self.a.tirar()); } }
    public proc encher(&self, n: int) { for i: int = 0; i < n; i++ { self.a.por(i * 3); } }
}
proc main() {
    d: Dupla = Dupla(); d.encher(4); d.mover(); Console.printl(d);
    p: Pilha = Pilha(); p.por(7); Console.printf("{p.topo} %s\n", p.vazia()); k: int = p.fim(); Console.printl(k);
    burn d;
}"#,
];

/// Pieces a mutation may insert.
const VOCAB: &[&str] = &[
    "proc", "return", "if", "else", "while", "for", "walk", "in", "span", "tri", "match", "burn", "give", "lend", "claim",
    "sym", "final", "own", "lent", "int", "tryte", "trit", "real", "text", "enum", "NaT", "on", "break", "continue",
    "{", "}", "(", ")", "[", "]", ";", ",", ".", "..", ":", "=", "==", "!=", "<", ">", "<=", ">=", "<=>", "+",
    "-", "*", "/", "%", "**", "<<", ">>", "&&", "||", "!", "|", "=>", "->", "++", "--", "+=", "0", "1", "-1", "364",
    "3812798742493", "0t+-0", "0vM", "1.5", "'a'", "\"s\"", "x", "a", "i", "main", "print", "printf", "len", "sub",
    "toText", "toInt", "exit", "bool", "true", "false", "class", "public", "self", "&self", "self", "Pilha", ".", "struct", "Ponto", "x:", "append", "pop", "insert", "remove", "resize", "clear", "replace",
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
        compile(&b.join(" ")).unwrap_or_else(|d| panic!("seed {i} does not compile once re-joined: {}", d.message));
    }
    let (mut compiled, mut crashes) = (0, Vec::new());
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // failures are collected below instead
    for _ in 0..count {
        let base = &bases[rng.below(bases.len())];
        let src = mutate(&mut rng, base);
        let result = catch_unwind(AssertUnwindSafe(|| compile(&src)));
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
            .map(|(e, _)| format!("walk k in 0..0 {{ x: int = {e} on NaT {{ Console.printl(\"NaT\"); continue; }}; Console.printl(x); }}\n"))
            .collect();
        let src = format!("proc main() {{\n{body}}}");
        let module = compile(&src).unwrap_or_else(|d| panic!("batch {batch}: {}\n{src}", d.message));
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
