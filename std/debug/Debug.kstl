// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Tools for finding mistakes: looking inside the program while it runs.
 *
 * @since 0.7
 * @author Edynu
 * @see Debug.dump
 * @see Debug.hashMix
 */

/*
 * Shows every variable of the running proc and the heap, under `label`,
 * so you can see the program's state at that point.
 *
 * @param label what to call this point in the output
 */
public native proc dump(label: text);

/*
 * Mixes one more value into a hash: `hash * 31 + next`, wrapped into the
 * `int` range. It is what the `hashCode` every class gets uses, field by
 * field (the same trick as Java's).
 *
 * @param hash the hash so far
 * @param next the value to mix in
 * @return the new hash
 */
public native proc hashMix(hash: int, next: int) -> int;
