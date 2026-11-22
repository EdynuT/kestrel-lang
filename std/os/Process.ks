// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * The running program: the arguments it was started with, and how it ends.
 *
 * @since 0.7
 * @author Edynu
 * @see Process.argCount
 * @see Process.arg
 * @see Process.exit
 */

/* Returns the number of arguments the program was started with. */
public native proc argCount() -> int;

/* 
 * Returns the argument at the given index, counting from 0: for
 * `ksc run a b`, `arg(0)` is `"a"`. An index outside `0..argCount() - 1`
 * stops the program (a `Trap`). 
 */
public native proc arg(index: int) -> text;

/* Exits the program with the given status code. */
public native proc exit(status: int);