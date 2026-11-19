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
    let mut p = Parser { tokens, pos: 0, in_head: false, depth: 0 };
    let mut items = Vec::new();
    while p.peek() != &Tok::Eof {
        items.push(p.item()?);
    }
    Ok(Program { items })
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Inside the head of `if`/`while`/`match`/`tri`/`walk`: a `{` there
    /// opens the body, never an array literal (the same rule as Rust).
    in_head: bool,
    /// How deep the code being parsed is nested (blocks, parentheses,
    /// operators). Bounded so deep input gives an error, not a stack overflow.
    depth: u32,
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
            _ => Err(self.unexpected("`proc`, `const`, `enum` or `struct`")),
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

    fn proc_decl(&mut self) -> PResult<ProcDecl> {
        self.expect(Tok::Proc)?;
        let name = self.ident()?;
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            // `lent v[]: int` borrows an array, `own v[]: int` takes it (`give`).
            let mode_span = self.span();
            let mode = match self.peek() {
                Tok::Lent | Tok::Own => Some(self.bump().tok.clone()),
                _ => None,
            };
            let sym_span = self.span();
            let sym = self.eat(&Tok::Sym);
            let (pname, mut ty) = self.typed_name(sym, sym_span)?;
            match (mode, ty.ty) {
                (Some(Tok::Lent), Type::Own(b)) => ty.ty = Type::Lent(b),
                (Some(Tok::Own), Type::Own(_)) => {}
                (None, Type::Own(_)) => {
                    return Err(Diagnostic::new("an array parameter needs `lent` or `own`", pname.span)
                        .with_note(format!("`lent {0}[]: ...` borrows it, `own {0}[]: ...` takes it", pname.name)));
                }
                (Some(_), _) => {
                    return Err(Diagnostic::new("`lent` and `own` only apply to arrays", mode_span)
                        .with_note(format!("write `{}[]: ...`", pname.name)));
                }
                (None, _) => {}
            }
            params.push(Param { name: pname, ty });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        let body = self.block()?;
        Ok(ProcDecl { name, params, ret, body })
    }

    fn elem(&self, s: Scalar, span: Span) -> PResult<Elem> {
        match s {
            Scalar::Tryte => Ok(Elem::Tryte),
            Scalar::Int => Ok(Elem::Int),
            _ => Err(Diagnostic::new(format!("arrays hold trytes or ints, not {s}"), span)),
        }
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
        let Type::Scalar(s) = ty.ty else {
            return Err(Diagnostic::new(format!("arrays hold trytes or ints, not {}", ty.ty), ty.span));
        };
        let elem = self.elem(s, ty.span)?;
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
        let sym = self.eat(&Tok::Sym);
        if let Some(s) = self.scalar() {
            if self.peek() == &Tok::LBracket && self.peek_at(1) == &Tok::RBracket {
                self.bump();
                self.bump();
                let elem = self.elem(s, start)?;
                return Ok(TypeExpr { ty: Type::Own(Buf { elem, sym }), span: start.to(self.tokens[self.pos - 1].span) });
            }
            if sym {
                return Err(Diagnostic::new("`sym` only applies to arrays", start).with_note("write `sym int[]`"));
            }
            return Ok(TypeExpr { ty: Type::Scalar(s), span: start });
        }
        if sym {
            return Err(self.unexpected("`tryte[]` or `int[]` after `sym`"));
        }
        if let Tok::Ident(name) = self.peek() {
            let ty = Type::Named(intern(name));
            self.bump();
            return Ok(TypeExpr { ty, span: start });
        }
        let ty = match self.peek() {
            Tok::Text => {
                self.bump();
                Type::Text
            }
            Tok::Own | Tok::Lent => {
                return Err(self
                    .unexpected("a type")
                    .with_note("arrays are declared on the name: `x[]: int`, or `lent v[]: int` for a parameter"));
            }
            _ => return Err(self.unexpected("a type")),
        };
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

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = self.stmt_kind()?;
        Ok(Stmt { kind, span: start.to(self.tokens[self.pos - 1].span) })
    }

    /// Parses one statement, including its `;` when it needs one.
    fn stmt_kind(&mut self) -> PResult<StmtKind> {
        let end = |p: &mut Parser| p.expect(Tok::Semi).map(|_| ());
        let kind = match self.peek().clone() {
            Tok::Burn => {
                self.bump();
                let name = self.ident()?;
                end(self)?;
                StmtKind::Burn(name)
            }
            Tok::Yield => {
                self.bump();
                let value = if matches!(self.peek(), Tok::Semi | Tok::Comma) { None } else { Some(self.expr()?) };
                end(self)?;
                StmtKind::Yield(value)
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

    /// True at the start of a declaration: `x: T`, `x := e`, `x[]: T` or `sym x[]: T`.
    fn at_declaration(&self) -> bool {
        match (self.peek(), self.peek_at(1)) {
            (Tok::Sym, _) => true,
            (Tok::Ident(_), Tok::Colon | Tok::ColonEq) => true,
            (Tok::Ident(_), Tok::LBracket) => self.peek_at(2) == &Tok::RBracket,
            _ => false,
        }
    }

    fn let_stmt(&mut self) -> PResult<StmtKind> {
        let sym_span = self.span();
        let sym = self.eat(&Tok::Sym);
        let (name, ty) = if !sym && self.peek_at(1) == &Tok::ColonEq {
            let name = self.ident()?;
            self.bump();
            (name, None)
        } else {
            let (name, ty) = self.typed_name(sym, sym_span)?;
            self.expect(Tok::Eq)?;
            (name, Some(ty))
        };
        let value = self.expr()?;
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
        // `p.a.b = ...`: peel the fields off, outermost last.
        let span = lhs.span;
        let mut fields = Vec::new();
        let mut base = lhs;
        while let ExprKind::Member(inner, field) = base.kind {
            fields.push(field);
            base = *inner;
        }
        fields.reverse();
        let place = match base.kind {
            ExprKind::Var(name) => Place { name: Ident { name, span: base.span }, index: None, fields, span },
            ExprKind::Index(arr, idx) => match arr.kind {
                ExprKind::Var(name) => Place { name: Ident { name, span: arr.span }, index: Some(*idx), fields, span },
                _ => return Err(Diagnostic::new("only a variable can be indexed on the left of `=`", arr.span)),
            },
            _ => {
                return Err(Diagnostic::new("cannot assign to this expression", span)
                    .with_note("the left side of `=` must be a variable, `array[index]` or a field like `p.x`"));
            }
        };
        let value = match (compound, op_tok) {
            (None, _) => self.expr()?,
            (Some(op), Tok::PlusPlus | Tok::MinusMinus) => {
                let one = Expr { kind: ExprKind::Num(Word::ONE), span: op_span };
                Expr { kind: ExprKind::Binary(op, Box::new(target), Box::new(one)), span: place.span.to(op_span) }
            }
            (Some(op), _) => {
                let rhs = self.expr()?;
                let span = place.span.to(rhs.span);
                Expr { kind: ExprKind::Binary(op, Box::new(target), Box::new(rhs)), span }
            }
        };
        Ok(StmtKind::Assign { place, value })
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

    /// `for i := 0; i < n; i++ { ... }`
    fn for_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::For)?;
        let start = self.span();
        // The counter is always declared here, with its type, so it lives only
        // inside the loop: `for i: int = 0; ...`.
        let typed = matches!((self.peek(), self.peek_at(1)), (Tok::Ident(_), Tok::Colon));
        if !typed {
            let note = match (self.peek(), self.peek_at(1)) {
                (_, Tok::ColonEq) => "`:=` is not allowed here: write the type, as in `for i: int = 0; ...`".to_string(),
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
                && matches!(self.peek_at(1), Tok::Trit | Tok::Tryte | Tok::Int | Tok::Real)
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
            && matches!(self.peek_at(1), Tok::Trit | Tok::Tryte | Tok::Int | Tok::Real)
            && self.peek_at(2) == &Tok::RParen
        {
            self.bump();
            let s = self.scalar().expect("checked above");
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
            } else if self.eat(&Tok::Dot) {
                let field = self.ident()?;
                let span = e.span.to(field.span);
                e = Expr { kind: ExprKind::Member(Box::new(e), field), span };
            } else {
                return Ok(e);
            }
        }
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
            Tok::NaT => {
                self.bump();
                ExprKind::NaT
            }
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
            Tok::Give | Tok::Lend => {
                let give = self.bump().tok == Tok::Give;
                let name = self.ident()?;
                let span = start.to(name.span);
                let kind = if give { ExprKind::Give(name) } else { ExprKind::Lend(name) };
                return Ok(Expr { kind, span });
            }
            Tok::Ident(name) => {
                let name = Ident { name, span: self.bump().span };
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    while self.peek() != &Tok::RParen {
                        args.push(self.nested_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    let close = self.expect(Tok::RParen)?;
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
            ExprKind::Cast(a, s) => format!("(cast {s:?} {})", sx(a)),
            ExprKind::Member(a, f) => format!("{}.{}", sx(a), f.name),
            ExprKind::ArrayLit(items) => format!("{{{}}}", items.iter().map(sx).collect::<Vec<_>>().join(" ")),
            ExprKind::Index(a, i) => format!("{}[{}]", sx(a), sx(i)),
            ExprKind::Call { name, args } => {
                format!("{}({})", name.name, args.iter().map(sx).collect::<Vec<_>>().join(", "))
            }
            ExprKind::Claim(n) => format!("(claim {})", sx(n)),
            ExprKind::Give(n) => format!("(give {})", n.name),
            ExprKind::Lend(n) => format!("(lend {})", n.name),
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
        assert_eq!(ex("(real) a / (real) b"), "(/ (cast Real a) (cast Real b))");
        assert_eq!(ex("(trit) -x"), "(cast Trit (neg x))");
        assert_eq!(ex("(int)(a + b) * 2"), "(* (cast Int (+ a b)) 2)");
        assert_eq!(ex("!v[i - 1] % 3"), "(% (! v[(- i 1)]) 3)");
        assert_eq!(ex("a + 1 < b && !c || d == e"), "(|| (&& (< (+ a 1) b) (! c)) (== d e))");
        assert_eq!(ex("f(lend b, give c, 'A', \"hi\")"), "f((lend b), (give c), '1, \"hi\")");
        assert_eq!(ex("(1 + 2) * 3"), "(* (+ 1 2) 3)");
    }

    #[test]
    fn trit_literals() {
        let stmts = body("t: trit = +; u: trit = -; f(-, +);");
        let StmtKind::Let { value, .. } = &stmts[0] else { panic!() };
        assert_eq!(sx(value), "1");
        let StmtKind::Let { value, .. } = &stmts[1] else { panic!() };
        assert_eq!(sx(value), "-1");
        let StmtKind::Expr(e) = &stmts[2] else { panic!() };
        assert_eq!(sx(e), "f(-1, 1)");
    }

    #[test]
    fn declarations() {
        let stmts = body("acc: int = 0; y := claim 2 * k + 1; s: int = f() on NaT { yield; };");
        let StmtKind::Let { name, ty, .. } = &stmts[0] else { panic!() };
        assert_eq!((name.name.as_str(), ty.unwrap().ty), ("acc", Type::Scalar(Scalar::Int)));
        let StmtKind::Let { ty: None, value, .. } = &stmts[1] else { panic!() };
        assert_eq!(sx(value), "(claim (+ (* 2 k) 1))");
        let StmtKind::Let { on_nat: Some(b), .. } = &stmts[2] else { panic!() };
        assert_eq!(b.stmts[0].kind, StmtKind::Yield(None));
    }

    #[test]
    fn assignment() {
        let stmts = body("x = 1; b[-1] = 2; add(1, 2); x += 3; b[0]++;");
        assert!(matches!(&stmts[0], StmtKind::Assign { place, .. } if place.index.is_none()));
        assert!(matches!(&stmts[1], StmtKind::Assign { place, .. } if place.index.is_some()));
        assert!(matches!(&stmts[2], StmtKind::Expr(Expr { kind: ExprKind::Call { .. }, .. })));
        let StmtKind::Assign { value, .. } = &stmts[3] else { panic!() };
        assert_eq!(sx(value), "(+ x 3)");
        let StmtKind::Assign { value, .. } = &stmts[4] else { panic!() };
        assert_eq!(sx(value), "(+ b[0] 1)");
    }

    #[test]
    fn tri_and_walk() {
        let stmts = body(
            "tri s <=> 10 { + => print(\"big\"); - => { x = 1; } 0 => yield; }
             walk i in span 2 { b[i] = i; }
             walk j in -1..k + 1 { break; }",
        );
        let StmtKind::Tri { head, neg, zero, pos } = &stmts[0] else { panic!() };
        assert_eq!(sx(head), "(<=> s 10)");
        assert_eq!(neg.stmts.len(), 1);
        assert_eq!(zero.stmts[0].kind, StmtKind::Yield(None));
        assert!(matches!(&pos.stmts[0].kind, StmtKind::Expr(_)));
        assert!(matches!(&stmts[1], StmtKind::Walk { range: Range::Span(_), .. }));
        let StmtKind::Walk { range: Range::Between(a, b), .. } = &stmts[2] else { panic!() };
        assert_eq!((sx(a), sx(b)), ("(neg 1)".to_string(), "(+ k 1)".to_string()));
    }

    #[test]
    fn items() {
        let p = program("const LIMIT: int = 0vMM;\nproc sum(lent sym v[]: int, k: int) -> int { yield k; }");
        assert!(matches!(&p.items[0], Item::Const(c) if c.name.name == "LIMIT"));
        let Item::Proc(f) = &p.items[1] else { panic!() };
        assert_eq!(f.params[0].ty.ty, Type::Lent(Buf { elem: Elem::Int, sym: true }));
        assert_eq!(f.ret.unwrap().ty, Type::Scalar(Scalar::Int));
    }

    #[test]
    fn errors() {
        assert!(parse_err("proc main() { final 5; }").message.contains("after `final`"));
        assert!(parse_err("proc main() { t: trit = 1 < 2 < 3; }").message.contains("cannot be chained"));
        let d = parse_err("proc main() { x := 1\n y := 2; }");
        assert_eq!(d.message, "expected `;`, found `y`");
        assert_eq!(d.span.start, "proc main() { x := 1".len());

        assert!(parse_err("proc main() { tri x { - => a; + => b; } }").message.contains("missing the 0 arm"));
        assert!(parse_err("proc main() { tri x { - => a; - => b; 0 => c; + => d; } }").message.contains("appears twice"));
        assert!(parse_err("proc main() { x + 1 = 3; }").message.contains("cannot assign"));
        assert!(parse_err("proc main() { b[]: real = claim 3; }").message.contains("trytes or ints"));
        let e = parse_err("proc main() { walk i in 3.. { print(i); } }");
        assert!(e.message.contains("the end of the range"), "{}", e.message);
        assert!(parse_err("proc main() { if {1} { } }").message.contains("expected an expression, found `{`"));
        assert!(parse_err("proc f() -> lent int[] { } proc main() {}").note.unwrap().contains("declared on the name"));
        assert!(parse_err("proc main() { x := a <=> b <=> c; }").message.contains("cannot be chained"));
        assert!(parse_err("x := 1;").message.contains("expected `proc`, `const`, `enum` or `struct`"));
        assert!(parse_err("proc main() { x := ; }").message.contains("expected an expression, found `;`"));
    }

    #[test]
    fn spec_example_parses() {
        let src = r#"
proc sum(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    yield acc;
}

proc main() -> int {
    sym b[]: int = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int = sum(lend b, 2) on NaT { burn b; yield -1; };
    s = s << 1;
    tri s <=> 10 {
        - => print("less");
        0 => print("equal");
        + => print("greater");
    }
    burn b;
    yield 0;
}
"#;
        let p = program(src);
        let Item::Proc(main) = &p.items[1] else { panic!() };
        assert_eq!(main.name.name, "main");
        assert_eq!(main.body.stmts.len(), 7);
    }
}
