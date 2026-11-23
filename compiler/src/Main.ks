// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/* 
 * Main entry point for the program. 
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

proc main() {
    if !Files.exists(Process.arg(0)) {
        Console.printf("File does not exist: %s\n", Process.arg(0));
        Process.exit(1);
    }
    source: text = Files.read(Process.arg(0));
    lexer: Lexer = Lexer(source);
    lexer.run();

    for i: int = 0; i < len(lexer.tokens); i++ {
        Console.printl(padLeft(Texts.toText(lexer.tokens[i].start), 5) + ".." + padRight(Texts.toText(lexer.tokens[i].end), 5) + " " + describe(lexer.tokens[i], source));
    }
    burn lexer;
}

proc describe(token: Token, source: text) -> text {
    word: text = Texts.sub(source, token.start, token.end);
    match token.kind {
        TokenKind.EndOfFile => return "end of file";
        TokenKind.Number => return "number `" + Texts.toText(Texts.toInt(word) ?: 0) + "`";
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
