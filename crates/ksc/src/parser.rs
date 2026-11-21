//! Recursive-descent parser: tokens -> [`Program`] (SPEC §11).
//!
//! Expression precedence, lowest to highest:
//! `||`, `&&`, comparisons (`<=> == != < > <= >=`, not chainable), `+ -`, `* / %`, `<< >>`, unary `! -` and casts `(T) x`, `**`
//! (right-associative, so `-2 ** 2 == -(2 ** 2)`), indexing, primary.

use kestrel_core::Word;

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token};

pub fn parse(tokens: Vec<Token>) -> Result<Program, Diagnostic> {
    parse_source(tokens, "")
}

/// Like [`parse`], with the source text the tokens came from: a failed
/// `assert` then shows its line and its condition as written.
pub fn parse_source(tokens: Vec<Token>, src: &str) -> Result<Program, Diagnostic> {
    let mut p = Parser { tokens, pos: 0, in_head: false, depth: 0, class: None, src: src.to_string(), safe_vars: 0, splits: Vec::new() };
    let mut items = Vec::new();
    while p.peek() != &Tok::Eof {
        items.push(p.item()?);
    }
    Ok(Program { items })
}

/// One expression and nothing else (the `{...}` of a `printf` format).
pub fn parse_expr(tokens: Vec<Token>) -> Result<Expr, Diagnostic> {
    let mut p = Parser { tokens, pos: 0, in_head: false, depth: 0, class: None, src: String::new(), safe_vars: 0, splits: Vec::new() };
    let e = p.expr()?;
    if p.peek() != &Tok::Eof {
        return Err(p.unexpected("the end of `{...}`"));
    }
    Ok(e)
}

struct Parser {
    tokens: Vec<Token>,
    /// The source text (empty when unknown), for `assert` messages.
    src: String,
    /// `?.` seen so far: each gets its own hidden variable.
    safe_vars: u32,
    /// `>>` tokens split in two by [`Parser::close_angle`], to undo when a
    /// guess that `<` opened type arguments was wrong.
    splits: Vec<(usize, Token)>,
    pos: usize,
    /// Inside the head of `if`/`while`/`match`/`tri`/`walk`: a `{` there
    /// opens the body, never an array literal (the same rule as Rust).
    in_head: bool,
    /// How deep the code being parsed is nested (blocks, parentheses,
    /// operators). Bounded so deep input gives an error, not a stack overflow.
    depth: u32,
    /// The class whose body is being parsed (methods may take `self`).
    class: Option<&'static str>,
}

/// Nesting limit, like rustc's `recursion_limit`.
use kestrel_core::limits::MAX_NESTING as MAX_DEPTH;

type PResult<T> = Result<T, Diagnostic>;

impl Parser {
    // ---- token helpers ----

