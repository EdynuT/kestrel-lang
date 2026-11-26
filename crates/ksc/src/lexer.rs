//! Lexer: turns `.kstl` source text into tokens (SPEC §2, §11).
//!
//! Instruction mnemonics (`add`, `mov`, ...) and built-ins (`print`, `show`,
//! ...) are plain identifiers; the parser recognizes them by context, so they
//! stay usable as variable names.

use std::fmt;

use kestrel_core::bet::{parse_hepta, parse_trits, LiteralError};
use kestrel_core::charset::char_to_tryte;
use kestrel_core::word::WORD_MAX;
use kestrel_core::{Real, Word};

use crate::diag::{Diagnostic, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// Integer literal: decimal, `0t...` or `0v...` (may be NaT).
    Num(Word),
    RealLit(Real),
    /// Character literal, already converted to its tryte value.
    Char(i64),
    Str(String),
    Ident(String),

    // Keywords
    Proc,
    Return,
    Const,
    Final,
    Claim,
    Burn,
    Give,
    Lend,
    Own,
    Lent,
    Tri,
    Walk,
    In,
    Span,
    On,
    NaT,
    Break,
    Continue,
    Trit,
    Bool,
    True,
    False,
    Tryte,
    Int,
    Real,
    Text,

    // Operators and punctuation
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Percent,
    Shl,
    Shr,
    Spaceship,
    AndAnd,
    /// `&x` borrows `x`; `c: &Conta` is a borrowed object.
    Amp,
    OrOr,
    Bang,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    If,
    Else,
    While,
    For,
    Enum,
    Struct,
    Class,
    Interface,
    Use,
    Try,
    Catch,
    Throw,
    Throws,
    Native,
    Public,
    Private,
    At,
    /// `?` after a type: `No?` may be null.
    Question,
    /// `a?.b`: reads `b` only if `a` is not null.
    QuestionDot,
    /// `a ?: b`: `a`, or `b` if `a` is null.
    Elvis,
    Null,
    Sym,
    Match,
    /// `f is Circulo`: tests the class of an interface value.
    Is,
    /// `<T extends Forma>`: the interface a type parameter must implement.
    Extends,
    Dot,
    Pipe,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    PlusPlus,
    MinusMinus,
    Eq,
    Colon,
    Comma,
    Semi,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    DotDot,
    FatArrow,
    Arrow,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

const KEYWORDS: &[(&str, Tok)] = &[
    ("proc", Tok::Proc),
    ("return", Tok::Return),
    ("const", Tok::Const),
    ("final", Tok::Final),
    ("claim", Tok::Claim),
    ("burn", Tok::Burn),
    ("give", Tok::Give),
    ("lend", Tok::Lend),
    ("own", Tok::Own),
    ("lent", Tok::Lent),
    ("tri", Tok::Tri),
    ("if", Tok::If),
    ("else", Tok::Else),
    ("while", Tok::While),
    ("for", Tok::For),
    ("enum", Tok::Enum),
    ("struct", Tok::Struct),
    ("class", Tok::Class),
    ("interface", Tok::Interface),
    ("use", Tok::Use),
    ("try", Tok::Try),
    ("catch", Tok::Catch),
    ("throw", Tok::Throw),
    ("throws", Tok::Throws),
    ("native", Tok::Native),
    ("public", Tok::Public),
    ("private", Tok::Private),
    ("sym", Tok::Sym),
    ("match", Tok::Match),
    ("is", Tok::Is),
    ("extends", Tok::Extends),
    ("walk", Tok::Walk),
    ("in", Tok::In),
    ("span", Tok::Span),
    ("on", Tok::On),
    ("NaT", Tok::NaT),
    ("null", Tok::Null),
    ("break", Tok::Break),
    ("continue", Tok::Continue),
    ("trit", Tok::Trit),
    ("bool", Tok::Bool),
    ("true", Tok::True),
    ("false", Tok::False),
    ("tryte", Tok::Tryte),
    ("int", Tok::Int),
    ("real", Tok::Real),
    ("text", Tok::Text),
];

/// Operators, longest first so that `<=>` wins over `<<`.
const OPERATORS: &[(&str, Tok)] = &[
    ("<=>", Tok::Spaceship),
    ("**", Tok::StarStar),
    ("<<", Tok::Shl),
    (">>", Tok::Shr),
    ("+=", Tok::PlusEq),
    ("-=", Tok::MinusEq),
    ("*=", Tok::StarEq),
    ("/=", Tok::SlashEq),
    ("%=", Tok::PercentEq),
    ("++", Tok::PlusPlus),
    ("--", Tok::MinusMinus),
    ("<=", Tok::Le),
    (">=", Tok::Ge),
    ("==", Tok::EqEq),
    ("!=", Tok::NotEq),
    ("&&", Tok::AndAnd),
    ("&", Tok::Amp),
    ("||", Tok::OrOr),
    ("?.", Tok::QuestionDot),
    ("?:", Tok::Elvis),
    ("..", Tok::DotDot),
    (".", Tok::Dot),
    ("=>", Tok::FatArrow),
    ("->", Tok::Arrow),
    ("+", Tok::Plus),
    ("-", Tok::Minus),
    ("*", Tok::Star),
    ("/", Tok::Slash),
    ("%", Tok::Percent),
    ("!", Tok::Bang),
    ("<", Tok::Lt),
    (">", Tok::Gt),
    ("=", Tok::Eq),
    (":", Tok::Colon),
    (",", Tok::Comma),
    (";", Tok::Semi),
    ("|", Tok::Pipe),
    ("@", Tok::At),
    ("?", Tok::Question),
    ("(", Tok::LParen),
    (")", Tok::RParen),
    ("{", Tok::LBrace),
    ("}", Tok::RBrace),
    ("[", Tok::LBracket),
    ("]", Tok::RBracket),
];

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Num(w) => write!(f, "number `{w}`"),
            Tok::RealLit(r) => write!(f, "real `{r}`"),
            Tok::Char(_) => f.write_str("character literal"),
            Tok::Str(_) => f.write_str("string literal"),
            Tok::Ident(name) => write!(f, "`{name}`"),
            Tok::Eof => f.write_str("end of file"),
            other => {
                let text = KEYWORDS
                    .iter()
                    .chain(OPERATORS)
                    .find(|(_, t)| t == other)
                    .map_or("?", |(s, _)| s);
                write!(f, "`{text}`")
            }
        }
    }
}

