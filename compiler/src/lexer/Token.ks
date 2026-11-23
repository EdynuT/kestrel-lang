// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * One piece of the source code found by the lexer: its kind and where
 * it is (`start` to `end`, counted in characters from the start of
 * the file; `end` is not included).
 *
 * @since 0.8
 * @author Edynu
 */

use lexer.TokenKind;

public struct Token {
    kind: TokenKind;
    start: int;
    end: int;
}
