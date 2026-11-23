// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Lexer-related definitions for the compiler.
 *
 * @since 0.8
 * @author Edynu
 */

use std.io.Console;
use std.os.Process;
use std.text.Chars;
use std.text.Texts;
use lexer.Token;
use lexer.TokenKind;

public class Lexer {
    source: text;
    position: int;
    public tokens[]: Token;

    public proc Lexer(source: text) -> Lexer {
        empty[]: Token = {};
        return Lexer { source: source, position: 0, tokens: give empty };
    }

    public proc current(&self) -> tryte {
        if self.position < len(self.source) {
            return self.source[self.position];
        }
        return 0;
    }

    /* Whether every character was read. */
    public proc atEnd(&self) -> bool {
        return self.position >= len(self.source);
    }

    /* Cuts the whole source into tokens, ending with `EndOfFile`. */
    public proc run(&self) {
        self.skipBlanks();
        while !self.atEnd() {
            start: int = self.position;
            char: tryte = self.current();
            kind: TokenKind = TokenKind.EndOfFile;
            if Chars.isLetter(char) || char == '_' {
                while !self.atEnd() && (Chars.isLetter(self.current()) || Chars.isDigit(self.current()) || self.current() == '_') {
                    self.position++;
                }
                kind = self.keyword(Texts.sub(self.source, start, self.position));
            } else if Chars.isDigit(char) {
                kind = self.number();
            } else if char == '"' {
                kind = self.string();
            } else if char == '\'' {
                kind = self.character();
            } else{
                kind = self.symbol();                
            }
            append(self.tokens, Token { kind: kind, start: start, end: self.position });
            self.skipBlanks();
        }
        append(self.tokens, Token { kind: TokenKind.EndOfFile, start: self.position, end: self.position });
    }

    /* A text literal: from `"` to the next `"`, skipping `\"`. */
    proc string(&self) -> TokenKind {
        start: int = self.position;
        self.position++;
        while !self.atEnd() && self.current() != '"' {
            if self.current() == '\\' {
                self.position++;
            }
            self.position++;
        }
        if self.atEnd() {
            // For now; a `Diagnostic` will take its place.
            Console.printl("error: unterminated string literal at " + Texts.toText(start));
            Process.exit(1);
        }
        self.position++;
        return TokenKind.String;
    }

    /* A character literal: from `'` to the next `'`, skipping `\'`. */
    proc character(&self) -> TokenKind {
        start: int = self.position;
        self.position++;
        while !self.atEnd() && self.current() != '\'' {
            if self.current() == '\\' {
                self.position++;
            }
            self.position++;
        }
        if self.atEnd() {
            // For now; a `Diagnostic` will take its place.
            Console.printl("error: unterminated character literal at " + Texts.toText(start));
            Process.exit(1);
        }
        self.position++;
        return TokenKind.Character;
    }

    /* A decimal, ternary or heptavintimal integer, or a decimal real. */
    proc number(&self) -> TokenKind {
        if self.current() == '0' && (self.next() == 't' || self.next() == 'v') {
            return self.ternary();
        }
        while !self.atEnd() && Chars.isDigit(self.current()) {
            self.position++;
        }
        if !self.atEnd() && self.current() == '.' && Chars.isDigit(self.next()) {
            self.position++;
            while !self.atEnd() && Chars.isDigit(self.current()) {
                self.position++;
            }
            return TokenKind.RealNumber;
        }
        return TokenKind.Number;
    }

    /* Skips spaces, line breaks and `//` comments. */
    proc skipBlanks(&self) {
        while !self.atEnd() {
            if Chars.isSpace(self.current()) {
                self.position++;
            } else if self.current() == '/' && self.next() == '/' {
                while !self.atEnd() && self.current() != '\n' {
                    self.position++;
                }
            } else if self.current() == '/' && self.next() == '*' {
                start: int = self.position;
                self.position += 2;
                while !self.atEnd() && !(self.current() == '*' && self.next() == '/') {
                    self.position++;
                }
                if !self.atEnd() {
                    self.position += 2;
                } else {
                    Console.printl("error: unterminated block comment at " + Texts.toText(start));
                    Process.exit(1);
                }
            } else {
                return;
            }
        }
    }

