// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * What a token is: a name, a literal, a keyword or a symbol.
 *
 * Each keyword and each symbol has a kind of its own, so the parser
 * compares kinds (`TokenKind.Proc`), never texts.
 *
 * @since 0.8
 * @author Edynu
 */

public enum TokenKind {
    // Names and literals: their text is read from the source.
    Identifier,     // count, List, main
    Number,         // 42, 0t+-, 0vM
    RealNumber,     // 2.5
    Character,      // 'a'
    String,         // "text"

    // Keywords
    Proc,           // proc
    Return,         // return
    Const,          // const
    Final,          // final
    Claim,          // claim
    Burn,           // burn
    Give,           // give
    Lend,           // lend
    Own,            // own
    Lent,           // lent
    Tri,            // tri
    Walk,           // walk
    In,             // in
    Span,           // span
    On,             // on
    Poison,         // NaT (an enum item cannot be named `NaT`: it is a word of the language)
    Break,          // break
    Continue,       // continue
    Trit,           // trit
    Bool,           // bool
    True,           // true
    False,          // false
    Tryte,          // tryte
    Int,            // int
    Real,           // real
    Text,           // text
    If,             // if
    Else,           // else
    While,          // while
    For,            // for
    Enum,           // enum
    Struct,         // struct
    Class,          // class
    Interface,      // interface
    Use,            // use
    Try,            // try
    Catch,          // catch
    Throw,          // throw
    Throws,         // throws
    Native,         // native
    Public,         // public
    Private,        // private
    Null,           // null
    Sym,            // sym
    Match,          // match
    Is,             // is
    Extends,        // extends

    // Arithmetic
    Plus,           // +
    Minus,          // -
    Star,           // *
    StarStar,       // **
    Slash,          // /
    Percent,        // %
    ShiftLeft,      // <<
    ShiftRight,     // >>

    // Comparison and logic
    Spaceship,      // <=>
    AndAnd,         // &&
    OrOr,           // ||
    Bang,           // !
    EqualEqual,     // ==
    NotEqual,       // !=
    Less,           // <
    Greater,        // >
    LessEqual,      // <=
    GreaterEqual,   // >=

    // Assignment
    Equal,          // =
    PlusEqual,      // +=
    MinusEqual,     // -=
    StarEqual,      // *=
    SlashEqual,     // /=
    PercentEqual,   // %=
    PlusPlus,       // ++
    MinusMinus,     // --

    // Null
    Question,       // ?   (after a type: `Node?`)
    QuestionDot,    // ?.
    Elvis,          // ?:

    // Punctuation
    Ampersand,      // &   (a loan: `&self`, `&values`)
    At,             // @
    Dot,            // .
    DotDot,         // ..
    Pipe,           // |
    Colon,          // :
    Comma,          // ,
    Semicolon,      // ;
    LeftParen,      // (
    RightParen,     // )
    LeftBrace,      // {
    RightBrace,     // }
    LeftBracket,    // [
    RightBracket,   // ]
    Arrow,          // ->
    FatArrow,       // =>

    EndOfFile
}
