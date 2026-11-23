// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Token-related definitions for the lexer.
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
