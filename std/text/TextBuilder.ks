// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * Builds a text piece by piece: the characters are kept in an array, and
 * the text is made once, at the end (`+` copies the whole text every time).
 *
 * @since 0.7
 * @author Edynu
 * @see TextBuilder.toText
 * @see TextBuilder.add
 * @see TextBuilder.addChar
 * @see TextBuilder.len
 * @see TextBuilder.build
 */

public class TextBuilder {
    chars[]: tryte;

    /* An empty builder: `TextBuilder()`. */
    public proc TextBuilder() -> TextBuilder {
        empty[]: tryte = {};
        return TextBuilder { chars: give empty };
    }

    /* Puts every character of `value` at the end. */
    public proc add(&self, value: text) {
        walk i in 0..len(value) - 1 {
            append(self.chars, value[i]);
        }
    }

    /* Puts a single character at the end. */
    public proc addChar(&self, char: tryte) {
        append(self.chars, char);
    }

    /* Returns the number of characters accumulated so far. */
    public proc len(&self) -> int {
        return len(self.chars);
    }

    /* Builds the final text from the accumulated characters. */
    public proc build(&self) -> text {
        return toText(self.chars, 'c');
    }
}
