//! The bugs found in the single-file prototype (kestrel3.rs), rewritten in
//! the current syntax. Each one must now be rejected at compile time.

use ksc::{lexer::lex, linear, parser::parse, sema};

/// Runs every front-end phase and returns the first error message.
fn compile_error(src: &str) -> String {
    let result = lex(src)
        .and_then(parse)
        .and_then(|p| sema::check(&p))
        .and_then(|h| linear::check(&h));
    match result {
        Ok(()) => panic!("this program should be rejected:\n{src}"),
        Err(d) => d.message,
    }
}

const TAKE: &str = "proc take(b: own [word]) { burn b; }\n";

#[test]
fn bug1_copy_creates_two_owners() {
    // Prototype: `let !b = a;` then `purge(a); purge(b);` compiled (double free).
    let e = compile_error("proc main() { a: own [word] = claim 3; b: own [word] = a; burn a; burn b; }");
    assert!(e.contains("cannot be used as a value"), "{e}");
}

#[test]
fn bug2_claim_into_plain_variable() {
    // Prototype: `let x = alloc(3);` leaked silently.
    assert!(compile_error("proc main() { x := claim 3; }").contains("needs to know the buffer type"));
    assert!(compile_error("proc main() { x: own [word] = claim 3; }").contains("never burned"));
}

#[test]
fn bug3_own_parameter_without_give() {
    // Prototype: `eat(a)` without `!` let the callee free the caller's buffer.
    let e = compile_error(&format!("{TAKE}proc main() {{ a: own [word] = claim 3; take(a); burn a; }}"));
    assert!(e.contains("takes ownership"), "{e}");
}

#[test]
fn bug4_overwrite_live_owner() {
    // Prototype: `a = alloc(5);` while `a` was alive leaked the first buffer.
    let e = compile_error("proc main() { a: own [word] = claim 3; a = claim 5; burn a; }");
    assert!(e.contains("would leak"), "{e}");
}

#[test]
fn bug5_tri_arm_leaks_when_another_diverges() {
    // Prototype: with one diverging arm, only the first surviving arm was checked.
    let e = compile_error(
        "proc main() { a: own [word] = claim 3; t: trit = -;
           tri t { - => { burn a; yield; } 0 => burn a, + => {} } }",
    );
    assert!(e.contains("different states"), "{e}");
}

#[test]
fn bug8_wrong_number_of_arguments() {
    // Prototype: `f(1, 2, 3)` for `fn f(x)` panicked inside the VM.
    let e = compile_error("proc f(x: word) -> word { yield x; } proc main() { y := f(1, 2, 3); }");
    assert!(e.contains("takes 1 argument, found 3"), "{e}");
}

#[test]
fn bug9_poison_guard_that_falls_through() {
    // Prototype: a guard whose else did not diverge produced NaT anyway.
    let e = compile_error("proc main() { x := 1 / 0 on NaT { print(\"oops\"); }; show(x); }");
    assert!(e.contains("must end with `yield`"), "{e}");
}

// Bug 6 (inner `let` overwrote the outer variable) and bug 7 (shift
// overflow wrapped around) are runtime behaviors: bug 7 is covered by the
// kestrel-core word tests, bug 6 will be covered once code generation exists.
