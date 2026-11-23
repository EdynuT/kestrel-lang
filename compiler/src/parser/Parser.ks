// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Builds the syntax tree (`Node`) from the tokens of the source code.
 *
 * It runs the lexer itself and walks its tokens one at a time, as the
 * lexer walks the characters: `current()` is the token it is looking
 * at, `advance()` moves to the next one and `expect()` requires a
 * kind of token. Operations are read by precedence, so in `1 + 2 * 3`
 * the `*` ends up deeper in the tree than the `+`. Code that does not
 * fit the grammar throws a `Diagnostic`.
 *
 * @since 0.8
 * @author Edynu
 */

use std.collections.List;

use lexer.Token;
use lexer.TokenKind;
use lexer.Lexer;
use diag.Diagnostic;
use parser.Node;
use parser.NodeKind;

public class Parser {
    source: text;
    lexer: Lexer;
    position: int;
    
    public proc Parser(source: text) -> Parser throws Diagnostic {
        lexer: Lexer = Lexer(source);
        lexer.run();
        return Parser { source: source, lexer: give lexer, position: 0};
    }

    public proc current(&self) -> Token {
        return self.lexer.tokens.get(self.position);
    }

    public proc advance(&self) {
        if self.current().kind != TokenKind.EndOfFile {
            self.position++;
        }
    }

    /* Reads the current token if it is `kind`; `what` names it in the error: "`)`". */
    public proc expect(&self, kind: TokenKind, what: text) throws Diagnostic {
        if self.current().kind != kind {
            throw Diagnostic("expected " + what, self.current().start, self.current().end);
        }
        self.advance();
    }

    public proc primary(&self) -> Node throws Diagnostic {
        token: Token = self.current();
        match token.kind {
            TokenKind.Number => {
                self.advance();
                return Node(NodeKind.Number, token.start, token.end, TokenKind.EndOfFile, List<Node>());
            }
            TokenKind.Identifier => {
                self.advance();
                return Node(NodeKind.Name, token.start, token.end, TokenKind.EndOfFile, List<Node>());
            }
            TokenKind.LeftParen => {
                self.advance();
                expr: Node = self.primary();
                self.expect(TokenKind.RightParen, ")");
                return expr;
            }
            else => {
                throw Diagnostic("expected an expression", token.start, token.end);
            }
        }
    }
}
