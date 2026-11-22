//! Programs compiled to machine code (`ksc::native`) give what the
//! interpreter gives: the same output and exit code.

use std::process::Command;

use ksvm::Vm;

const TOUR: &str = include_str!("../../ksc/tests/tour.ks");

const ERRORS: &str = r#"
@impl(Error)
class NotFound {
    path: text;
    public proc NotFound(p: text) -> NotFound { return NotFound { path: p }; }
    @override
    public proc message(&self) -> text { return "not found: " + self.path; }
}
proc load(p: text) -> int throws NotFound {
    a[]: int = {1, 2};
    if p == "x" { throw NotFound(p); }
    burn a;
    return 7;
}
proc divide(a: int, b: int) -> int { return a / b; }
proc deep(n: int) -> int throws NotFound { if n == 0 { return load("x"); } return deep(n - 1) + 1; }
proc main() -> int {
    try { printl(deep(50)); } catch e: NotFound { printl(e.message()); burn e; }
    try { printl(divide(1, 0)); } catch e: Trap { printl(e.message()); burn e; }
    walk i in 1..3 {
        try { printl(load("ok") + i); printl(load("x")); } catch e: Error { printl("caught " + toText(i)); burn e; }
    }
    printl(argCount());
    return 42;
}
"#;

/// Stdout and exit code: by the interpreter, then by the native executable.
fn both(name: &str, src: &str) -> ((String, i32), (String, i32)) {
    let module = ksc::compile(src).unwrap_or_else(|d| panic!("{}", d.render("test.ks", src)));
    let mut out = Vec::new();
    let r = Vm::new(&module, "".as_bytes(), &mut out).with_args(vec!["a".into(), "b".into()]).run();
    let vm = (String::from_utf8(out).unwrap(), i32::from(ksvm::exit_code(r)));
    let runtime = std::fs::read(env!("CARGO_BIN_EXE_ksvm")).unwrap();
    let exe = ksc::native::build(&module, &runtime).unwrap();
    // One at a time: a file being written while another test starts a
    // program can't be run ("text file busy").
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::env::temp_dir().join(format!("kestrel-native-{name}-{}", std::process::id()));
    std::fs::write(&path, exe).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let o = Command::new(&path).args(["a", "b"]).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    (vm, (String::from_utf8(o.stdout).unwrap(), o.status.code().unwrap_or(-1)))
}

#[test]
fn tour_runs_natively() {
    let (vm, native) = both("tour", TOUR);
    assert_eq!(vm, native);
}

#[test]
fn errors_and_traps_natively() {
    let (vm, native) = both("errors", ERRORS);
    assert_eq!(vm, native);
    assert_eq!(native.1, 42);
    assert!(native.0.starts_with("not found: x\ndivision by zero"), "{}", native.0);
    // Uncaught, a trap stops the program with code 101.
    let (vm, native) = both("trap", "proc f(a: int) -> int { return 10 / a; } proc main() { printl(1); printl(f(0)); }");
    assert_eq!(vm, native);
    assert_eq!(native, ("1\n".to_string(), 101));
}

/// The cases where the machine code gives way to the runtime: NaT,
/// overflow, reals, texts, empty objects, a stack that has to grow.
const EDGES: &str = r#"
class Node { public v: int; public next: Node?; public proc Node(v: int) -> Node { return Node { v: v, next: null }; } }
proc depth(n: int) -> int { if n == 0 { return 0; } return depth(n - 1) + 1; }
proc add(x: int, y: int) -> int { r: int = x + y on NaT { return -1; }; return r; }
proc mul(x: int, y: int) -> int { r: int = x * y on NaT { return -2; }; return r; }
proc main() {
    big: int = 3812798742493;
    printl(add(big, 1) + mul(big, big) + add(-big, -1) + add(2, 3) + mul(4, 5));
    walk i in -2..2 {
        walk j in -1..1 {
            printf("{i < j}{i <= j}{i > j}{i >= j}{i == j}{i != j}{i <=> j} {-i} {i * j - i} ");
        }
    }
    printl();
    x: real = 1.5;
    y: real = x * 2.0 - 0.5;
    printl(y < x);
    printl(-y);
    t: text = "ab";
    printl(t + "c" == "abc");
    n: Node? = null;
    printl(n == null);
    n = Node(3);
    printl(n.v);
    burn n;
    m: int? = null;
    printl(m ?: 7);
    printl(depth(3000));
    printl(big + 1);
}
"#;

#[test]
fn edges_natively() {
    let (vm, native) = both("edges", EDGES);
    assert_eq!(vm, native);
    assert_eq!(native.1, 101, "{}", native.0); // the last line overflows
}