    fn peek(&self) -> &Tok {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let last = self.tokens.len() - 1; // always Eof
        &self.tokens[(self.pos + n).min(last)].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos.min(self.tokens.len() - 1)].span
    }

    /// Span right after the previous token, where a missing `;` belongs.
    fn after_prev(&self) -> Span {
        let end = self.pos.checked_sub(1).map_or(0, |i| self.tokens[i].span.end);
        Span::new(end, end + 1)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == tok {
            self.bump();
            true
        } else {
            false
        }
    }

    fn unexpected(&self, expected: &str) -> Diagnostic {
        Diagnostic::new(format!("expected {expected}, found {}", self.peek()), self.span())
    }

    fn expect(&mut self, tok: Tok) -> PResult<Span> {
        if self.peek() == &tok {
            return Ok(self.bump().span);
        }
        // A missing `;` reads better pointed at the end of the previous token.
        if tok == Tok::Semi {
            return Err(Diagnostic::new(format!("expected `;`, found {}", self.peek()), self.after_prev()));
        }
        Err(self.unexpected(&tok.to_string()))
    }

    fn ident(&mut self) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            _ => Err(self.unexpected("a name")),
        }
    }

    // ---- items ----

    fn item(&mut self) -> PResult<Item> {
        match self.peek() {
            Tok::Proc => self.proc_decl().map(Item::Proc),
            Tok::Const => self.const_decl().map(Item::Const),
            Tok::Enum => self.enum_decl().map(Item::Enum),
            Tok::Struct => self.struct_decl().map(Item::Struct),
            Tok::Interface => self.interface_decl().map(Item::Interface),
            Tok::Class | Tok::At => {
                let annotations = self.annotations()?;
                match self.peek() {
                    Tok::Class => self.class_decl(annotations).map(Item::Class),
                    Tok::Proc => {
                        let mut p = self.proc_decl()?;
                        p.annotations = annotations;
                        Ok(Item::Proc(p))
                    }
                    _ => Err(self.unexpected("`proc` or `class` after the annotations")),
                }
            }
            _ => Err(self.unexpected("`proc`, `const`, `enum`, `struct`, `class` or `interface`")),
        }
    }

    fn const_decl(&mut self) -> PResult<ConstDecl> {
        self.expect(Tok::Const)?;
        let name = self.ident()?;
        self.expect(Tok::Colon)?;
        let ty = self.type_expr()?;
        self.expect(Tok::Eq)?;
        let value = self.expr()?;
        self.expect(Tok::Semi)?;
        Ok(ConstDecl { name, ty, value })
    }

    /// `@name` / `@name(A, B)` / `@name("text")`, any number of them.
    fn annotations(&mut self) -> PResult<Vec<Annotation>> {
        let mut out = Vec::new();
        while self.eat(&Tok::At) {
            let name = self.ident()?;
            let mut args = Vec::new();
            let mut text = None;
            if self.eat(&Tok::LParen) {
                while self.peek() != &Tok::RParen {
                    if let Tok::Str(s) = self.peek().clone() {
                        self.bump();
                        text = Some(s);
                    } else {
                        args.push(self.ident()?);
                    }
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(Tok::RParen)?;
            }
            out.push(Annotation { name, args, text });
        }
        Ok(out)
    }

    /// `class Name { [public|private] field: T; ... [public|private] proc m(...) { } }`
    fn class_decl(&mut self, annotations: Vec<Annotation>) -> PResult<ClassDecl> {
        self.expect(Tok::Class)?;
        let name = self.ident()?;
        let generics = self.type_params()?;
        self.expect(Tok::LBrace)?;
        let outer = self.class.replace(intern(&name.name));
        let (mut fields, mut methods) = (Vec::new(), Vec::new());
        let result = (|| {
            while self.peek() != &Tok::RBrace {
                let anns = self.annotations()?;
                let public = if self.eat(&Tok::Public) {
                    true
                } else {
                    self.eat(&Tok::Private);
                    false
                };
                if self.peek() == &Tok::Proc {
                    methods.push(MethodDecl { decl: self.proc_decl()?, public, annotations: anns });
                    continue;
                }
                if let Some(a) = anns.first() {
                    return Err(Diagnostic::new("annotations go on methods or on the class", a.name.span));
                }
                let sym_span = self.span();
                let sym = self.eat(&Tok::Sym);
                let (fname, ty) = self.typed_name(sym, sym_span)?;
                self.expect(Tok::Semi)?;
                fields.push(FieldDecl { name: fname, ty, public });
            }
            Ok(())
        })();
        self.class = outer;
        result?;
        self.expect(Tok::RBrace)?;
        Ok(ClassDecl { name, generics, instance: false, annotations, fields, methods })
    }

    /// `<T, K extends Hash>` after a class or proc name (nothing if absent).
    fn type_params(&mut self) -> PResult<Vec<TypeParam>> {
        let mut out: Vec<TypeParam> = Vec::new();
        if !self.eat(&Tok::Lt) {
            return Ok(out);
        }
        loop {
            let name = self.ident()?;
            if out.iter().any(|p| p.name.name == name.name) {
                return Err(Diagnostic::new(format!("type parameter `{}` appears twice", name.name), name.span));
            }
            let bound = if self.eat(&Tok::Extends) { Some(self.ident()?) } else { None };
            out.push(TypeParam { name, bound });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.close_angle()?;
        Ok(out)
    }

    /// The `>` that closes `<...>`; in `List<List<int>>` the lexer made `>>`
    /// one token, so its first half is taken and a `>` is left.
    fn close_angle(&mut self) -> PResult<Span> {
        match self.peek() {
            Tok::Gt => Ok(self.bump().span),
            Tok::Shr => {
                let at = self.pos;
                self.splits.push((at, self.tokens[at].clone()));
                let span = self.tokens[at].span;
                self.tokens[at] = Token { tok: Tok::Gt, span: Span::new(span.start + 1, span.end) };
                Ok(Span::new(span.start, span.start + 1))
            }
            _ => Err(self.unexpected("`>`")),
        }
    }

    /// `<int, text>` after a generic's name: the type arguments.
    fn type_args(&mut self) -> PResult<Vec<Type>> {
        self.expect(Tok::Lt)?;
        let mut args = Vec::new();
        loop {
            let t = self.type_expr()?;
            if let Type::Own(_) = t.ty {
                return Err(Diagnostic::new("a type argument cannot be an array", t.span)
                    .with_note("use a class that holds the array, or `List<T>`"));
            }
            args.push(t.ty);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.close_angle()?;
        Ok(args)
    }

    /// `Name<args>` in an expression (`Box<int>.new()`, `max<int>(a, b)`,
    /// `Box<T> { ... }`), told apart from `a < b` by what follows the `>`.
    fn generic_ref(&mut self, name: &str) -> Option<&'static str> {
        if self.peek() != &Tok::Lt {
            return None;
        }
        let (pos, splits) = (self.pos, self.splits.len());
        if let Ok(args) = self.type_args() {
            let next = self.peek();
            if matches!(next, Tok::Dot | Tok::LParen) || (next == &Tok::LBrace && !self.in_head) {
                return Some(generic_name(name, &args));
            }
        }
        // Not a generic: undo, `<` is a comparison.
        while self.splits.len() > splits {
            let (at, tok) = self.splits.pop().expect("checked");
            self.tokens[at] = tok;
        }
        self.pos = pos;
        None
    }

    fn proc_decl(&mut self) -> PResult<ProcDecl> {
        self.expect(Tok::Proc)?;
        let name = self.ident()?;
        let generics = self.type_params()?;
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        let mut this = None;
        while self.peek() != &Tok::RParen {
            // `c: &Conta` and `&v[]: int` borrow; `c: Conta` and `v[]: int`
            // take the object or array (the caller writes `give`).
            if matches!(self.peek(), Tok::Lent | Tok::Own) {
                return Err(Diagnostic::new(format!("`{}` is not used any more", self.peek()), self.span()).with_note(
                    "a borrowed parameter is `c: &Conta` (array: `&v[]: int`, method: `&self`); one that takes it is `c: Conta` (method: `self`)",
                ));
            }
            let sym_span = self.span();
            let sym = self.eat(&Tok::Sym);
            let amp_span = self.span();
            let amp = self.eat(&Tok::Amp);
            // A method's first parameter: `&self` uses the object, `self` consumes it.
            if self.peek() == &Tok::Ident("self".into()) {
                let at = self.span();
                if self.class.is_none() || !params.is_empty() || this.is_some() || sym {
                    return Err(Diagnostic::new("`self` can only be the first parameter of a method", at));
                }
                this = Some(if amp { SelfMode::Lent } else { SelfMode::Own });
                self.bump();
                if !self.eat(&Tok::Comma) {
                    break;
                }
                continue;
            }
            let (pname, mut ty) = self.typed_name(sym, sym_span)?;
            let mode = match ty.ty {
                Type::Own(b) if amp => {
                    ty.ty = Type::Lent(b);
                    Some(SelfMode::Lent)
                }
                _ if amp => {
                    return Err(Diagnostic::new("`&` before the name is for arrays", amp_span)
                        .with_note(format!("an object is borrowed with `&` on its type: `{}: &T`", pname.name)));
                }
                Type::LentNamed(_) | Type::LentNamedOpt(_) => Some(SelfMode::Lent),
                Type::Own(_) => Some(SelfMode::Own),
                // An object without `&` is taken (checked later: the name must be a class).
                _ => None,
            };
            params.push(Param { name: pname, ty, mode });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        // In an interface, a method ends with `;` (no body), unless it is a
        // default method with a `{ ... }` body.
        if self.class.is_some_and(|c| c.starts_with('@')) && self.peek() != &Tok::LBrace {
            let at = self.expect(Tok::Semi)?;
            return Ok(ProcDecl {
                name,
                this,
                params,
                ret,
                body: Block { stmts: Vec::new(), span: at },
                generics,
                instance: false,
                annotations: Vec::new(),
            });
        }
        let body = self.block()?;
        Ok(ProcDecl { name, this, params, ret, body, generics, instance: false, annotations: Vec::new() })
    }

    /// `interface Name { proc m(&self, ...) -> T; ... }`
    fn interface_decl(&mut self) -> PResult<InterfaceDecl> {
        self.expect(Tok::Interface)?;
        let name = self.ident()?;
        self.expect(Tok::LBrace)?;
        // `@` marks "inside an interface" for `proc_decl` (methods have no body).
        let outer = self.class.replace(intern(&format!("@{}", name.name)));
        let mut methods = Vec::new();
        let mut bodies = Vec::new();
        let result = (|| {
            while self.peek() != &Tok::RBrace {
                if matches!(self.peek(), Tok::Public | Tok::Private) {
                    return Err(Diagnostic::new("interface methods are always public", self.span()));
                }
                if self.peek() != &Tok::Proc {
                    return Err(self.unexpected("`proc`").with_note("an interface only lists methods: `proc area(&self) -> real;`"));
                }
                let has_body = {
                    // Look past the signature for `{`: a default method.
                    let m = self.proc_decl()?;
                    let body = !m.body.stmts.is_empty() || self.tokens[self.pos - 1].tok == Tok::RBrace;
                    methods.push(m);
                    body
                };
                bodies.push(has_body);
            }
            Ok(())
        })();
        self.class = outer;
        result?;
        self.expect(Tok::RBrace)?;
        Ok(InterfaceDecl { name, methods, bodies })
    }

    /// `[sym] name[]: T` (array) or `name: T`, as in declarations and
    /// parameters. Returns the name and its type.
    fn typed_name(&mut self, sym: bool, sym_span: Span) -> PResult<(Ident, TypeExpr)> {
        let name = self.ident()?;
        let array = self.peek() == &Tok::LBracket;
        if array {
            self.bump();
            self.expect(Tok::RBracket)?;
        } else if sym {
            return Err(Diagnostic::new("`sym` only applies to arrays", sym_span)
                .with_note(format!("write `sym {}[]: int`", name.name)));
        }
        self.expect(Tok::Colon)?;
        let ty = self.type_expr()?;
        if !array {
            if let Type::Own(_) = ty.ty {
                return Err(Diagnostic::new("put the `[]` on the name", ty.span)
                    .with_note(format!("write `{}[]: {}`", name.name, ty.ty.to_string().trim_end_matches("[]").trim_start_matches("sym "))));
            }
            return Ok((name, ty));
        }
        let Some(elem) = Elem::of(ty.ty) else {
            if let Type::NamedOpt(n) = ty.ty {
                return Err(Diagnostic::new("an array cannot hold optional objects", ty.span)
                    .with_note(format!("every element is an object: write `{}[]: {n}`", name.name)));
            }
            return Err(Diagnostic::new("an array cannot hold arrays", ty.span)
                .with_note("write the element type: `x[]: int`"));
        };
        Ok((name, TypeExpr { ty: Type::Own(Buf { elem, sym }), span: ty.span }))
    }

    fn scalar(&mut self) -> Option<Scalar> {
        let s = match self.peek() {
            Tok::Trit => Scalar::Trit,
            Tok::Tryte => Scalar::Tryte,
            Tok::Int => Scalar::Int,
            Tok::Real => Scalar::Real,
            _ => return None,
        };
        self.bump();
        Some(s)
    }

    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        self.expect(Tok::Enum)?;
        let name = self.ident()?;
        self.expect(Tok::LBrace)?;
        let mut variants = Vec::new();
        while self.peek() != &Tok::RBrace {
            let v = self.ident()?;
            let value = if self.eat(&Tok::Eq) { Some(self.expr()?) } else { None };
            variants.push((v, value));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        Ok(EnumDecl { name, variants })
    }

    /// `struct Name { a: int; b: text; }`
    fn struct_decl(&mut self) -> PResult<StructDecl> {
        self.expect(Tok::Struct)?;
        let name = self.ident()?;
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        while self.peek() != &Tok::RBrace {
            let field = self.ident()?;
            if self.peek() == &Tok::LBracket {
                return Err(Diagnostic::new("a struct cannot hold an array", self.span())
                    .with_note("a struct is plain data that is copied; an array has an owner"));
            }
            self.expect(Tok::Colon)?;
            let ty = self.type_expr()?;
            if let Type::Own(_) = ty.ty {
                return Err(Diagnostic::new("a struct cannot hold an array", ty.span)
                    .with_note("a struct is plain data that is copied; an array has an owner"));
            }
            if self.peek() == &Tok::Comma {
                return Err(self.unexpected("`;`").with_note("struct fields end with `;`, like variables"));
            }
            self.expect(Tok::Semi)?;
            fields.push((field, ty));
        }
        self.expect(Tok::RBrace)?;
        Ok(StructDecl { name, fields })
    }

    /// A type: `int`, `text`, an enum or struct name, or an array type `[sym] T[]`
    /// (array types are written this way only after `->`).
    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        // `&Conta`, `&No?`: a borrowed object.
        if self.eat(&Tok::Amp) {
            let inner = self.type_expr()?;
            let ty = match inner.ty {
                Type::Named(n) => Type::LentNamed(n),
                Type::NamedOpt(n) => Type::LentNamedOpt(n),
                Type::Own(_) => {
                    return Err(Diagnostic::new("a borrowed array puts `&` before its name", start)
                        .with_note("write `&v[]: int`"));
                }
                t => {
                    return Err(Diagnostic::new(format!("only arrays and objects are borrowed with `&`; {t} is copied"), start)
                        .with_note("write the type without `&`"));
                }
            };
            return Ok(TypeExpr { ty, span: start.to(inner.span) });
        }
        let sym = self.eat(&Tok::Sym);
        let ty = if let Some(s) = self.scalar() {
            Type::Scalar(s)
        } else if self.eat(&Tok::Bool) {
            Type::Bool
        } else {
            match self.peek().clone() {
                Tok::Ident(name) => {
                    self.bump();
                    // `Box<int>`: a generic class with its type arguments.
                    let name = if self.peek() == &Tok::Lt { generic_name(&name, &self.type_args()?).to_string() } else { name };
                    if self.eat(&Tok::Question) {
                        if self.peek() == &Tok::LBracket {
                            return Err(Diagnostic::new("an array cannot hold optional objects", start)
                                .with_note(format!("every element is an object: write `{name}[]`")));
                        }
                        Type::NamedOpt(intern(&name))
                    } else {
                        Type::Named(intern(&name))
                    }
                }
                Tok::Text => {
                    self.bump();
                    Type::Text
                }
                Tok::Own | Tok::Lent => {
                    return Err(self
                        .unexpected("a type")
                        .with_note("a borrowed object is `&Conta`; arrays are declared on the name: `x[]: int`, `&v[]: int`"));
                }
                Tok::Eq => return Err(self.unexpected("a type").with_note("every variable has its type written: `x: int = 5;`")),
                _ => return Err(self.unexpected("a type")),
            }
        };
        // `int?`, `text?`: may be null (class and enum names were handled above).
        let ty = if self.peek() == &Tok::Question {
            if self.peek_at(1) == &Tok::LBracket {
                return Err(Diagnostic::new("array elements cannot be null", self.span())
                    .with_note(format!("write `{ty}[]`")));
            }
            self.bump();
            ty.opt().expect("a scalar, bool or text")
        } else {
            ty
        };
        // `T[]` (after `->`): an array type.
        if self.peek() == &Tok::LBracket && self.peek_at(1) == &Tok::RBracket {
            self.bump();
            self.bump();
            let elem = Elem::of(ty).expect("a plain type");
            return Ok(TypeExpr { ty: Type::Own(Buf { elem, sym }), span: start.to(self.tokens[self.pos - 1].span) });
        }
        if sym {
            return Err(Diagnostic::new("`sym` only applies to arrays", start).with_note(format!("write `sym {ty}[]`")));
        }
        Ok(TypeExpr { ty, span: start.to(self.tokens[self.pos - 1].span) })
    }

    // ---- statements ----

    /// One level deeper; fails past [`MAX_DEPTH`]. Pair with `self.depth -= n`.
    fn nest(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Diagnostic::new(format!("this code is nested too deeply (limit {MAX_DEPTH} levels)"), self.span())
                .with_note("split it into smaller pieces: intermediate variables or separate procs"));
        }
        Ok(())
    }

    fn block(&mut self) -> PResult<Block> {
        self.nest()?;
        let start = self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            stmts.push(self.stmt()?);
        }
        let end = self.expect(Tok::RBrace)?;
        self.depth -= 1;
        Ok(Block { stmts, span: start.to(end) })
    }

    /// `assert(cond);` / `assert(cond, "message");`
    fn assert_stmt(&mut self) -> PResult<StmtKind> {
        let at = self.bump().span;
        self.expect(Tok::LParen)?;
        let cond = self.expr()?;
        let text = if self.eat(&Tok::Comma) {
            match self.peek().clone() {
                Tok::Str(s) => {
                    self.bump();
                    Some(s)
                }
                _ => return Err(self.unexpected("a string literal: `assert(x > 0, \"x must be positive\")`")),
            }
        } else {
            None
        };
        self.expect(Tok::RParen)?;
        self.expect(Tok::Semi)?;
        let written = self.src.get(cond.span.start..cond.span.end).map(str::to_string);
        let line = self.src.get(..at.start).map(|s| s.matches('\n').count() + 1);
        let what = text.or(written).unwrap_or_else(|| "assert failed".into());
        let msg = match line {
            Some(n) if !self.src.is_empty() => format!("assert failed (line {n}): {what}"),
            _ => format!("assert failed: {what}"),
        };
        Ok(StmtKind::Assert { cond, msg })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = self.stmt_kind()?;
        Ok(Stmt { kind, span: start.to(self.tokens[self.pos - 1].span) })
    }

    /// Parses one statement, including its `;` when it needs one.
    fn stmt_kind(&mut self) -> PResult<StmtKind> {
        let end = |p: &mut Parser| p.expect(Tok::Semi).map(|_| ());
        if self.peek() == &Tok::Ident("yield".into()) {
            return Err(Diagnostic::new("`yield` is now `return`", self.span()).with_note("write `return x;`"));
        }
        if self.peek() == &Tok::Ident("assert".into()) && self.peek_at(1) == &Tok::LParen {
            return self.assert_stmt();
        }
        let kind = match self.peek().clone() {
            Tok::Burn => {
                self.bump();
                let name = self.ident()?;
                end(self)?;
                StmtKind::Burn(name)
            }
            Tok::Return => {
                self.bump();
                let value = if matches!(self.peek(), Tok::Semi | Tok::Comma) { None } else { Some(self.expr()?) };
                end(self)?;
                StmtKind::Return(value)
            }
            Tok::Break => {
                self.bump();
                end(self)?;
                StmtKind::Break
            }
            Tok::Continue => {
                self.bump();
                end(self)?;
                StmtKind::Continue
            }
            Tok::Final => {
                self.bump();
                if !self.at_declaration() {
                    return Err(self.unexpected("a variable declaration after `final`, like `final x: int = 5;`"));
                }
                let mut kind = self.let_stmt()?;
                if let StmtKind::Let { is_final, .. } = &mut kind {
                    *is_final = true;
                }
                end(self)?;
                kind
            }
            Tok::Lent => {
                return Err(Diagnostic::new("`lent` is not used any more", self.span())
                    .with_note("a variable that borrows is `atual: &No = &lista;` (an array: `&v[]: int = &obj.items;`)"));
            }
            // `&v[]: int = &obj.items;`: an array variable that only borrows.
            Tok::Amp if matches!(self.peek_at(1), Tok::Ident(_)) && self.peek_at(2) == &Tok::LBracket => {
                let at = self.bump().span;
                let mut kind = self.let_stmt()?;
                if let StmtKind::Let { ty: Some(t), .. } = &mut kind {
                    let Type::Own(b) = t.ty else { unreachable!("`x[]` is an array") };
                    t.ty = Type::Lent(b);
                } else {
                    return Err(Diagnostic::new("a borrowed array needs its type: `&v[]: int = &obj.items;`", at));
                }
                end(self)?;
                kind
            }
            Tok::Tri => return self.tri(),
            Tok::If => return self.if_stmt(),
            Tok::Match => return self.match_stmt(),
            Tok::While => {
                self.bump();
                let cond = self.head_expr()?;
                let body = self.block()?;
                return Ok(StmtKind::While { cond, body });
            }
            Tok::Walk => return self.walk(),
            Tok::For => return self.for_stmt(),
            _ if self.at_declaration() => {
                let kind = self.let_stmt()?;
                end(self)?;
                kind
            }
            _ => {
                let kind = self.expr_or_assign()?;
                end(self)?;
                kind
            }
        };
        Ok(kind)
    }

    /// True at the start of a declaration: `x: T`, `x[]: T` or `sym x[]: T`.
    fn at_declaration(&self) -> bool {
        match (self.peek(), self.peek_at(1)) {
            (Tok::Sym, _) => true,
            (Tok::Ident(_), Tok::Colon) => true,
            (Tok::Ident(_), Tok::LBracket) => self.peek_at(2) == &Tok::RBracket,
            _ => false,
        }
    }

    fn let_stmt(&mut self) -> PResult<StmtKind> {
        let sym_span = self.span();
        let sym = self.eat(&Tok::Sym);
        let (name, ty) = self.typed_name(sym, sym_span)?;
        // `x: int;` declares without a value.
        if self.peek() == &Tok::Semi {
            return Ok(StmtKind::Let { is_final: false, name, ty: Some(ty), value: None, on_nat: None });
        }
        self.expect(Tok::Eq)?;
        let ty = Some(ty);
        let value = Some(self.expr()?);
        let on_nat = if self.eat(&Tok::On) {
            self.expect(Tok::NaT)?;
            Some(self.block()?)
        } else {
            None
        };
        Ok(StmtKind::Let { is_final: false, name, ty, value, on_nat })
    }

    fn expr_or_assign(&mut self) -> PResult<StmtKind> {
        let lhs = self.expr()?;
        let op_tok = self.peek().clone();
        let compound = match op_tok {
            Tok::Eq => None,
            Tok::PlusEq | Tok::PlusPlus => Some(BinOp::Add),
            Tok::MinusEq | Tok::MinusMinus => Some(BinOp::Sub),
            Tok::StarEq => Some(BinOp::Mul),
            Tok::SlashEq => Some(BinOp::Div),
            Tok::PercentEq => Some(BinOp::Mod),
            _ => return Ok(StmtKind::Expr(lhs)),
        };
        let op_span = self.bump().span;
        let target = lhs.clone();
        // The target is a variable, `v[i]`, `p.x`, or a chain of them (`a[i].b`).
        fn is_place(e: &Expr) -> bool {
            match &e.kind {
                ExprKind::Var(_) => true,
                ExprKind::Index(base, _) | ExprKind::Member(base, _) => is_place(base),
                _ => false,
            }
        }
        if !is_place(&lhs) {
            return Err(Diagnostic::new("cannot assign to this expression", lhs.span)
                .with_note("the left side of `=` must be a variable, `array[index]` or a field like `p.x`"));
        }
        let span = lhs.span;
        let value = match (compound, op_tok) {
            (None, _) => self.expr()?,
            (Some(op), Tok::PlusPlus | Tok::MinusMinus) => {
                let one = Expr { kind: ExprKind::Num(Word::ONE), span: op_span };
                Expr { kind: ExprKind::Binary(op, Box::new(target), Box::new(one)), span: span.to(op_span) }
            }
            (Some(op), _) => {
                let rhs = self.expr()?;
                let span = span.to(rhs.span);
                Expr { kind: ExprKind::Binary(op, Box::new(target), Box::new(rhs)), span }
            }
        };
        Ok(StmtKind::Assign { target: lhs, value })
    }

    fn if_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::If)?;
        let cond = self.head_expr()?;
        let then = self.block()?;
        let otherwise = if !self.eat(&Tok::Else) {
            None
        } else if self.peek() == &Tok::If {
            let start = self.span();
            let kind = self.if_stmt()?;
            let span = start.to(self.tokens[self.pos - 1].span);
            Some(Block { stmts: vec![Stmt { kind, span }], span })
        } else {
            Some(self.block()?)
        };
        Ok(StmtKind::If { cond, then, otherwise })
    }

    /// A `tri`/`match` arm body: `{ ... }` or one statement (with its `;`).
    fn arm_body(&mut self) -> PResult<Block> {
        if self.peek() == &Tok::LBrace {
            return self.block();
        }
        let start = self.span();
        let kind = self.stmt_kind()?;
        let span = start.to(self.tokens[self.pos - 1].span);
        Ok(Block { stmts: vec![Stmt { kind, span }], span })
    }

    fn match_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::Match)?;
        let scrut = self.head_expr()?;
        self.expect(Tok::LBrace)?;
        let mut arms = Vec::new();
        while self.peek() != &Tok::RBrace {
            let start = self.span();
            let mut pats = Vec::new();
            if !self.eat(&Tok::Else) {
                loop {
                    pats.push(self.expr()?);
                    if !self.eat(&Tok::Pipe) {
                        break;
                    }
                }
            }
            let span = start.to(self.tokens[self.pos - 1].span);
            self.expect(Tok::FatArrow)?;
            let body = self.arm_body()?;
            arms.push(MatchArm { pats, body, span });
        }
        self.expect(Tok::RBrace)?;
        Ok(StmtKind::Match { scrut, arms })
    }

    fn tri(&mut self) -> PResult<StmtKind> {
        let tri_span = self.expect(Tok::Tri)?;
        let head = self.head_expr()?;
        self.expect(Tok::LBrace)?;
        let mut arms: [Option<Block>; 3] = [None, None, None];
        while self.peek() != &Tok::RBrace {
            let label_span = self.span();
            let (slot, label) = match self.peek() {
                Tok::Minus => (0, "-"),
                Tok::Num(w) if *w == Word::ZERO => (1, "0"),
                Tok::Plus => (2, "+"),
                _ => return Err(self.unexpected("a `tri` arm (`-`, `0` or `+`)")),
            };
            self.bump();
            self.expect(Tok::FatArrow)?;
            let body = self.arm_body()?;
            if arms[slot].replace(body).is_some() {
                return Err(Diagnostic::new(format!("the `{label}` arm appears twice"), label_span));
            }
        }
        let close = self.expect(Tok::RBrace)?;
        let [neg, zero, pos] = arms;
        let missing: Vec<&str> = [(&neg, "-"), (&zero, "0"), (&pos, "+")]
            .iter()
            .filter(|(a, _)| a.is_none())
            .map(|(_, l)| *l)
            .collect();
        if !missing.is_empty() {
            return Err(Diagnostic::new(
                format!("`tri` is missing the {} arm(s)", missing.join(", ")),
                tri_span.to(close),
            )
            .with_note("every tri needs exactly three arms: `-`, `0` and `+`"));
        }
        Ok(StmtKind::Tri { head, neg: neg.unwrap(), zero: zero.unwrap(), pos: pos.unwrap() })
    }

    /// `for i: int = 0; i < n; i++ { ... }`
    fn for_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::For)?;
        let start = self.span();
        // The counter is always declared here, with its type, so it lives only
        // inside the loop: `for i: int = 0; ...`.
        let typed = matches!((self.peek(), self.peek_at(1)), (Tok::Ident(_), Tok::Colon));
        if !typed {
            let note = match (self.peek(), self.peek_at(1)) {
                (_, Tok::Eq) => {
                    "a `for` cannot reuse an outside variable: declare a new one, as in `for i: int = 0; ...`".to_string()
                }
                (Tok::Ident(n), Tok::Semi) => {
                    format!("`{n}` alone declares nothing; to start from its value: `for i: int = {n}; ...`")
                }
                _ => "e.g. `for i: int = 0; i < 10; i++ { ... }`".to_string(),
            };
            return Err(Diagnostic::new("`for` must start by declaring its counter with a type", start).with_note(note));
        }
        let init = self.let_stmt()?;
        let init = Stmt { kind: init, span: start.to(self.tokens[self.pos - 1].span) };
        self.expect(Tok::Semi)?;
        let cond = self.head_expr()?;
        self.expect(Tok::Semi)?;
        let start = self.span();
        if self.at_declaration() {
            return Err(Diagnostic::new("the last part of `for` must change a variable, not declare one", start)
                .with_note("e.g. `i++`, `i += 2` or `i = i * 3`"));
        }
        let outer = std::mem::replace(&mut self.in_head, true);
        let step = self.expr_or_assign();
        self.in_head = outer;
        let step = Stmt { kind: step?, span: start.to(self.tokens[self.pos - 1].span) };
        let body = self.block()?;
        Ok(StmtKind::For { init: Box::new(init), cond, step: Box::new(step), body })
    }

    fn walk(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::Walk)?;
        let var = self.ident()?;
        self.expect(Tok::In)?;
        let range = if self.eat(&Tok::Span) {
            Range::Span(self.head_expr()?)
        } else {
            let a = self.head_expr()?;
            self.expect(Tok::DotDot)?;
            if self.peek() == &Tok::LBrace {
                return Err(self.unexpected("the end of the range after `..`").with_note("ranges have both ends: `walk i in 3..10 { ... }`"));
            }
            Range::Between(a, self.head_expr()?)
        };
        let body = self.block()?;
        Ok(StmtKind::Walk { var, range, body })
    }

    // ---- expressions ----

    pub fn expr(&mut self) -> PResult<Expr> {
        self.nest()?;
        let e = self.binary(0)?;
        self.depth -= 1;
        Ok(e)
    }

    /// An expression inside `( )` or `[ ]`, where `{` is a literal again.
    fn nested_expr(&mut self) -> PResult<Expr> {
        let outer = std::mem::replace(&mut self.in_head, false);
        let e = self.expr();
        self.in_head = outer;
        e
    }

    /// An expression right before a body block (`if x { ... }`).
    fn head_expr(&mut self) -> PResult<Expr> {
        let outer = std::mem::replace(&mut self.in_head, true);
        let e = self.expr();
        self.in_head = outer;
        e
    }

    /// Binary operators by level; level 0 binds loosest.
    fn binary(&mut self, level: usize) -> PResult<Expr> {
        const LEVELS: &[&[(Tok, BinOp)]] = &[
            &[(Tok::OrOr, BinOp::Or)],
            &[(Tok::AndAnd, BinOp::And)],
            &[
                (Tok::Spaceship, BinOp::Cmp3),
                (Tok::EqEq, BinOp::Eq),
                (Tok::NotEq, BinOp::Ne),
                (Tok::Lt, BinOp::Lt),
                (Tok::Gt, BinOp::Gt),
                (Tok::Le, BinOp::Le),
                (Tok::Ge, BinOp::Ge),
            ],
            &[(Tok::Plus, BinOp::Add), (Tok::Minus, BinOp::Sub)],
            &[(Tok::Star, BinOp::Mul), (Tok::Slash, BinOp::Div), (Tok::Percent, BinOp::Mod)],
            &[(Tok::Shl, BinOp::Shl), (Tok::Shr, BinOp::Shr)],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut lhs = self.binary(level + 1)?;
        // `f is Circulo` sits with the comparisons (`a is B && ...`).
        if level == 2 && self.eat(&Tok::Is) {
            let class = self.ident()?;
            let span = lhs.span.to(class.span);
            return Ok(Expr { kind: ExprKind::Is(Box::new(lhs), class), span });
        }
        // Each operator in a chain like `a + b + c` makes the tree one level deeper.
        let mut chain = 0;
        while let Some((_, op)) = ops.iter().find(|(t, _)| t == self.peek()) {
            self.nest()?;
            chain += 1;
            let op_span = self.bump().span;
            let rhs = self.binary(level + 1)?;
            if level == 2 && ops.iter().any(|(t, _)| t == self.peek()) {
                return Err(Diagnostic::new("comparisons cannot be chained", self.span())
                    .with_note("compare two values at a time: `a < b && b < c`"));
            }
            let span = lhs.span.to(rhs.span).to(op_span);
            lhs = Expr { kind: ExprKind::Binary(*op, Box::new(lhs), Box::new(rhs)), span };
        }
        self.depth -= chain;
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        // Only prefix operators and casts nest (`- - x`, `(int) (real) x`).
        let prefix = matches!(self.peek(), Tok::Minus | Tok::Plus | Tok::Bang)
            || (self.peek() == &Tok::LParen
                && matches!(self.peek_at(1), Tok::Trit | Tok::Tryte | Tok::Int | Tok::Real | Tok::Bool)
                && self.peek_at(2) == &Tok::RParen);
        if !prefix {
            return self.unary_inner();
        }
        self.nest()?;
        let e = self.unary_inner()?;
        self.depth -= 1;
        Ok(e)
    }

    fn unary_inner(&mut self) -> PResult<Expr> {
        let start = self.span();
        let op = match self.peek() {
            Tok::Bang => Some(UnOp::Not),
            Tok::Minus | Tok::Plus => {
                // A lone `-` or `+` before a delimiter is a trit literal: `x: trit = +;`
                if matches!(self.peek_at(1), Tok::Semi | Tok::Comma | Tok::RParen | Tok::RBracket | Tok::RBrace | Tok::LBrace) {
                    let v = if self.bump().tok == Tok::Minus { Word::NEG_ONE } else { Word::ONE };
                    return Ok(Expr { kind: ExprKind::Num(v), span: start });
                }
                (self.peek() == &Tok::Minus).then_some(UnOp::Neg)
            }
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let inner = self.unary()?;
            let span = start.to(inner.span);
            return Ok(Expr { kind: ExprKind::Unary(op, Box::new(inner)), span });
        }
        if self.peek() == &Tok::Plus {
            self.bump();
            return self.unary();
        }
        // C-style cast `(T) x`: type names are keywords, so `(` + scalar + `)` is never a grouping.
        if self.peek() == &Tok::LParen
            && matches!(self.peek_at(1), Tok::Trit | Tok::Tryte | Tok::Int | Tok::Real | Tok::Bool)
            && self.peek_at(2) == &Tok::RParen
        {
            self.bump();
            let s = if self.eat(&Tok::Bool) { Type::Bool } else { Type::Scalar(self.scalar().expect("checked above")) };
            self.bump();
            let inner = self.unary()?;
            let span = start.to(inner.span);
            return Ok(Expr { kind: ExprKind::Cast(Box::new(inner), s), span });
        }
        self.power()
    }

    fn power(&mut self) -> PResult<Expr> {
        let base = self.postfix()?;
        if !self.eat(&Tok::StarStar) {
            return Ok(base);
        }
        self.nest()?;
        let exp = self.unary()?; // right-associative: 2 ** 3 ** 2 == 2 ** 9
        self.depth -= 1;
        let span = base.span.to(exp.span);
        Ok(Expr { kind: ExprKind::Binary(BinOp::Pow, Box::new(base), Box::new(exp)), span })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat(&Tok::LBracket) {
                let idx = self.nested_expr()?;
                let close = self.expect(Tok::RBracket)?;
                let span = e.span.to(close);
                e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), span };
            } else if self.peek() == &Tok::Dot || self.peek() == &Tok::QuestionDot {
                // `a?.b`: the member is read from a hidden variable that holds `a`.
                let safe = self.bump().tok == Tok::QuestionDot;
                let field = self.ident()?;
                let (recv, var) = if safe {
                    self.safe_vars += 1;
                    let var = format!("$safe{}", self.safe_vars);
                    (Expr { kind: ExprKind::Var(var.clone()), span: e.span }, Some(var))
                } else {
                    (e.clone(), None)
                };
                let (kind, end) = if self.eat(&Tok::LParen) {
                    let (args, close) = self.call_args()?;
                    (ExprKind::MethodCall { recv: Box::new(recv), name: field, args }, close)
                } else {
                    let end = field.span;
                    (ExprKind::Member(Box::new(recv), field), end)
                };
                let span = e.span.to(end);
                let rest = Expr { kind, span };
                e = match var {
                    Some(var) => Expr { kind: ExprKind::Safe { recv: Box::new(e), var, rest: Box::new(rest) }, span },
                    None => rest,
                };
            } else {
                return Ok(e);
            }
        }
    }

    /// Arguments after `(`, up to and including `)`. `a..b` is allowed (for `replace`).
    fn call_args(&mut self) -> PResult<(Vec<Expr>, Span)> {
        let mut args = Vec::new();
        while self.peek() != &Tok::RParen {
            let a = self.nested_expr()?;
            let a = if self.eat(&Tok::DotDot) {
                let b = self.nested_expr()?;
                let span = a.span.to(b.span);
                Expr { kind: ExprKind::Range(Box::new(a), Box::new(b)), span }
            } else {
                a
            };
            args.push(a);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        let close = self.expect(Tok::RParen)?;
        Ok((args, close))
    }

    /// `Name { a: 1, b: 2 }`, after the name.
    fn struct_lit(&mut self, name: Ident, start: Span) -> PResult<Expr> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        while self.peek() != &Tok::RBrace {
            let field = self.ident()?;
            self.expect(Tok::Colon)?;
            fields.push((field, self.nested_expr()?));
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        let close = self.expect(Tok::RBrace)?;
        Ok(Expr { kind: ExprKind::StructLit { name, fields }, span: start.to(close) })
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let kind = match self.peek().clone() {
            Tok::Num(w) => {
                self.bump();
                ExprKind::Num(w)
            }
            Tok::RealLit(r) => {
                self.bump();
                ExprKind::Real(r)
            }
            Tok::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::Null => {
                self.bump();
                ExprKind::NaT
            }
            Tok::NaT => {
                return Err(Diagnostic::new("`NaT` is written `null`", self.span())
                    .with_note("`x: int? = null;`, `if x == null { ... }`; `NaT` is only used in `on NaT`"));
            }
            Tok::True | Tok::False => ExprKind::Bool(self.bump().tok == Tok::True),
            Tok::LParen => {
                self.bump();
                let inner = self.nested_expr()?;
                let close = self.expect(Tok::RParen)?;
                return Ok(Expr { kind: inner.kind, span: start.to(close) });
            }
            Tok::LBrace if self.in_head => return Err(self.unexpected("an expression")),
            Tok::LBrace => {
                self.bump();
                let mut items = Vec::new();
                while self.peek() != &Tok::RBrace {
                    items.push(self.expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                let close = self.expect(Tok::RBrace)?;
                return Ok(Expr { kind: ExprKind::ArrayLit(items), span: start.to(close) });
            }
            Tok::Claim => {
                self.bump();
                let size = self.expr()?;
                let span = start.to(size.span);
                return Ok(Expr { kind: ExprKind::Claim(Box::new(size)), span });
            }
            Tok::Give => {
                // `give x`, or `give a.b` (an object taken out of a field).
                self.bump();
                let name = self.ident()?;
                let mut path = Expr { kind: ExprKind::Var(name.name.clone()), span: name.span };
                while self.peek() == &Tok::Dot && matches!(self.peek_at(1), Tok::Ident(_)) && self.peek_at(2) != &Tok::LParen {
                    self.bump();
                    let field = self.ident()?;
                    let span = path.span.to(field.span);
                    path = Expr { kind: ExprKind::Member(Box::new(path), field), span };
                }
                let span = start.to(path.span);
                if let ExprKind::Var(_) = path.kind {
                    return Ok(Expr { kind: ExprKind::Give(name), span });
                }
                return Ok(Expr { kind: ExprKind::GiveField(Box::new(path)), span });
            }
            Tok::Lend => {
                return Err(Diagnostic::new("`&x` is now written `&x`", self.span())
                    .with_note("e.g. `ver(&c)`, `soma(&lista.proximo)`, `mostrar(&formas[i])`"));
            }
            Tok::Amp => {
                // `&x`, `&a.b.c` (an array or object held in a field)
                // or `&v[i]` (an object held in an array).
                self.bump();
                let mut path = Expr { kind: ExprKind::Var(self.ident()?.name), span: self.tokens[self.pos - 1].span };
                loop {
                    if self.peek() == &Tok::Dot && matches!(self.peek_at(1), Tok::Ident(_)) && self.peek_at(2) != &Tok::LParen {
                        self.bump();
                        let field = self.ident()?;
                        let span = path.span.to(field.span);
                        path = Expr { kind: ExprKind::Member(Box::new(path), field), span };
                    } else if self.eat(&Tok::LBracket) {
                        let idx = self.expr()?;
                        let close = self.expect(Tok::RBracket)?;
                        let span = path.span.to(close);
                        path = Expr { kind: ExprKind::Index(Box::new(path), Box::new(idx)), span };
                    } else {
                        break;
                    }
                }
                let span = start.to(path.span);
                return Ok(Expr { kind: ExprKind::Lend(Box::new(path)), span });
            }
            Tok::Ident(name) => {
                let mut name = Ident { name, span: self.bump().span };
                if let Some(g) = self.generic_ref(&name.name) {
                    name = Ident { name: g.to_string(), span: start.to(self.tokens[self.pos - 1].span) };
                }
                if self.eat(&Tok::LParen) {
                    let (args, close) = self.call_args()?;
                    return Ok(Expr { kind: ExprKind::Call { name, args }, span: start.to(close) });
                }
                // `Name { field: ...`: a struct value (never in a head, where `{` opens the body).
                if !self.in_head
                    && self.peek() == &Tok::LBrace
                    && matches!(self.peek_at(1), Tok::Ident(_))
                    && self.peek_at(2) == &Tok::Colon
                {
                    return self.struct_lit(name, start);
                }
                ExprKind::Var(name.name)
            }
            _ => return Err(self.unexpected("an expression")),
        };
        Ok(Expr { kind, span: start })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn program(src: &str) -> Program {
        parse(lex(src).unwrap()).unwrap()
    }

    fn parse_err(src: &str) -> Diagnostic {
        parse(lex(src).unwrap()).unwrap_err()
    }

    /// Parses `src` as the body of `main` and returns its statements.
    fn body(src: &str) -> Vec<StmtKind> {
        let p = program(&format!("proc main() {{ {src} }}"));
        let Item::Proc(f) = &p.items[0] else { panic!() };
        f.body.stmts.iter().map(|s| s.kind.clone()).collect()
    }

    /// Compact S-expression form of an expression, for readable tests.
    fn sx(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Num(w) => w.to_string(),
            ExprKind::Real(r) => r.to_string(),
            ExprKind::Char(c) => format!("'{c}"),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::NaT => "NaT".into(),
            ExprKind::Var(v) => v.clone(),
            ExprKind::Unary(UnOp::Neg, a) => format!("(neg {})", sx(a)),
            ExprKind::Unary(UnOp::Not, a) => format!("(! {})", sx(a)),
            ExprKind::Binary(op, a, b) => format!("({} {} {})", op.symbol(), sx(a), sx(b)),
            ExprKind::Cast(a, s) => format!("(cast {s} {})", sx(a)),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Member(a, f) => format!("{}.{}", sx(a), f.name),
            ExprKind::ArrayLit(items) => format!("{{{}}}", items.iter().map(sx).collect::<Vec<_>>().join(" ")),
            ExprKind::Index(a, i) => format!("{}[{}]", sx(a), sx(i)),
            ExprKind::Call { name, args } => {
                format!("{}({})", name.name, args.iter().map(sx).collect::<Vec<_>>().join(", "))
            }
            ExprKind::Claim(n) => format!("(claim {})", sx(n)),
            ExprKind::Give(n) => format!("(give {})", n.name),
            ExprKind::GiveField(p) => format!("(give {})", sx(p)),
            ExprKind::Is(a, c) => format!("(is {} {})", sx(a), c.name),
            ExprKind::Safe { recv, rest, .. } => format!("(?. {} {})", sx(recv), sx(rest)),
            ExprKind::Lend(n) => format!("(&{})", sx(n)),
            ExprKind::MethodCall { recv, name, args } => {
                format!("{}.{}({})", sx(recv), name.name, args.iter().map(sx).collect::<Vec<_>>().join(", "))
            }
            ExprKind::Range(a, b) => format!("{}..{}", sx(a), sx(b)),
            ExprKind::StructLit { name, fields } => {
                let fs: Vec<String> = fields.iter().map(|(f, e)| format!("{}: {}", f.name, sx(e))).collect();
                format!("{} {{{}}}", name.name, fs.join(", "))
            }
        }
    }

    fn ex(src: &str) -> String {
        match &body(&format!("{src};"))[0] {
            StmtKind::Expr(e) => sx(e),
            other => panic!("not an expression: {other:?}"),
        }
    }

    #[test]
    fn precedence() {
        assert_eq!(ex("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(ex("1 - 2 - 3"), "(- (- 1 2) 3)");
        assert_eq!(ex("-2 ** 2"), "(neg (** 2 2))");
        assert_eq!(ex("2 ** 3 ** 2"), "(** 2 (** 3 2))");
        assert_eq!(ex("a <=> b && c || d"), "(|| (&& (<=> a b) c) d)");
        assert_eq!(ex("x << 2 * 3"), "(* (<< x 2) 3)");
        assert_eq!(ex("(real) a / (real) b"), "(/ (cast real a) (cast real b))");
        assert_eq!(ex("(trit) -x"), "(cast trit (neg x))");
        assert_eq!(ex("(int)(a + b) * 2"), "(* (cast int (+ a b)) 2)");
        assert_eq!(ex("!v[i - 1] % 3"), "(% (! v[(- i 1)]) 3)");
        assert_eq!(ex("a + 1 < b && !c || d == e"), "(|| (&& (< (+ a 1) b) (! c)) (== d e))");
        assert_eq!(ex("f(&b, give c, 'A', \"hi\")"), "f((&b), (give c), '1, \"hi\")");
        assert_eq!(ex("(1 + 2) * 3"), "(* (+ 1 2) 3)");
    }

    #[test]
    fn trit_literals() {
        let stmts = body("t: trit = +; u: trit = -; f(-, +);");
        let StmtKind::Let { value: Some(value), .. } = &stmts[0] else { panic!() };
        assert_eq!(sx(value), "1");
        let StmtKind::Let { value: Some(value), .. } = &stmts[1] else { panic!() };
        assert_eq!(sx(value), "-1");
        let StmtKind::Expr(e) = &stmts[2] else { panic!() };
        assert_eq!(sx(e), "f(-1, 1)");
    }

    #[test]
    fn declarations() {
        let stmts = body("acc: int = 0; y[]: int = claim 2 * k + 1; s: int = f() on NaT { return; };");
        let StmtKind::Let { name, ty, .. } = &stmts[0] else { panic!() };
        assert_eq!((name.name.as_str(), ty.unwrap().ty), ("acc", Type::Scalar(Scalar::Int)));
        let StmtKind::Let { ty: Some(_), value: Some(value), .. } = &stmts[1] else { panic!() };
        assert_eq!(sx(value), "(claim (+ (* 2 k) 1))");
        let StmtKind::Let { on_nat: Some(b), .. } = &stmts[2] else { panic!() };
        assert_eq!(b.stmts[0].kind, StmtKind::Return(None));
    }

    #[test]
    fn assignment() {
        let stmts = body("x = 1; b[-1] = 2; add(1, 2); x += 3; b[0]++;");
        assert!(matches!(&stmts[0], StmtKind::Assign { target, .. } if matches!(target.kind, ExprKind::Var(_))));
        assert!(matches!(&stmts[1], StmtKind::Assign { target, .. } if matches!(target.kind, ExprKind::Index(..))));
        assert!(matches!(&stmts[2], StmtKind::Expr(Expr { kind: ExprKind::Call { .. }, .. })));
        let StmtKind::Assign { value, .. } = &stmts[3] else { panic!() };
        assert_eq!(sx(value), "(+ x 3)");
        let StmtKind::Assign { value, .. } = &stmts[4] else { panic!() };
        assert_eq!(sx(value), "(+ b[0] 1)");
    }

    #[test]
    fn tri_and_walk() {
        let stmts = body(
            "tri s <=> 10 { + => print(\"big\"); - => { x = 1; } 0 => return; }
             walk i in span 2 { b[i] = i; }
             walk j in -1..k + 1 { break; }",
        );
        let StmtKind::Tri { head, neg, zero, pos } = &stmts[0] else { panic!() };
        assert_eq!(sx(head), "(<=> s 10)");
        assert_eq!(neg.stmts.len(), 1);
        assert_eq!(zero.stmts[0].kind, StmtKind::Return(None));
        assert!(matches!(&pos.stmts[0].kind, StmtKind::Expr(_)));
        assert!(matches!(&stmts[1], StmtKind::Walk { range: Range::Span(_), .. }));
        let StmtKind::Walk { range: Range::Between(a, b), .. } = &stmts[2] else { panic!() };
        assert_eq!((sx(a), sx(b)), ("(neg 1)".to_string(), "(+ k 1)".to_string()));
    }

    #[test]
    fn items() {
        let p = program("const LIMIT: int = 0vMM;\nproc sum(sym &v[]: int, k: int) -> int { return k; }");
        assert!(matches!(&p.items[0], Item::Const(c) if c.name.name == "LIMIT"));
        let Item::Proc(f) = &p.items[1] else { panic!() };
        assert_eq!(f.params[0].ty.ty, Type::Lent(Buf { elem: Elem::Scalar(Scalar::Int), sym: true }));
        assert_eq!(f.ret.unwrap().ty, Type::Scalar(Scalar::Int));
    }

    #[test]
    fn errors() {
        assert!(parse_err("proc main() { final 5; }").message.contains("after `final`"));
        assert!(parse_err("proc main() { t: trit = 1 < 2 < 3; }").message.contains("cannot be chained"));
        let d = parse_err("proc main() { x: int = 1\n y: int = 2; }");
        assert_eq!(d.message, "expected `;`, found `y`");
        assert_eq!(d.span.start, "proc main() { x: int = 1".len());

        assert!(parse_err("proc main() { tri x { - => a; + => b; } }").message.contains("missing the 0 arm"));
        assert!(parse_err("proc main() { tri x { - => a; - => b; 0 => c; + => d; } }").message.contains("appears twice"));
        assert!(parse_err("proc main() { x + 1 = 3; }").message.contains("cannot assign"));
        assert!(parse_err("proc main() { b[]: int[] = claim 3; }").message.contains("an array cannot hold arrays"));
        let e = parse_err("proc main() { walk i in 3.. { print(i); } }");
        assert!(e.message.contains("the end of the range"), "{}", e.message);
        assert!(parse_err("proc main() { if {1} { } }").message.contains("expected an expression, found `{`"));
        assert!(parse_err("proc f() -> &int[] { } proc main() {}").message.contains("before its name"));
        assert!(parse_err("proc main() { x: trit = a <=> b <=> c; }").message.contains("cannot be chained"));
        assert!(parse_err("x: int = 1;").message.contains("expected `proc`, `const`, `enum`, `struct`, `class` or `interface`"));
        assert!(parse_err("proc main() { x: int = ; }").message.contains("expected an expression, found `;`"));
    }

    #[test]
    fn spec_example_parses() {
        let src = r#"
proc sum(sym &v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int = sum(&b, 2) on NaT { burn b; return -1; };
    s = s << 1;
    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    return 0;
}
"#;
        let p = program(src);
        let Item::Proc(main) = &p.items[1] else { panic!() };
        assert_eq!(main.name.name, "main");
        assert_eq!(main.body.stmts.len(), 7);
    }
}
