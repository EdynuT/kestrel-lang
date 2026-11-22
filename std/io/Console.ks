// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * The console: printing to the screen and reading what is typed.
 *
 * `print` writes its values one after another, `printl` ends the line,
 * and `printf` fills a format (`"x = {x}"`, `"%d"`). Reading takes one
 * line at a time (`readLine`), or a number (`input`).
 *
 * @since 0.7
 * @author Edynu
 * @see Console.print
 * @see Console.printl
 * @see Console.printf
 * @see Console.readLine
 * @see Console.eof
 * @see Console.input
 * @see Console.emit
 */

/* Prints values to the console. */
public native proc print(values: ...);

/* Prints values to the console and ends the line. */
public native proc printl(values: ...);

/* Prints formatted values to the console. */
public native proc printf(format: text, values: ...);

/* Reads a line from the console. */
public native proc readLine() -> text;

/* Checks if the end of the input has been reached. */
public native proc eof() -> bool;

/* Reads an integer from the console, or returns `null` if input is invalid. */
public native proc input() -> int?;

/* Emits a tryte value to the console. */
public native proc emit(value: tryte);
