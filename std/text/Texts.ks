// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Text-related utility functions.
 *
 * @since 0.7
 * @author Edynu
 * @see Texts.startsWith
 * @see Texts.endsWith
 * @see Texts.indexOf
 * @see Texts.trim
 * @see Texts.split
 * @see Texts.strip
 */

use std.collections.List;

/* Searches for substrings and checks prefixes/suffixes. */
public proc startsWith(value: text, prefix: text) -> bool {
    if len(prefix) > len(value) {
        return false;
    }
    walk i in 0..len(prefix) - 1 {
        if value[i] != prefix[i] {
            return false;
        }
    }
    return true;
}

/* Checks if `value` ends with `suffix`. */
public proc endsWith(value: text, suffix: text) -> bool {
    if len(suffix) > len(value) {
        return false;
    }
    walk i in 0..len(suffix) - 1 {
        if value[len(value) - len(suffix) + i] != suffix[i] {
            return false;
        }
    }
    return true;
}

/* Finds the first occurrence of `sub` in `value`, or -1 if not found. */
public proc indexOf(value: text, sub: text) -> int {
    walk i in 0..len(value) - len(sub) {
        found: bool = true;
        walk j in 0..len(sub) - 1 {
            if value[i + j] != sub[j] {
                found = false;
                break;
            }
        }
        if found {
            return i;
        }
    }
    return -1;
}

/* Trims leading and trailing spaces from `value`. */
public proc trim(value: text) -> text {
    start: int = 0;
    end: int = len(value) - 1;
    while start <= end && isSpace(value[start]) {
        start = start + 1;
    }
    while end >= start && isSpace(value[end]) {
        end = end - 1;
    }
    return sub(value, start, end + 1);
}

/* Splits `value` into a list of substrings separated by `delimiter`. */
public proc split(value: text, delimiter: text) -> List<text> {
    result: List<text> = List<text>();
    // An empty delimiter is found everywhere: the text stays whole.
    if len(delimiter) == 0 {
        result.add(value);
        return result;
    }
    start: int = 0;
    while true {
        rest: text = sub(value, start, len(value));
        position: int = indexOf(rest, delimiter);
        if position == -1 {
            result.add(rest);
            return result;
        }
        result.add(sub(value, start, start + position));
        start = start + position + len(delimiter);
    }
    return result;
}

/* Strips all characters in `chars` from the start and end of `value`. */
public proc strip(value: text, chars: text) -> text {
    start: int = 0;
    end: int = len(value) - 1;
    while start <= end && indexOf(chars, sub(value, start, start + 1)) != -1 {
        start = start + 1;
    }
    while end >= start && indexOf(chars, sub(value, end, end + 1)) != -1 {
        end = end - 1;
    }
    return sub(value, start, end + 1);
}
