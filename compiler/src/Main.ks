// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * The compiler's entry point: `ksc run File.ks` reads the file and
 * prints its tokens, one per line, in the same format as
 * `ksc File.ks --tokens`.
 *
 * A `Diagnostic` from the lexer is printed as `error: ...` and the
 * program exits with status 1.
 *
 * @since 0.8
 * @author Edynu
 */

use std.io.Files;
use std.io.Console;
use std.os.Process;
use std.text.Texts;

use lexer.Lexer;
use lexer.Token;
use lexer.TokenKind;
use diag.Diagnostic;
use diag.Reporter;

proc main() {
    if !Files.exists(Process.arg(0)) {
        Console.printf("File does not exist: %s\n", Process.arg(0));
        Process.exit(1);
    }
    source: text = Files.read(Process.arg(0));
    lexer: Lexer = Lexer(source);
    try {
        lexer.run();
    } catch error: Diagnostic {
        reporter: Reporter = Reporter(Process.arg(0), source);
        reporter.report(&error);
        burn reporter;
        burn error;
        Process.exit(1);
    } catch failure: Trap {
        // A character outside the tryte table (such as `\r`) cannot be read.
        Console.printl("error: " + failure.message());
        burn failure;
        Process.exit(1);
    }

    for i: int = 0; i < lexer.tokens.len(); i++ {
        token: Token = lexer.tokens.get(i);
        Console.printl(padLeft(Texts.toText(token.start), 5) + ".." + padRight(Texts.toText(token.end), 5) + " " + describe(token, source));
    }
    burn lexer;
}

proc describe(token: Token, source: text) -> text {
    word: text = Texts.sub(source, token.start, token.end);
    match token.kind {
        TokenKind.EndOfFile => return "end of file";
        TokenKind.Number => {
            // A `?` digit (`0t+?`) makes the number NaT.
            if Texts.indexOf(word, "?") >= 0 {
                return "number `NaT`";
            }
            return "number `" + Texts.toText(Texts.toInt(word) ?: 0) + "`";
        }
        TokenKind.RealNumber => return "real `" + Texts.toText(Texts.toReal(word) ?: 0.0) + "`";
        TokenKind.String => return "string literal";
        TokenKind.Character => return "character literal";
        // Names, keywords and symbols: their own text.
        else => return "`" + word + "`";
    }
}

proc padLeft(value: text, width: int) -> text {
    length: int = len(value);
    if length >= width {
        return value;
    }
    padding: text = "";
    while len(padding) < width - length {
        padding = padding + " ";
    }
    return padding + value;
}

proc padRight(value: text, width: int) -> text {
    length: int = len(value);
    if length >= width {
        return value;
    }
    padding: text = "";
    while len(padding) < width - length {
        padding = padding + " ";
    }
    return value + padding;
}
