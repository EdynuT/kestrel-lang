// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * One node of the syntax tree that the parser builds from the tokens.
 *
 * Every node uses this one class: its `kind` says what it is, `start`
 * and `end` say where it is in the source (its text is read back from
 * there, as with a `Token`), and `children` holds the nodes inside it.
 * In `1 + 2 * 3`, the `+` node has two children: `1` and the `*` node.
 *
 * @since 0.8
 * @author Edynu
 */

use std.collections.List;
use parser.NodeKind;
use lexer.TokenKind;

public class Node {
    public kind: NodeKind;
    public start: int;
    public end: int;
    public operator: TokenKind;
    public children: List<Node>;

    public proc Node(kind: NodeKind, start: int, end: int, operator: TokenKind, children: List<Node>) -> Node {
        return Node { kind: kind, start: start, end: end, operator: operator, children: give children };
    }
}
