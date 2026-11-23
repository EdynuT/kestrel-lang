//! Exercises the Kestrel lexer using the bootstrap compiler and VM.

use std::collections::HashMap;
use std::fmt::Write;

fn compile_lexer_program(source: &str) -> kestrel_core::Module {
    let files = HashMap::from([
        (
            "lexer.Lexer".to_string(),
            include_str!("../../../compiler/src/lexer/Lexer.ks").to_string(),
        ),
        (
            "lexer.Token".to_string(),
            include_str!("../../../compiler/src/lexer/Token.ks").to_string(),
        ),
        (
            "lexer.TokenKind".to_string(),
            include_str!("../../../compiler/src/lexer/TokenKind.ks").to_string(),
        ),
    ]);
    let (map, result) = ksc::compile_program("Main.ks", source, &files, true);
    result
        .unwrap_or_else(|d| panic!("{}", map.render(&d)))
        .module
}

#[test]
fn numbers_and_dots() {
    let cases: &[(&str, &[(&str, usize, usize)])] = &[
        ("", &[("EndOfFile", 0, 0)]),
        ("42", &[("Number", 0, 2), ("EndOfFile", 2, 2)]),
        ("12.34", &[("RealNumber", 0, 5), ("EndOfFile", 5, 5)]),
        (
            "-0.5",
            &[("Minus", 0, 1), ("RealNumber", 1, 4), ("EndOfFile", 4, 4)],
        ),
        (
            "1.5;",
            &[
                ("RealNumber", 0, 3),
                ("Semicolon", 3, 4),
                ("EndOfFile", 4, 4),
            ],
        ),
        (".", &[("Dot", 0, 1), ("EndOfFile", 1, 1)]),
        ("..", &[("DotDot", 0, 2), ("EndOfFile", 2, 2)]),
        (
            "1..5",
            &[
                ("Number", 0, 1),
                ("DotDot", 1, 3),
                ("Number", 3, 4),
                ("EndOfFile", 4, 4),
            ],
        ),
        (
            ".5",
            &[("Dot", 0, 1), ("Number", 1, 2), ("EndOfFile", 2, 2)],
        ),
        (
            "1.",
            &[("Number", 0, 1), ("Dot", 1, 2), ("EndOfFile", 2, 2)],
        ),
        (
            "1.foo",
            &[
                ("Number", 0, 1),
                ("Dot", 1, 2),
                ("Identifier", 2, 5),
                ("EndOfFile", 5, 5),
            ],
        ),
        (
            "1.2.3",
            &[
                ("RealNumber", 0, 3),
                ("Dot", 3, 4),
                ("Number", 4, 5),
                ("EndOfFile", 5, 5),
            ],
        ),
        (
            "7.8 9",
            &[("RealNumber", 0, 3), ("Number", 4, 5), ("EndOfFile", 5, 5)],
        ),
        (
            "0t+0- 0vAm 42",
            &[
                ("Number", 0, 5),
                ("Number", 6, 10),
                ("Number", 11, 13),
                ("EndOfFile", 13, 13),
            ],
        ),
        ("0t+?", &[("Number", 0, 4), ("EndOfFile", 4, 4)]),
        ("0v0?Am", &[("Number", 0, 6), ("EndOfFile", 6, 6)]),
        ("0vaM", &[("Number", 0, 4), ("EndOfFile", 4, 4)]),
        (
            "0t+- - 1",
            &[
                ("Number", 0, 4),
                ("Minus", 5, 6),
                ("Number", 7, 8),
                ("EndOfFile", 8, 8),
            ],
        ),
        (
            "0vM+0t0",
            &[
                ("Number", 0, 3),
                ("Plus", 3, 4),
                ("Number", 4, 7),
                ("EndOfFile", 7, 7),
            ],
        ),
        (
            "0t000000000000000000000000000",
            &[("Number", 0, 29), ("EndOfFile", 29, 29)],
        ),
        ("0vABCDEFGHI", &[("Number", 0, 11), ("EndOfFile", 11, 11)]),
    ];

    let mut source = String::from("use lexer.Lexer; use lexer.TokenKind;\nproc main() {\n");
    for (case, (input, tokens)) in cases.iter().enumerate() {
        writeln!(
            source,
            "l{case}: Lexer = Lexer(\"{input}\"); l{case}.run();"
        )
        .unwrap();
        writeln!(source, "assert(len(l{case}.tokens) == {});", tokens.len()).unwrap();
        for (index, (kind, start, end)) in tokens.iter().enumerate() {
            writeln!(
                source,
                "assert(l{case}.tokens[{index}].kind == TokenKind.{kind}); \
                 assert(l{case}.tokens[{index}].start == {start}); \
                 assert(l{case}.tokens[{index}].end == {end});"
            )
            .unwrap();
        }
        writeln!(source, "burn l{case};").unwrap();
    }
    source.push_str("}\n");

    let module = compile_lexer_program(&source);
    let result = ksvm::Vm::new(&module, &b""[..], Vec::new())
        .run()
        .expect("lexer test program failed");
    assert_eq!(ksvm::exit_code(Ok(result)), 0);
}

#[test]
fn invalid_ternary_literals_report_errors() {
    let module = compile_lexer_program(
        "use lexer.Lexer; use std.os.Process;\n\
         proc main() { l: Lexer = Lexer(Process.arg(0)); l.run(); burn l; }",
    );
    let cases = [
        ("0t", "number literal has no digits"),
        ("0v", "number literal has no digits"),
        ("0t2", "number literal has no digits"),
        ("0t+x", "invalid character right after a number"),
        ("0t+_", "invalid character right after a number"),
        ("0vN", "invalid digit in 0v literal"),
        ("0vn", "invalid digit in 0v literal"),
        ("0v9", "invalid digit in 0v literal"),
        ("0vÁ", "invalid digit in 0v literal"),
        ("0vA_", "invalid character right after a number"),
        (
            "0t0000000000000000000000000000",
            "number literal has too many digits",
        ),
        ("0vABCDEFGHIJ", "number literal has too many digits"),
    ];
    for (input, message) in cases {
        let mut printed = Vec::new();
        let result = ksvm::Vm::new(&module, &b""[..], &mut printed)
            .with_args(vec![format!("  {input}")])
            .run()
            .unwrap_or_else(|trap| panic!("{input:?} trapped: {trap}"));
        assert_eq!(ksvm::exit_code(Ok(result)), 1, "{input:?}");
        assert_eq!(
            String::from_utf8(printed).unwrap(),
            format!("error: {message} at 2\n"),
            "{input:?}"
        );
    }
}