    /* A balanced-ternary integer in `0t` or `0v` notation. */
    proc ternary(&self) -> TokenKind {
        start: int = self.position;
        prefix: tryte = self.next();
        self.position += 2;
        if prefix == 't' {
            while !self.atEnd() && (self.current() == '-' || self.current() == '0' || self.current() == '+' || self.current() == '?') {
                self.position++;
            }
        } else {
            while !self.atEnd() && (Chars.isLetter(self.current()) || Chars.isDigit(self.current()) || self.current() == '?') {
                digit: tryte = self.current();
                // Lowercase trytes run from 'a' = -1 down to 'm' = -13.
                if digit != '0' && digit != '?' && !(digit >= 'A' && digit <= 'M') && !(digit >= 'm' && digit <= 'a') {
                    self.numberError("invalid digit in 0v literal", start);
                }
                self.position++;
            }
        }
        digits: int = self.position - start - 2;
        if digits == 0 {
            self.numberError("number literal has no digits", start);
        }
        if (prefix == 't' && digits > 27) || (prefix == 'v' && digits > 9) {
            self.numberError("number literal has too many digits", start);
        }
        if !self.atEnd() && (Chars.isLetter(self.current()) || Chars.isDigit(self.current()) || self.current() == '_') {
            self.numberError("invalid character right after a number", start);
        }
        return TokenKind.Number;
    }

    /* Reports a number literal error and exits. */
    proc numberError(&self, message: text, start: int) {
        Console.printl("error: " + message + " at " + Texts.toText(start));
        Process.exit(1);
    }

    /* The character after the current one, or 0 at the end. */
    proc next(&self) -> tryte {
        if self.position + 1 < len(self.source) {
            return self.source[self.position + 1];
        }
        return 0;
    }

    /* Returns the `TokenKind` corresponding to a keyword, or `Identifier` if it's not a keyword. */
    proc keyword(&self, word: text) -> TokenKind {
        if word == "proc" {
            return TokenKind.Proc;
        }
        else if word == "return" {
            return TokenKind.Return;
        }
        else if word == "if" {
            return TokenKind.If;
        }
        else if word == "else" {
            return TokenKind.Else;
        }
        else if word == "while" {
            return TokenKind.While;
        }
        else if word == "for" {
            return TokenKind.For;
        }
        else if word == "match" {
            return TokenKind.Match;
        }
        else if word == "break" {
            return TokenKind.Break;
        }
        else if word == "continue" {
            return TokenKind.Continue;
        }
        else if word == "struct" {
            return TokenKind.Struct;
        }
        else if word == "enum" {
            return TokenKind.Enum;
        }
        else if word == "const" {
            return TokenKind.Const;
        }
        else if word == "true" {
            return TokenKind.True;
        }
        else if word == "false" {
            return TokenKind.False;
        }
        else if word == "null" {
            return TokenKind.Null;
        }
        else if word == "use" {
            return TokenKind.Use;
        }
        else if word == "final" {
            return TokenKind.Final;
        }
        else if word == "claim" {
            return TokenKind.Claim;
        }
        else if word == "burn" {
            return TokenKind.Burn;
        }
        else if word == "give" {
            return TokenKind.Give;
        }
        else if word == "lend" {
            return TokenKind.Lend;
        }
        else if word == "own" {
            return TokenKind.Own;
        }
        else if word == "lent" {
            return TokenKind.Lent;
        }
        else if word == "tri" {
            return TokenKind.Tri;
        }
        else if word == "walk" {
            return TokenKind.Walk;
        }
        else if word == "in" {
            return TokenKind.In;
        }
        else if word == "span" {
            return TokenKind.Span;
        }
        else if word == "on" {
            return TokenKind.On;
        }
        else if word == "NaT" {
            return TokenKind.Poison;
        }
        else if word == "trit" {
            return TokenKind.Trit;
        }
        else if word == "bool" {
            return TokenKind.Bool;
        }
        else if word == "tryte" {
            return TokenKind.Tryte;
        }
        else if word == "int" {
            return TokenKind.Int;
        }
        else if word == "real" {
            return TokenKind.Real;
        }
        else if word == "text" {
            return TokenKind.Text;
        }
        else if word == "class" {
            return TokenKind.Class;
        }
        else if word == "interface" {
            return TokenKind.Interface;
        }
        else if word == "try" {
            return TokenKind.Try;
        }
        else if word == "catch" {
            return TokenKind.Catch;
        }
        else if word == "throw" {
            return TokenKind.Throw;
        }
        else if word == "throws" {
            return TokenKind.Throws;
        }
        else if word == "native" {
            return TokenKind.Native;
        }
        else if word == "public" {
            return TokenKind.Public;
        }
        else if word == "private" {
            return TokenKind.Private;
        }
        else if word == "sym" {
            return TokenKind.Sym;
        }
        else if word == "is" {
            return TokenKind.Is;
        }
        else if word == "extends" {
            return TokenKind.Extends;
        }
        return TokenKind.Identifier;
    }