/// The text of a keyword token (`text` for `Tok::Text`); `None` for others.
pub fn keyword_text(tok: &Tok) -> Option<&'static str> {
    KEYWORDS.iter().find(|(_, t)| t == tok).map(|(s, _)| *s)
}

pub fn lex(src: &str) -> Result<Vec<Token>, Diagnostic> {
    lex_at(src, 0)
}

/// Like [`lex`], for a file whose first byte sits at `base` in the
/// [`crate::diag::SourceMap`]: every span is moved by `base`.
pub fn lex_at(src: &str, base: usize) -> Result<Vec<Token>, Diagnostic> {
    let mut lx = Lexer { src, pos: 0, tokens: Vec::new() };
    let shift = |sp: Span| Span::new(sp.start + base, sp.end + base);
    lx.run().map_err(|d| Diagnostic { span: shift(d.span), ..d })?;
    for t in &mut lx.tokens {
        t.span = shift(t.span);
    }
    Ok(lx.tokens)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let start = self.pos;
        while self.peek().is_some_and(&f) {
            self.bump();
        }
        self.src[start..self.pos].to_string()
    }

    fn push(&mut self, tok: Tok, start: usize) {
        self.tokens.push(Token { tok, span: Span::new(start, self.pos) });
    }

    fn err(&self, msg: impl Into<String>, start: usize) -> Diagnostic {
        Diagnostic::new(msg, Span::new(start, self.pos.max(start + 1)))
    }

    fn run(&mut self) -> Result<(), Diagnostic> {
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                _ if c.is_whitespace() => {
                    self.bump();
                }
                '/' if self.peek_at(1) == Some('/') => {
                    self.take_while(|c| c != '\n');
                }
                // `/* ... */`: documentation (for now skipped like a comment;
                // it does not nest, as in Java).
                '/' if self.peek_at(1) == Some('*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            Some('*') if self.peek_at(1) == Some('/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            Some(_) => {
                                self.bump();
                            }
                            None => {
                                self.pos = start + 2;
                                return Err(self.err("`/*` is never closed", start).with_note("end the comment with `*/`"));
                            }
                        }
                    }
                }
                '0'..='9' => self.number()?,
                'A'..='Z' | 'a'..='z' | '_' => {
                    let word = self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
                    let tok = KEYWORDS
                        .iter()
                        .find(|(k, _)| *k == word.as_str())
                        .map_or_else(|| Tok::Ident(word.clone()), |(_, t)| t.clone());
                    self.push(tok, start);
                }
                '\'' => self.char_lit()?,
                '"' => self.string_lit()?,
                _ => {
                    let rest = &self.src[self.pos..];
                    let Some((text, tok)) = OPERATORS.iter().find(|(op, _)| rest.starts_with(op)) else {
                        self.bump();
                        let d = self.err(format!("unexpected character `{c}`"), start);
                        return Err(match c {
                            '~' => d.with_note("negate a trit with `!x`, a number with `-x`"),
                            '\\' => d.with_note("escapes like `\\n` only work inside quotes: `\"\\n\"`"),
                            _ => d,
                        });
                    };
                    self.pos += text.len();
                    self.push(tok.clone(), start);
                }
            }
        }
        self.push(Tok::Eof, self.pos);
        Ok(())
    }

    fn number(&mut self) -> Result<(), Diagnostic> {
        let start = self.pos;
        let prefix = (self.peek(), self.peek_at(1));
        let tok = if prefix == (Some('0'), Some('t')) {
            self.pos += 2;
            let body = self.take_while(|c| matches!(c, '+' | '-' | '0' | '?'));
            Tok::Num(parse_trits(&body).map_err(|e| self.literal_err(e, "0t", start))?)
        } else if prefix == (Some('0'), Some('v')) {
            self.pos += 2;
            let body = self.take_while(|c| c.is_ascii_alphanumeric() || c == '?');
            Tok::Num(parse_hepta(&body).map_err(|e| self.literal_err(e, "0v", start))?)
        } else {
            let int = self.take_while(|c| c.is_ascii_digit());
            // `1.5` is a real, but `1..5` is a range.
            if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
                let frac = self.take_while(|c| c.is_ascii_digit());
                let r = Real::from_decimal(&int, &frac)
                    .ok_or_else(|| self.err("real literal has too many digits", start))?;
                if r.is_nat() {
                    return Err(self.err("real literal is too large (limit about 10^174)", start));
                }
                Tok::RealLit(r)
            } else {
                let v: i128 = int.parse().unwrap_or(i128::MAX);
                if v > i128::from(WORD_MAX) {
                    return Err(self
                        .err("integer literal does not fit in an int", start)
                        .with_note(format!("an int holds -{WORD_MAX} ..= {WORD_MAX}")));
                }
                Tok::Num(Word::new(v as i64))
            }
        };
        if self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
            return Err(self.err("invalid character right after a number", start));
        }
        self.push(tok, start);
        Ok(())
    }

    fn literal_err(&self, e: LiteralError, prefix: &str, start: usize) -> Diagnostic {
        let msg = match e {
            LiteralError::Empty => format!("`{prefix}` literal has no digits"),
            LiteralError::TooLong { max, got } => format!("`{prefix}` literal has {got} digits (at most {max})"),
            LiteralError::BadDigit(c) => format!("`{c}` is not a valid digit in a `{prefix}` literal"),
        };
        let note = if prefix == "0t" { "trits are `-`, `0`, `+` and `?`" } else { "digits are `0`, `A`-`M`, `a`-`m` and `?`" };
        self.err(msg, start).with_note(note)
    }


    fn literal_char(&mut self, quote: char, start: usize) -> Result<Option<char>, Diagnostic> {
        let c_start = self.pos;
        let c = match self.bump() {
            None | Some('\n') => {
                let kind = if quote == '"' { "string" } else { "character" };
                return Err(self.err(format!("unterminated {kind} literal"), start));
            }
            Some(c) if c == quote => return Ok(None),
            Some('\\') => match self.bump() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('e') => '\x1b',
                Some('"') => '"',
                Some('\'') => '\'',
                Some('\\') => '\\',
                other => {
                    let shown = other.map_or(String::new(), |c| c.to_string());
                    return Err(self.err(format!("unknown escape `\\{shown}`"), c_start).with_note("escapes are `\\n`, `\\t`, `\\e`, `\\\"`, `\\'` and `\\\\`"));
                }
            },
            Some(c) => c,
        };
        if char_to_tryte(c).is_none() {
            return Err(self
                .err(format!("character {c:?} is not in the Kestrel character table"), c_start)
                .with_note("allowed: letters (with á à â ã é ê í ó ô õ ú ç), digits, space, \\n, \\t and . , : ; ! ? - + / _ = ( ) [ ] * < > @ # % { } \" ' | & \\ ~ ^ ` $"));
        }
        Ok(Some(c))
    }

    fn char_lit(&mut self) -> Result<(), Diagnostic> {
        let start = self.pos;
        self.bump(); // opening quote
        let Some(c) = self.literal_char('\'', start)? else {
            return Err(self.err("empty character literal", start));
        };
        if self.bump() != Some('\'') {
            return Err(self.err("character literal must hold exactly one character", start));
        }
        self.push(Tok::Char(char_to_tryte(c).expect("checked by literal_char")), start);
        Ok(())
    }

    fn string_lit(&mut self) -> Result<(), Diagnostic> {
        let start = self.pos;
        self.bump(); // opening quote
        let mut s = String::new();
        while let Some(c) = self.literal_char('"', start)? {
            s.push(c);
        }
        self.push(Tok::Str(s), start);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn err(src: &str) -> String {
        lex(src).unwrap_err().message
    }

    fn n(v: i64) -> Tok {
        Tok::Num(Word::new(v))
    }

    fn id(s: &str) -> Tok {
        Tok::Ident(s.into())
    }

    #[test]
    fn declaration() {
        assert_eq!(
            toks("acc: int = 0; // comment"),
            vec![id("acc"), Tok::Colon, Tok::Int, Tok::Eq, n(0), Tok::Semi, Tok::Eof]
        );
        assert_eq!(toks("i++; x += 2;"), vec![id("i"), Tok::PlusPlus, Tok::Semi, id("x"), Tok::PlusEq, n(2), Tok::Semi, Tok::Eof]);
    }

    #[test]
    fn block_comments() {
        assert_eq!(toks("a /* b\n * c */ d"), vec![id("a"), id("d"), Tok::Eof]);
        assert_eq!(toks("/**/x/* * / */"), vec![id("x"), Tok::Eof]);
        assert!(err("x /* never").contains("never closed"));
    }

    #[test]
    fn tri() {
        assert_eq!(
            toks("tri s <=> 10 { - => x, 0 => y, + => z, }")[..8],
            [Tok::Tri, id("s"), Tok::Spaceship, n(10), Tok::LBrace, Tok::Minus, Tok::FatArrow, id("x")]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(toks("0t+0- 0vAm 42"), vec![n(8), n(14), n(42), Tok::Eof]);
        assert_eq!(toks("0t+?"), vec![Tok::Num(Word::NAT), Tok::Eof]);
        assert_eq!(toks("1..5"), vec![n(1), Tok::DotDot, n(5), Tok::Eof]);
        assert_eq!(toks("1.5"), vec![Tok::RealLit(Real::from_decimal("1", "5").unwrap()), Tok::Eof]);
        assert_eq!(toks("3812798742493"), vec![n(WORD_MAX), Tok::Eof]);
        assert_eq!(toks("2 ** 3"), vec![n(2), Tok::StarStar, n(3), Tok::Eof]);
        assert_eq!(toks("proc f() -> int"), vec![Tok::Proc, id("f"), Tok::LParen, Tok::RParen, Tok::Arrow, Tok::Int, Tok::Eof]);
    }

    #[test]
    fn text_and_chars() {
        assert_eq!(toks(r#"print("Hi\n");"#)[2], Tok::Str("Hi\n".into()));
        assert_eq!(toks("'A' 'a' ' ' '\\n'"), vec![Tok::Char(1), Tok::Char(-1), Tok::Char(0), Tok::Char(-27), Tok::Eof]);
        assert_eq!(toks(r#""\e[31mred\e[0m""#)[0], Tok::Str("\x1b[31mred\x1b[0m".into()));
        assert_eq!(toks("'\\e'"), vec![Tok::Char(-73), Tok::Eof]);
    }

    #[test]
    fn errors() {
        assert!(err("3812798742494").contains("does not fit in an int"));
        assert!(err("0t+x").contains("right after a number"));
        assert!(err("0vN").contains("`N` is not a valid digit"));
        assert!(err("0t").contains("no digits"));
        assert!(err("12abc").contains("right after a number"));
        assert!(err("x: int $ 1;").contains("unexpected character `$`"));
        assert!(err(r#""a € b""#).contains("'€' is not in the Kestrel character table"));
        assert_eq!(toks(r#""a | b \\ c""#)[0], Tok::Str("a | b \\ c".into()));
        assert!(err(r#""niño""#).contains("'ñ'"));
        assert!(err(r#""open"#).contains("unterminated string"));
        assert!(err("'ab'").contains("exactly one character"));
        assert!(err(r#""\q""#).contains("unknown escape"));
        let d = lex("~x").unwrap_err();
        assert_eq!(d.message, "unexpected character `~`");
        assert!(d.note.unwrap().contains("`!x`"));
        assert_eq!(lex("a <= b != c == d >= e < f > g <=> h ! i").unwrap().len(), 18); // 17 + Eof
    }

    #[test]
    fn spans_cover_tokens() {
        let t = lex("x <= 0t+0-;").unwrap();
        assert_eq!(t[0].span, Span::new(0, 1));
        assert_eq!(t[1].span, Span::new(2, 4));
        assert_eq!(t[2].span, Span::new(5, 10));
    }

    #[test]
    fn spec_example_lexes() {
        let src = r#"
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

proc main() -> int {
    sym b[]: int = claim 5;       // indices -2..+2
    walk i in span 2 {
        b[i] = i * i;
    }

    s: int = sum(lend b, 2) on NaT { burn b; return -1; };
    s = s << 1;                    // s = s * 3

    tri s <=> 10 {
        - => print("less"),
        0 => print("equal"),
        + => print("greater"),
    }
    burn b;
    return 0;
}
"#;
        let t = toks(src);
        assert_eq!(t.last(), Some(&Tok::Eof));
        assert_eq!(t.iter().filter(|t| **t == Tok::Burn).count(), 2);
    }
}