/// Values the machine code keeps back instead of pushing: a variable read
/// before it changes, long expressions, NaT and objects in the middle.
const DEFERRED: &str = r#"
proc half(x: int) -> int { r: int = x / 0 on NaT { return x % 3; }; return r; }
proc main() {
    a: int = 1; b: int = 2;
    t: int = a; a = b; b = t;
    printl(a * 10 + b);
    a = a;
    c: int = ((a + 1) * (b + 2) - (a - b) * 3) / 2 % 5 + (a + b + 1 + 2 + 3 + 4 + 5 + 6 + 7);
    printl(c);
    n: int? = null;
    printl(n ?: a + b);
    m: int? = 5;
    printl((m ?: 0) + a);
    printl(half(7) + half(-7));
    printl(-7 / 2 + -7 % 2 * 10);
    v[]: int = {4, 5, 6};
    w[]: int = give v;
    printl(len(w) + w[1] * a);
    burn w;
    s: int = 0;
    walk i in 1..5 { if i % 2 == 0 { s = s + i * i; } else { s = s - i; } }
    printl(s);
    printl(1 / (a - 2));
}
"#;

#[test]
fn deferred_values_natively() {
    let (vm, native) = both("deferred", DEFERRED);
    assert_eq!(vm, native);
    assert_eq!(native.1, 101, "{}", native.0); // the last line divides by zero
}

/// Arrays read and written by machine code: symmetric ones, ones that grow
/// (and move), texts, structs, and an index out of bounds.
const ARRAYS: &str = r#"
struct P { x: int; y: int; }
proc sum(&v[]: int) -> int { s: int = 0; walk i in 0..len(v) - 1 { s = s + v[i]; } return s; }
proc main() {
    sym s[]: int = claim 5;
    walk i in -2..2 { s[i] = i * i; }
    printl(s[-2] + s[2] + s[0]);
    v[]: int = {};
    walk i in 1..100 { append(v, i); v[len(v) - 1] = v[len(v) - 1] * 2; }
    printl(sum(&v));
    printl(len(v));
    t[]: text = {"a", "b"};
    t[1] = t[0] + "c";
    printl(t[1] + toText(len(t[1])));
    ps[]: P = {P { x: 1, y: 2 }, P { x: 3, y: 4 }};
    printl(ps[1].x + ps[0].y);
    q: int = remove(v, 0);
    printl(q + v[0]);
    burn s; burn t; burn ps;
    printl(v[100]);
    burn v;
}
"#;

#[test]
fn arrays_natively() {
    let (vm, native) = both("arrays", ARRAYS);
    assert_eq!(vm, native);
    assert_eq!(native.1, 101, "{}", native.0); // the last line is out of bounds
    assert!(native.0.starts_with("8\n10100\n100\nac2\n5\n6\n"), "{}", native.0);
}

/// Fields of objects and structs, read and written by machine code.
const OBJECTS: &str = r#"
struct P { x: int; y: int; }
interface Shape { proc area(&self) -> int; }
@impl(Shape)
class Rect {
    public w: int;
    public h: int;
    public corner: P;
    public proc Rect(w: int, h: int) -> Rect { return Rect { w: w, h: h, corner: P { x: w, y: h } }; }
    @override public proc area(&self) -> int { return self.w * self.h; }
    public proc grow(&self) { self.w = self.w + 1; self.corner = P { x: self.corner.x * 2, y: self.corner.y + 1 }; }
}
class Node { public v: int; public next: Node?; public proc Node(v: int) -> Node { return Node { v: v, next: null }; } }
proc main() {
    r: Rect = Rect(2, 3);
    walk i in 1..3 { r.grow(); }
    printl(r.w + r.h + r.corner.x + r.corner.y);
    s: Shape = give r;
    printl(s.area());
    ps[]: P = {P { x: 1, y: 2 }, P { x: 3, y: 4 }};
    ps[0] = P { x: ps[1].y, y: ps[0].x };
    ps[1].x = 9;
    printl(ps[0].x * 100 + ps[0].y * 10 + ps[1].x);
    head: Node? = null;
    walk i in 1..5 { n: Node = Node(i); n.next = give head; head = give n; }
    total: int = 0;
    cur: &Node? = &head;
    while cur != null { total = total * 10 + cur.v; cur = &cur.next; }
    printl(total);
    x: real? = null;
    printl(head == null || x != null);
    burn s; burn ps; burn head;
}
"#;

#[test]
fn objects_natively() {
    let (vm, native) = both("objects", OBJECTS);
    assert_eq!(vm, native);
    assert_eq!(native.0, "30\n15\n419\n54321\nfalse\n");
}
