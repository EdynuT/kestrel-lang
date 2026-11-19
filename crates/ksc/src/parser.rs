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
    let mut p = Parser { tokens, pos: 0 };
    let mut items = Vec::new();
    while p.peek() != &Tok::Eof {
        items.push(p.item()?);
    }
    Ok(Program { items })
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

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
            _ => Err(self.unexpected("`proc` or `const`")),
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
            let pname = self.ident()?;
            self.expect(Tok::Colon)?;
            params.push(Param { name: pname, ty: self.type_expr()? });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        let body = self.block()?;
        Ok(ProcDecl { name, params, ret, body })
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

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        if let Some(s) = self.scalar() {
            return Ok(TypeExpr { ty: Type::Scalar(s), span: start });
        }
        let ty = match self.peek() {
            Tok::Text => {
                self.bump();
                Type::Text
            }
            Tok::Own | Tok::Lent => {
                let own = self.bump().tok == Tok::Own;
                self.expect(Tok::LBracket)?;
                let elem = match self.peek() {
                    Tok::Tryte => Elem::Tryte,
                    Tok::Int => Elem::Int,
                    _ => {
                        return Err(self
                            .unexpected("`tryte` or `int`")
                            .with_note("buffers hold trytes or ints"));
                    }
                };
                self.bump();
                self.expect(Tok::RBracket)?;
                if own { Type::Own(elem) } else { Type::Lent(elem) }
            }
            _ => return Err(self.unexpected("a type")),
        };
        Ok(TypeExpr { ty, span: start.to(self.tokens[self.pos - 1].span) })
    }

    // ---- statements ----

    fn block(&mut self) -> PResult<Block> {
        let start = self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            stmts.push(self.stmt()?);
        }
        let end = self.expect(Tok::RBrace)?;
        Ok(Block { stmts, span: start.to(end) })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = self.stmt_kind(true)?;
        Ok(Stmt { kind, span: start.to(self.tokens[self.pos - 1].span) })
    }

    /// Parses one statement. `with_semi` is false inside a `tri` arm,
    /// where `- => print("x"),` ends with a comma instead.
    fn stmt_kind(&mut self, with_semi: bool) -> PResult<StmtKind> {
        let end = |p: &mut Parser| if with_semi { p.expect(Tok::Semi).map(|_| ()) } else { Ok(()) };
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
                if !matches!((self.peek(), self.peek_at(1)), (Tok::Ident(_), Tok::Colon | Tok::ColonEq)) {
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
            Tok::Walk => return self.walk(),
            Tok::Ident(name) => match self.peek_at(1) {
                Tok::Colon | Tok::ColonEq => {
                    let kind = self.let_stmt()?;
                    end(self)?;
                    kind
                }
                // Two names in a row can only be an instruction: `add acc, x;`
                Tok::Ident(_) if InstrOp::from_name(&name).is_some() => {
                    let kind = self.instr()?;
                    end(self)?;
                    kind
                }
                _ => {
                    let kind = self.expr_or_assign()?;
                    end(self)?;
                    kind
                }
            },
            _ => {
                let kind = self.expr_or_assign()?;
                end(self)?;
                kind
            }
        };
        Ok(kind)
    }

    fn let_stmt(&mut self) -> PResult<StmtKind> {
        let name = self.ident()?;
        let (ty, reg) = if self.eat(&Tok::ColonEq) {
            (None, None)
        } else {
            self.expect(Tok::Colon)?;
            let ty = self.type_expr()?;
            let reg = match self.peek() {
                Tok::Reg(r) => {
                    let r = *r;
                    Some((r, self.bump().span))
                }
                _ => None,
            };
            self.expect(Tok::Eq)?;
            (Some(ty), reg)
        };
        let value = self.expr()?;
        let on_nat = if self.eat(&Tok::On) {
            self.expect(Tok::NaT)?;
            Some(self.block()?)
        } else {
            None
        };
        Ok(StmtKind::Let { is_final: false, name, ty, reg, value, on_nat })
    }

    fn instr(&mut self) -> PResult<StmtKind> {
        let mnemonic = self.ident()?;
        let op = InstrOp::from_name(&mnemonic.name).expect("checked by the caller");
        let dest = self.place()?;
        let mut args = Vec::new();
        while self.eat(&Tok::Comma) {
            args.push(self.expr()?);
        }
        let (min, max, form) = match op {
            InstrOp::Neg => (0, 0, "neg d"),
            InstrOp::Mov => (1, 1, "mov d, s"),
            _ => (1, 2, "op d, s  or  op d, a, b"),
        };
        if args.len() < min || args.len() > max {
            let takes = if min == max {
                format!("{} operand{}", min + 1, if min == 0 { "" } else { "s" })
            } else {
                format!("{} or {} operands", min + 1, max + 1)
            };
            return Err(Diagnostic::new(
                format!("`{}` takes {takes}, found {}", mnemonic.name, args.len() + 1),
                mnemonic.span.to(self.tokens[self.pos - 1].span),
            )
            .with_note(format!("form: {form}")));
        }
        Ok(StmtKind::Instr { op, op_span: mnemonic.span, dest, args })
    }

    fn place(&mut self) -> PResult<Place> {
        let name = self.ident()?;
        let index = if self.eat(&Tok::LBracket) {
            let e = self.expr()?;
            self.expect(Tok::RBracket)?;
            Some(e)
        } else {
            None
        };
        let span = name.span.to(self.tokens[self.pos - 1].span);
        Ok(Place { name, index, span })
    }

    fn expr_or_assign(&mut self) -> PResult<StmtKind> {
        let lhs = self.expr()?;
        if !self.eat(&Tok::Eq) {
            return Ok(StmtKind::Expr(lhs));
        }
        let place = match lhs.kind {
            ExprKind::Var(name) => Place { name: Ident { name, span: lhs.span }, index: None, span: lhs.span },
            ExprKind::Index(base, idx) => match base.kind {
                ExprKind::Var(name) => Place { name: Ident { name, span: base.span }, index: Some(*idx), span: lhs.span },
                _ => return Err(Diagnostic::new("only a variable can be indexed on the left of `=`", base.span)),
            },
            _ => {
                return Err(Diagnostic::new("cannot assign to this expression", lhs.span)
                    .with_note("the left side of `=` must be a variable or `buffer[index]`"));
            }
        };
        let value = self.expr()?;
        Ok(StmtKind::Assign { place, value })
    }

    fn if_stmt(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::If)?;
        let cond = self.expr()?;
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

    fn tri(&mut self) -> PResult<StmtKind> {
        let tri_span = self.expect(Tok::Tri)?;
        // `tri cmp3 a, b { ... }` is the assembly spelling of `tri a <=> b { ... }`.
        let head = if matches!(self.peek(), Tok::Ident(n) if n == "cmp3") {
            self.bump();
            let a = self.expr()?;
            self.expect(Tok::Comma)?;
            let b = self.expr()?;
            let span = a.span.to(b.span);
            Expr { kind: ExprKind::Binary(BinOp::Cmp3, Box::new(a), Box::new(b)), span }
        } else {
            self.expr()?
        };
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
            let body = if self.peek() == &Tok::LBrace {
                let b = self.block()?;
                self.eat(&Tok::Comma);
                b
            } else {
                let start = self.span();
                let kind = self.stmt_kind(false)?;
                let span = start.to(self.tokens[self.pos - 1].span);
                if self.peek() != &Tok::RBrace {
                    self.expect(Tok::Comma)?;
                }
                Block { stmts: vec![Stmt { kind, span }], span }
            };
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

    fn walk(&mut self) -> PResult<StmtKind> {
        self.expect(Tok::Walk)?;
        let var = self.ident()?;
        self.expect(Tok::In)?;
        let range = if self.eat(&Tok::Span) {
            Range::Span(self.expr()?)
        } else {
            let a = self.expr()?;
            self.expect(Tok::DotDot)?;
            Range::Between(a, self.expr()?)
        };
        let body = self.block()?;
        Ok(StmtKind::Walk { var, range, body })
    }

    // ---- expressions ----

    pub fn expr(&mut self) -> PResult<Expr> {
        self.binary(0)
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
        while let Some((_, op)) = ops.iter().find(|(t, _)| t == self.peek()) {
            let op_span = self.bump().span;
            let rhs = self.binary(level + 1)?;
            if level == 2 && ops.iter().any(|(t, _)| t == self.peek()) {
                return Err(Diagnostic::new("comparisons cannot be chained", self.span())
                    .with_note("compare two values at a time: `a < b && b < c`"));
            }
            let span = lhs.span.to(rhs.span).to(op_span);
            lhs = Expr { kind: ExprKind::Binary(*op, Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let op = match self.peek() {
            Tok::Bang => Some(UnOp::Not),
            Tok::Minus | Tok::Plus => {
                // A lone `-` or `+` before a delimiter is a trit literal: `x: trit = +;`
                if matches!(self.peek_at(1), Tok::Semi | Tok::Comma | Tok::RParen | Tok::RBracket | Tok::RBrace) {
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
        let exp = self.unary()?; // right-associative: 2 ** 3 ** 2 == 2 ** 9
        let span = base.span.to(exp.span);
        Ok(Expr { kind: ExprKind::Binary(BinOp::Pow, Box::new(base), Box::new(exp)), span })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        while self.eat(&Tok::LBracket) {
            let idx = self.expr()?;
            let close = self.expect(Tok::RBracket)?;
            let span = e.span.to(close);
            e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), span };
        }
        Ok(e)
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
                let inner = self.expr()?;
                let close = self.expect(Tok::RParen)?;
                return Ok(Expr { kind: inner.kind, span: start.to(close) });
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
                        args.push(self.expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    let close = self.expect(Tok::RParen)?;
                    return Ok(Expr { kind: ExprKind::Call { name, args }, span: start.to(close) });
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
            ExprKind::Index(a, i) => format!("{}[{}]", sx(a), sx(i)),
            ExprKind::Call { name, args } => {
                format!("{}({})", name.name, args.iter().map(sx).collect::<Vec<_>>().join(", "))
            }
            ExprKind::Claim(n) => format!("(claim {})", sx(n)),
            ExprKind::Give(n) => format!("(give {})", n.name),
            ExprKind::Lend(n) => format!("(lend {})", n.name),
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
        let stmts = body("acc: int @R+1 = 0; y := claim 2 * k + 1; s: int = f() on NaT { yield; };");
        let StmtKind::Let { name, ty, reg, .. } = &stmts[0] else { panic!() };
        assert_eq!((name.name.as_str(), ty.unwrap().ty, reg.unwrap().0), ("acc", Type::Scalar(Scalar::Int), 1));
        let StmtKind::Let { ty: None, value, .. } = &stmts[1] else { panic!() };
        assert_eq!(sx(value), "(claim (+ (* 2 k) 1))");
        let StmtKind::Let { on_nat: Some(b), .. } = &stmts[2] else { panic!() };
        assert_eq!(b.stmts[0].kind, StmtKind::Yield(None));
    }

    #[test]
    fn instructions_and_assignment() {
        let stmts = body("add acc, v[i]; mul d, a, b; neg x; mov b[0], 5; x = 1; b[-1] = 2; add(1, 2);");
        assert!(matches!(&stmts[0], StmtKind::Instr { op: InstrOp::Add, args, .. } if args.len() == 1));
        assert!(matches!(&stmts[1], StmtKind::Instr { op: InstrOp::Mul, args, .. } if args.len() == 2));
        assert!(matches!(&stmts[2], StmtKind::Instr { op: InstrOp::Neg, args, .. } if args.is_empty()));
        assert!(matches!(&stmts[3], StmtKind::Instr { op: InstrOp::Mov, dest, .. } if dest.index.is_some()));
        assert!(matches!(&stmts[4], StmtKind::Assign { place, .. } if place.index.is_none()));
        assert!(matches!(&stmts[5], StmtKind::Assign { place, .. } if place.index.is_some()));
        // `add(...)` with parentheses is an ordinary call, not an instruction.
        assert!(matches!(&stmts[6], StmtKind::Expr(Expr { kind: ExprKind::Call { .. }, .. })));
    }

    #[test]
    fn tri_and_walk() {
        let stmts = body(
            "tri cmp3 s, 10 { + => print(\"big\"), - => { x = 1; } 0 => yield, }
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
        let p = program("const LIMIT: int = 0vMM;\nproc sum(v: lent [int], k: int) -> int { yield k; }");
        assert!(matches!(&p.items[0], Item::Const(c) if c.name.name == "LIMIT"));
        let Item::Proc(f) = &p.items[1] else { panic!() };
        assert_eq!(f.params[0].ty.ty, Type::Lent(Elem::Int));
        assert_eq!(f.ret.unwrap().ty, Type::Scalar(Scalar::Int));
    }

    #[test]
    fn errors() {
        assert!(parse_err("proc main() { final 5; }").message.contains("after `final`"));
        assert!(parse_err("proc main() { t: trit = 1 < 2 < 3; }").message.contains("cannot be chained"));
        let d = parse_err("proc main() { x := 1\n y := 2; }");
        assert_eq!(d.message, "expected `;`, found `y`");
        assert_eq!(d.span.start, "proc main() { x := 1".len());

        assert!(parse_err("proc main() { tri x { - => a, + => b, } }").message.contains("missing the 0 arm"));
        assert!(parse_err("proc main() { tri x { - => a, - => b, 0 => c, + => d, } }").message.contains("appears twice"));
        assert!(parse_err("proc main() { mov x, 1, 2; }").message.contains("`mov` takes 2 operands, found 3"));
        assert!(parse_err("proc main() { neg x, 1; }").message.contains("`neg` takes 1 operand, found 2"));
        assert!(parse_err("proc main() { x + 1 = 3; }").message.contains("cannot assign"));
        assert!(parse_err("proc main() { b: own [real] = claim 3; }").note.unwrap().contains("trytes or ints"));
        assert!(parse_err("proc main() { x := a <=> b <=> c; }").message.contains("cannot be chained"));
        assert!(parse_err("x := 1;").message.contains("expected `proc` or `const`"));
        assert!(parse_err("proc main() { x := ; }").message.contains("expected an expression, found `;`"));
    }

    #[test]
    fn spec_example_parses() {
        let src = r#"
proc sum(v: lent [int], k: int) -> int {
    acc: int @R0 = 0;
    walk i in span k {
        add acc, v[i];
    }
    yield acc;
}

proc main() -> int {
    b: own [int] = claim 5;
    walk i in span 2 {
        b[i] = i * i;
    }
    s: int @R+1 = sum(lend b, 2) on NaT { burn b; yield -1; };
    shl3 s, 1;
    tri cmp3 s, 10 {
        - => print("less"),
        0 => print("equal"),
        + => print("greater"),
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
