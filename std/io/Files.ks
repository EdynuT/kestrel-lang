// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Files: reading, writing and checking that they exist.
 *
 * Paths are as the system writes them (`notes.txt`, `src/Main.ks`),
 * relative to the folder the program runs in. A file that cannot be read
 * or written is a failure (a `Trap`).
 *
 * @since 0.7
 * @author Edynu
 * @see Files.read
 * @see Files.write
 * @see Files.writeBytes
 * @see Files.exists
 */

/* Reads the entire contents of a file. */
public native proc read(path: text) -> text;

/* Writes text to a file, overwriting it if it exists. */
public native proc write(path: text, content: text);

/* Writes bytes to a file, overwriting it if it exists. */
public native proc writeBytes(path: text, &bytes[]: int);

/* Checks if a file exists. */
public native proc exists(path: text) -> bool;
