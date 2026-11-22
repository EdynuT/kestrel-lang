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
