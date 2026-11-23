// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * An error found in the source code: a message and the place where it
 * happened (`start` to `end`, counted in characters from the start of
 * the file).
 *
 * The lexer throws it instead of printing it; whoever catches it
 * (`Main`) decides how to show it.
 *
 * @since 0.8
 * @author Edynu
 */

@impl(Error)
public class Diagnostic {
    description: text;
    public start: int;
    public end: int;

    public proc Diagnostic(description: text, start: int, end: int) -> Diagnostic {
        return Diagnostic { description: description, start: start, end: end };
    }

    @override
    public proc message(&self) -> text {
        return self.description;
    }
}