    /*
     * The symbol at the current character, which it reads: the longest
     * one it can be (`->` before `-`).
     */
    proc symbol(&self) -> TokenKind {
        char: tryte = self.current();
        self.position++;
        match char {
            '(' => return TokenKind.LeftParen;
            ')' => return TokenKind.RightParen;
            '{' => return TokenKind.LeftBrace;
            '}' => return TokenKind.RightBrace;
            '[' => return TokenKind.LeftBracket;
            ']' => return TokenKind.RightBracket;
            ';' => return TokenKind.Semicolon;
            ':' => return TokenKind.Colon;
            ',' => return TokenKind.Comma;
            '@' => return TokenKind.At;
            '+' => return self.either('+', TokenKind.PlusPlus, '=', TokenKind.PlusEqual, TokenKind.Plus);
            '-' => {
                if self.take('>') {
                    return TokenKind.Arrow;
                }
                return self.either('-', TokenKind.MinusMinus, '=', TokenKind.MinusEqual, TokenKind.Minus);
            }
            '*' => return self.either('*', TokenKind.StarStar, '=', TokenKind.StarEqual, TokenKind.Star);
            '/' => return self.pick('=', TokenKind.SlashEqual, TokenKind.Slash);
            '%' => return self.pick('=', TokenKind.PercentEqual, TokenKind.Percent);
            '=' => return self.either('=', TokenKind.EqualEqual, '>', TokenKind.FatArrow, TokenKind.Equal);
            '!' => return self.pick('=', TokenKind.NotEqual, TokenKind.Bang);
            '>' => return self.either('=', TokenKind.GreaterEqual, '>', TokenKind.ShiftRight, TokenKind.Greater);
            '<' => {
                if self.take('=') {
                    if self.take('>') {
                        return TokenKind.Spaceship;
                    }
                    return TokenKind.LessEqual;
                }
                if self.take('<') {
                    return TokenKind.ShiftLeft;
                }
                return TokenKind.Less;
            }
            '&' => return self.pick('&', TokenKind.AndAnd, TokenKind.Ampersand);
            '|' => return self.pick('|', TokenKind.OrOr, TokenKind.Pipe);
            '.' => return self.pick('.', TokenKind.DotDot, TokenKind.Dot);
            '?' => return self.either('.', TokenKind.QuestionDot, ':', TokenKind.Elvis, TokenKind.Question);
            else => {
                // For now; a `Diagnostic` will take its place.
                Console.printl("error: unknown character at " + Texts.toText(self.position - 1));
                Process.exit(1);
            }
        }
    }

    /*
     * After a symbol's first character: `first` if `firstChar` comes next,
     * `second` if `secondChar` does (and reads it), or else `alone`.
     */
    proc either(&self, firstChar: tryte, first: TokenKind, secondChar: tryte, second: TokenKind, alone: TokenKind) -> TokenKind {
        if self.take(firstChar) {
            return first;
        }
        if self.take(secondChar) {
            return second;
        }
        return alone;
    }

    /* After a symbol's first character: `longer` if `next` comes next (and reads it), or else `alone`. */
    proc pick(&self, next: tryte, longer: TokenKind, alone: TokenKind) -> TokenKind {
        if self.take(next) {
            return longer;
        }
        return alone;
    }

    /* Reads the current character if it is `expected`. */
    proc take(&self, expected: tryte) -> bool {
        if !self.atEnd() && self.current() == expected {
            self.position++;
            return true;
        }
        return false;
    }
}
