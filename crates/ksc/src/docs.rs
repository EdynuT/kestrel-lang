//! Documentation for an editor: what `/* */` says about the name under
//! the mouse (`ksc doc`). Uses only names as written, no types: `obj.add`
//! shows every `add` the program's files document.

use std::collections::HashMap;

use crate::ast::{Doc, Item, short_name};
use crate::diag::{Diagnostic, SourceMap};
use crate::lexer::{self, Tok, Token};
use crate::modules::Unit;

/// A declaration: a file (`units[unit]`) and its name as written there
/// (a key of `Program::docs`; `""` for the file itself).
pub type Target = (usize, String);

/// The documentation (Markdown) of the name at byte `offset` of unit `me`'s file.
pub fn hover(units: &[Unit], map: &SourceMap, me: usize, offset: usize) -> Option<String> {
    let shown: Vec<String> = targets(units, map, me, offset)
        .into_iter()
        .take(5)
        .filter_map(|(u, key)| {
            let d = units[u].program.docs.get(&key)?;
            let file = key.is_empty().then_some(units[u].info.path.as_str());
            Some(show(d, file, units, map))
        })
        .collect();
    (!shown.is_empty()).then(|| shown.join("\n\n---\n\n"))
}

/// The declarations the name at byte `offset` of unit `me`'s file may be
/// (by name, not type: `obj.add` may be the `add` of any class), with
/// every overload of each (`area`, `area~2`).
pub fn targets(units: &[Unit], map: &SourceMap, me: usize, offset: usize) -> Vec<Target> {
    let mut out = Vec::new();
    for (u, key) in found(units, map, me, offset) {
        let base = key.split('~').next().unwrap_or(&key).to_string();
        if out.contains(&(u, base.clone())) {
            continue;
        }
        let docs = &units[u].program.docs;
        out.push((u, base.clone()));
        out.extend((2..).map(|n| format!("{base}~{n}")).take_while(|k| docs.contains_key(k)).map(|k| (u, k)));
    }
    out
}

/// The tokens of unit `u`'s file.
fn tokens_of(units: &[Unit], map: &SourceMap, u: usize) -> Option<Vec<Token>> {
    let file = map.file_at(units[u].base)?;
    lex_with_formats(&file.src)
}

/// The tokens of `src`, plus those of the code inside each string's
/// `{...}` (a `printf` format: `"{Chars.isUpper(c)}"`), right after the
/// string and with their real places, so a name used only there counts.
/// `{{` is a plain brace and `:t` at the end is the format, not code.
pub fn lex_with_formats(src: &str) -> Option<Vec<Token>> {
    let tokens = lexer::lex(src).ok()?;
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        let inner = match t.tok {
            Tok::Str(_) => src.get(t.span.start..t.span.end).map(|raw| format_parts(raw, t.span.start)).unwrap_or_default(),
            _ => Vec::new(),
        };
        out.push(t);
        for (at, code) in inner {
            if let Ok(found) = lexer::lex(code) {
                out.extend(found.into_iter().filter(|x| x.tok != Tok::Eof).map(|mut x| {
                    x.span = crate::diag::Span::new(x.span.start + at, x.span.end + at);
                    x
                }));
            }
        }
    }
    Some(out)
}

/// The code of each `{...}` in a string literal as written (`raw`, quotes
/// included, starting at byte `start`): its place and its text.
fn format_parts(raw: &str, start: usize) -> Vec<(usize, &str)> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'{' if bytes.get(i + 1) == Some(&b'{') => i += 2,
            b'{' => {
                let Some(len) = raw[i + 1..].find('}') else { break };
                let code = &raw[i + 1..i + 1 + len];
                // `{value:t}`: the `:t` (or `:v`, `:c`) is the format.
                let code = match code.rsplit_once(':') {
                    Some((before, fmt)) if fmt.len() == 1 && fmt.chars().all(|c| c.is_ascii_alphabetic()) => before,
                    _ => code,
                };
                out.push((start + i + 1, code));
                i += len + 2;
            }
            _ => i += 1,
        }
    }
    out
}

fn found(units: &[Unit], map: &SourceMap, me: usize, offset: usize) -> Vec<Target> {
    let Some(tokens) = tokens_of(units, map, me) else { return Vec::new() };
    let Some(at) = tokens.iter().position(|t| t.span.start <= offset && offset < t.span.end && matches!(t.tok, Tok::Ident(_))) else {
        return Vec::new();
    };
    found_in(units, map, me, &tokens, &class_bodies(&tokens), at)
}

/// What the name at token `at` of unit `me` (its `tokens`) may be.
/// (`bodies` is [`class_bodies`] of `tokens`, worked out once by the caller.)
fn found_in(units: &[Unit], map: &SourceMap, me: usize, tokens: &[Token], bodies: &[(String, usize, usize)], at: usize) -> Vec<Target> {
    let unit = &units[me];
    let offset = tokens[at].span.start;
    // `A.B.c` up to the name under the mouse.
    let mut chain = vec![name(&tokens[at].tok).unwrap_or_default()];
    let mut k = at;
    while k >= 2 && is_dot(&tokens[k - 1].tok) {
        match &tokens[k - 2].tok {
            Tok::Ident(n) => chain.insert(0, n.clone()),
            t => match lexer::keyword_text(t) {
                Some(word) => chain.insert(0, word.to_string()),
                None => break,
            },
        }
        k -= 2;
    }
    let has = |u: usize, key: &str| units[u].program.docs.contains_key(key);
    let one = |u: usize, key: String| vec![(u, key)];
    // `use std.text.Texts;`: the file.
    if k >= 1 && tokens[k - 1].tok == Tok::Use {
        let path = chain.join(".");
        return unit.uses.iter().copied().filter(|&u| units[u].info.path == path).map(|u| (u, String::new())).collect();
    }
    let used = |alias: &str| unit.uses.iter().copied().find(|&u| units[u].info.alias() == alias);
    let last = chain.last().cloned().unwrap_or_default();
    if chain.len() >= 2 {
        let owner = &chain[chain.len() - 2];
        if chain.len() == 2
            && let Some(u) = used(owner)
        {
            // `Texts.trim`: an item of a used file; `List.add` too (its main item).
            let member = format!("{owner}.{last}");
            return if has(u, &last) { one(u, last) } else if has(u, &member) { one(u, member) } else { Vec::new() };
        }
        // `Circle.area`, a class of this file.
        let member = format!("{owner}.{last}");
        if has(me, &member) {
            return one(me, member);
        }
        // `self.current()`, `lexer.current()`, `self.lexer.current()`: the
        // class of what comes before the dot, when it can be told.
        if let Some((u, class)) = owner_class(units, map, me, tokens, bodies, k, &chain[..chain.len() - 1]) {
            let member = format!("{class}.{last}");
            if has(u, &member) {
                return one(u, member);
            }
        }
        // `obj.add` (its type unknown): a method or field of some class.
        return members(units, me, &last);
    }
    // The name in a declaration (`proc area(...)` inside `class Circle`):
    // that very one.
    let mut here: Vec<(&String, usize)> = unit
        .program
        .docs
        .iter()
        .filter(|(key, d)| (*key == &last || key.ends_with(&format!(".{last}"))) && d.at <= unit.base + offset && unit.base + offset < d.at + d.sig.len())
        .map(|(key, d)| (key, d.at))
        .collect();
    here.sort_by_key(|&(_, at)| std::cmp::Reverse(at));
    if let Some((key, _)) = here.first() {
        return one(me, (*key).clone());
    }
    if has(me, &last) {
        return one(me, last);
    }
    // `List` (the main item of `List.kstl`) or `Texts` (a file).
    if let Some(u) = used(&last) {
        return if has(u, &last) { one(u, last) } else { one(u, String::new()) };
    }
    members(units, me, &last)
}

/// The class (its file and name) of `path`, the names before a dot, read at
/// token `k` of unit `me`: `self` is the class around `k`, a variable or
/// parameter has the type written in its declaration (`lexer: Lexer`), and
/// each field after it has the type written in its class. By the text only,
/// like the rest of this file: `None` when it cannot be told.
fn owner_class(units: &[Unit], map: &SourceMap, me: usize, tokens: &[Token], bodies: &[(String, usize, usize)], k: usize, path: &[String]) -> Option<(usize, String)> {
    let (first, fields) = path.split_first()?;
    let mut owner = if first == "self" {
        let (name, _, _) = bodies.iter().filter(|&&(_, open, close)| open < k && k < close).last()?;
        (me, name.clone())
    } else if let Some(found) = class_named(units, me, first) {
        found // `Lexer.make`: the class itself
    } else {
        // The nearest declaration `first: Type` before `k` whose type is a class.
        let mut to = k;
        loop {
            let (i, ty) = declared_type(tokens, 0, to, first)?;
            if let Some(found) = class_named(units, me, &ty) {
                break found;
            }
            to = i;
        }
    };
    for field in fields {
        let field_tokens = tokens_of(units, map, owner.0)?;
        let (_, open, close) = class_bodies(&field_tokens).into_iter().find(|(name, _, _)| *name == owner.1)?;
        let (_, ty) = declared_type(&field_tokens, open, close, field)?;
        owner = class_named(units, owner.0, &ty)?;
    }
    Some(owner)
}

/// The class, interface or struct named `name` as seen from unit `from`:
/// one of its own, or the main item of a file it uses.
fn class_named(units: &[Unit], from: usize, name: &str) -> Option<(usize, String)> {
    let is_type = |u: usize| {
        units[u].program.items.iter().any(|i| {
            matches!(i, Item::Class(_) | Item::Interface(_) | Item::Struct(_)) && short_name(&crate::parser::item_name(i).name) == name
        })
    };
    if is_type(from) {
        return Some((from, name.to_string()));
    }
    units[from].uses.iter().copied().find(|&u| units[u].info.alias() == name && is_type(u)).map(|u| (u, name.to_string()))
}

/// Every `class Name { ... }` of a file: its name and the tokens of its
/// `{` and `}`.
fn class_bodies(tokens: &[Token]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    for i in 0..tokens.len() {
        if !matches!(tokens[i].tok, Tok::Class | Tok::Interface | Tok::Struct | Tok::Enum) {
            continue;
        }
        let Some(Tok::Ident(name)) = tokens.get(i + 1).map(|t| &t.tok) else { continue };
        let Some(open) = (i + 2..tokens.len()).find(|&j| tokens[j].tok == Tok::LBrace) else { continue };
        let mut depth = 0;
        for (j, t) in tokens.iter().enumerate().skip(open) {
            match t.tok {
                Tok::LBrace => depth += 1,
                Tok::RBrace => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                out.push((name.clone(), open, j));
                break;
            }
        }
    }
    out
}

/// The last `name: Type` in `tokens[from..to]` (not after a dot): where it
/// is and the type's name (`List` for `List<Node>`). A built-in type
/// (`name: int`) has no name to give, so the search goes on before it.
fn declared_type(tokens: &[Token], from: usize, to: usize, var: &str) -> Option<(usize, String)> {
    (from..to.min(tokens.len())).rev().find_map(|i| {
        let is_name = matches!(&tokens[i].tok, Tok::Ident(n) if n == var);
        let after_dot = i > 0 && is_dot(&tokens[i - 1].tok);
        if !is_name || after_dot || tokens.get(i + 1)?.tok != Tok::Colon {
            return None;
        }
        // Skip what may come before the type's name (`&`, `own`).
        tokens.get(i + 2..(i + 5).min(tokens.len()))?.iter().find_map(|t| name(&t.tok)).map(|ty| (i, ty))
    })
}

/// Every place where the declaration `target` is named: each name in the
/// program's files that leads back to it (as for the mouse), declaration
/// included. Each is a unit and a byte offset in its file.
pub fn references(units: &[Unit], map: &SourceMap, target: &Target) -> Vec<(usize, usize)> {
    let base = |key: &str| key.split('~').next().unwrap_or(key).to_string();
    let wanted = (target.0, base(&target.1));
    let Some(last) = wanted.1.rsplit('.').next().map(str::to_string).filter(|n| !n.is_empty()) else { return Vec::new() };
    let mut out = Vec::new();
    for u in 0..units.len() {
        let Some(tokens) = tokens_of(units, map, u) else { continue };
        let bodies = class_bodies(&tokens);
        for (i, t) in tokens.iter().enumerate() {
            if !matches!(&t.tok, Tok::Ident(n) if *n == last) {
                continue;
            }
            if found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, key)| (*fu, base(key)) == wanted) {
                out.push((u, t.span.start));
            }
        }
    }
    out
}

/// A warning for each parameter of unit `u`'s procs and methods that its
/// body never names, each proc without `public` that nothing calls,
/// each private method of its classes that nothing calls, and each private
/// field that nothing reads or changes. Only the class itself can call one, so only its body is read; a
/// call is `something.name(` that leads back to that very method (as for
/// the mouse), so a variable with the same name in another method, or a
/// method of another class with the same name, does not count. Nor does a
/// call from inside the method itself. A constructor (named as the class)
/// and an annotated method are left alone.
pub fn unused_private_methods(units: &[Unit], map: &SourceMap, u: usize) -> Vec<Diagnostic> {
    let Some(tokens) = tokens_of(units, map, u) else { return Vec::new() };
    let base = units[u].base;
    let bodies = class_bodies(&tokens);
    let mut out = Vec::new();
    // Where each name is written, in order: each rule below looks only at
    // the places of the names it checks (not at the whole file each time).
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, t) in tokens.iter().enumerate().skip(1) {
        if let Tok::Ident(n) = &t.tok {
            index.entry(n.as_str()).or_default().push(i);
        }
    }
    let places = |name: &str| index.get(name).map_or(&[][..], Vec::as_slice);
    // The tokens of a span (they are in order of place).
    let range = |span: crate::diag::Span| {
        let lo = tokens.partition_point(|t| base + t.span.start < span.start);
        let hi = tokens.partition_point(|t| base + t.span.start < span.end);
        lo.max(1)..hi
    };
    let within = |occ: &'_ [usize], r: &std::ops::Range<usize>| {
        let lo = occ.partition_point(|&i| i < r.start);
        let hi = occ.partition_point(|&i| i < r.end);
        occ[lo..hi].to_vec()
    };
    let braces = Braces::of(&tokens);
    // Variables of each body that are only ever written.
    for item in &units[u].program.items {
        let decls: Vec<&crate::ast::ProcDecl> = match item {
            Item::Proc(p) if !p.native && !p.instance => vec![p],
            Item::Class(c) if !c.instance => c.methods.iter().map(|m| &m.decl).collect(),
            _ => Vec::new(),
        };
        for decl in decls {
            out.extend(never_read(&tokens, &index, &braces, base, range(decl.body.span)));
        }
    }
    // A parameter is used when its body names it: not after a dot (that is a
    // field or a method) nor before a `:` (the label in `Point { x: 1 }`).
    // An `@override` method takes the parameters its interface asks for,
    // used or not, so it is left alone (as are native procs and copies of
    // generics).
    let mut params = |decl: &crate::ast::ProcDecl| {
        if decl.native || decl.instance {
            return;
        }
        let body = range(decl.body.span);
        for param in &decl.params {
            let name = &param.name.name;
            let used = within(places(name), &body).into_iter().any(|i| {
                !is_dot(&tokens[i - 1].tok)
                    && tokens.get(i + 1).is_none_or(|t| t.tok != Tok::Colon)
            });
            if !used {
                out.push(
                    Diagnostic::warning(format!("parameter `{name}` is never used"), param.name.span)
                        .with_note(format!("remove it (and its value in each call), or use it")),
                );
            }
        }
    };
    for item in &units[u].program.items {
        match item {
            Item::Proc(p) => params(p),
            Item::Class(c) if !c.instance => {
                for m in c.methods.iter().filter(|m| !m.annotations.iter().any(|a| a.name.name == "override")) {
                    params(&m.decl);
                }
            }
            _ => {}
        }
    }
    // A proc without `public` is seen only in its own file: it is used when
    // the file calls `name(...)` (or `name<T>(...)`), not after a dot (that
    // is a method), outside its own body, and it leads back to that proc.
    // `main`, an annotated proc (`@test`) and a native one are left alone.
    let program = &units[u].program;
    for item in &program.items {
        let Item::Proc(p) = item else { continue };
        // (In a project the name carries its file: `Main.describe`.)
        let name = &short_name(&p.name.name).to_string();
        if name == "main" || p.native || p.instance || !p.annotations.is_empty() || program.public.contains(name) || program.public.contains(&p.name.name) {
            continue;
        }
        let own = p.body.span;
        let called = places(name).iter().any(|&i| {
            let at = base + tokens[i].span.start;
            !matches!(tokens[i - 1].tok, Tok::Dot | Tok::QuestionDot | Tok::Proc)
                && tokens.get(i + 1).is_some_and(|t| matches!(t.tok, Tok::LParen | Tok::Lt))
                && !(own.start <= at && at < own.end)
                && found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, k)| *fu == u && k.split('~').next() == Some(name.as_str()))
        });
        if !called {
            out.push(
                Diagnostic::warning(format!("proc `{name}` is never called"), p.name.span)
                    .with_note(format!("nothing in this file calls it, and without `public` no other file can; remove it, or call it (`{name}(...)`)")),
            );
        }
    }
    let mut unused_types: Vec<String> = Vec::new();
    // Types and consts without `public` are seen only in their own file:
    // one is used when the file names it outside its own declaration (a
    // class names itself in its constructor; that does not count).
    for item in &program.items {
        let what = match item {
            Item::Const(_) => "const",
            Item::Enum(_) => "enum",
            Item::Struct(_) => "struct",
            Item::Class(c) if !c.instance => "class",
            Item::Interface(_) => "interface",
            _ => continue,
        };
        let full = &crate::parser::item_name(item).name;
        let name = short_name(full);
        if program.public.contains(name) || program.public.contains(full) {
            continue;
        }
        let body = bodies.iter().find(|(n, _, _)| n == name).map(|&(_, open, close)| (open, close));
        let used = places(name).iter().any(|&i| {
            !matches!(tokens[i - 1].tok, Tok::Dot | Tok::QuestionDot | Tok::Const | Tok::Enum | Tok::Struct | Tok::Class | Tok::Interface)
                && !body.is_some_and(|(open, close)| open < i && i < close)
                && found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, k)| *fu == u && k == name)
        });
        if !used {
            out.push(
                Diagnostic::warning(format!("{what} `{name}` is never used"), crate::parser::item_name(item).span)
                    .with_note(format!("nothing in this file uses it, and without `public` no other file can; remove it, or use it")),
            );
            unused_types.push(name.to_string());
            continue; // (its values and fields would all be reported too)
        }
        match item {
            // A value of an enum is used when the file writes `Enum.Value`.
            Item::Enum(e) => {
                for (value, _) in &e.variants {
                    let named = places(&value.name).iter().any(|&i| {
                        i >= 2 && is_dot(&tokens[i - 1].tok) && matches!(&tokens[i - 2].tok, Tok::Ident(n) if n == name)
                    });
                    if !named {
                        out.push(
                            Diagnostic::warning(format!("value `{name}.{}` is never used", value.name), value.span)
                                .with_note(format!("nothing in this file writes `{name}.{}`; remove it, or use it", value.name)),
                        );
                    }
                }
            }
            // A field of a struct is used when the file writes `something.field`
            // and it leads back to that struct (filling it, `Point { x: 1 }`,
            // does not count: every field must get a value there).
            Item::Struct(st) => {
                for (field, _) in &st.fields {
                    let key = format!("{name}.{}", field.name);
                    let used = places(&field.name).iter().any(|&i| {
                        is_dot(&tokens[i - 1].tok)
                            && tokens.get(i + 1).is_none_or(|t| t.tok != Tok::LParen)
                            && found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, k)| *fu == u && *k == key)
                    });
                    if !used {
                        out.push(
                            Diagnostic::warning(format!("field `{name}.{}` is never used", field.name), field.span)
                                .with_note(format!("nothing in this file reads or changes it; remove it, or use it")),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    for item in &program.items {
        let Item::Class(c) = item else { continue };
        let class = short_name(&c.name.name).to_string();
        if c.instance || unused_types.contains(&class) {
            continue;
        }
        let Some(&(_, open, close)) = bodies.iter().find(|(n, _, _)| *n == class) else { continue };
        for m in c.methods.iter().filter(|m| !m.public && m.annotations.is_empty()) {
            let name = &m.decl.name.name;
            if *name == class {
                continue;
            }
            let key = format!("{class}.{name}");
            let own = m.decl.body.span;
            let called = within(places(name), &(open..close)).into_iter().any(|i| {
                let at = base + tokens[i].span.start;
                is_dot(&tokens[i - 1].tok)
                    && tokens.get(i + 1).is_some_and(|t| t.tok == Tok::LParen)
                    && !(own.start <= at && at < own.end)
                    && found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, k)| *fu == u && k.split('~').next() == Some(key.as_str()))
            });
            if !called {
                out.push(
                    Diagnostic::warning(format!("private method `{name}` is never called"), m.decl.name.span)
                        .with_note(format!("nothing in `{class}` calls it; remove it, or call it (`self.{name}(...)`)")),
                );
            }
        }
        // A private field is used when the class writes `something.field`
        // (read or changed) and it leads back to that very field; giving it
        // its first value (`Class { field: value }`) does not count, since
        // every field must get one there.
        for f in c.fields.iter().filter(|f| !f.public) {
            let name = &f.name.name;
            let key = format!("{class}.{name}");
            let used = within(places(name), &(open..close)).into_iter().any(|i| {
                is_dot(&tokens[i - 1].tok)
                    && tokens.get(i + 1).is_none_or(|t| t.tok != Tok::LParen)
                    && found_in(units, map, u, &tokens, &bodies, i).iter().any(|(fu, k)| *fu == u && *k == key)
            });
            if !used {
                out.push(
                    Diagnostic::warning(format!("private field `{name}` is never used"), f.name.span)
                        .with_note(format!("nothing in `{class}` reads or changes it (`self.{name}`); remove it, or use it")),
                );
            }
        }
    }
    out
}

/// `.` or `?.` (`a?.b` reads `b` of `a` when `a` is not null).
fn is_dot(t: &Tok) -> bool {
    matches!(t, Tok::Dot | Tok::QuestionDot)
}

/// Is `t` one of `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `++`, `--`?
fn writes(t: &Tok) -> bool {
    matches!(t, Tok::Eq | Tok::PlusEq | Tok::MinusEq | Tok::StarEq | Tok::SlashEq | Tok::PercentEq | Tok::PlusPlus | Tok::MinusMinus)
}

/// A warning for each variable declared in the body at span `body` that
/// is given values but never read. A declaration is `name: Type = ...;` (or
/// `name: Type;`) where a statement starts; `name` after it is a write when
/// `=`, `+=` or `++` follows, and the `name` on the right of its own
/// assignment (`total = total + 1`) feeds only itself, so it is no read
/// either. A variable never named at all is the checker's warning.
fn never_read(tokens: &[Token], index: &HashMap<&str, Vec<usize>>, braces: &Braces, base: usize, body: std::ops::Range<usize>) -> Vec<Diagnostic> {
    let starts = |t: &Tok| matches!(t, Tok::Semi | Tok::LBrace | Tok::RBrace | Tok::For);
    let mut out = Vec::new();
    for d in body {
        let Tok::Ident(name) = &tokens[d].tok else { continue };
        if !starts(&tokens[d - 1].tok) {
            continue;
        }
        // `name: Type =` or `name[]: Type =` (not `Point { x: 1, ... }`).
        let mut k = d + 1;
        if tokens.get(k).is_some_and(|t| t.tok == Tok::LBracket) && tokens.get(k + 1).is_some_and(|t| t.tok == Tok::RBracket) {
            k += 2;
        }
        if tokens.get(k).is_none_or(|t| t.tok != Tok::Colon) {
            continue;
        }
        // The type is a few tokens (`List<Node>`, `&Node?`): past a handful
        // it is not a declaration.
        // (A `,` inside `<...>` belongs to the type: `HashMap<text, int>`.)
        let mut angle = 0i32;
        let Some(end) = (k + 1..tokens.len().min(k + 16)).find(|&j| match tokens[j].tok {
            Tok::Lt => {
                angle += 1;
                false
            }
            Tok::Gt => {
                angle -= 1;
                false
            }
            Tok::Comma => angle <= 0,
            Tok::Eq | Tok::Semi | Tok::RBrace | Tok::LParen | Tok::RParen => true,
            _ => false,
        }) else {
            continue;
        };
        if !matches!(tokens[end].tok, Tok::Eq | Tok::Semi) {
            continue;
        }
        // Its block: up to the `}` that closes the one it is declared in.
        let Some(close) = braces.closing(end) else { continue };
        let named: Vec<usize> = index.get(name.as_str()).map_or(Vec::new(), |occ| {
            let lo = occ.partition_point(|&j| j <= end);
            let hi = occ.partition_point(|&j| j < close);
            occ[lo..hi].iter().copied().filter(|&j| !is_dot(&tokens[j - 1].tok)).collect()
        });
        // Where `name` is written, and the right side of each write.
        let mut write = Vec::new();
        let mut feeds = Vec::new();
        for &j in &named {
            if tokens.get(j + 1).is_some_and(|t| writes(&t.tok)) {
                write.push(j);
                let stop = (j + 1..close).find(|&m| tokens[m].tok == Tok::Semi || tokens[m].tok == Tok::LBrace).unwrap_or(close);
                feeds.push((j + 1, stop));
            }
        }
        let read = named.iter().any(|&j| {
            tokens.get(j + 1).is_none_or(|t| t.tok != Tok::Colon)
                && !write.contains(&j)
                && !feeds.iter().any(|&(from, to)| from <= j && j < to)
        });
        if !write.is_empty() && !read {
            out.push(
                Diagnostic::warning(format!("variable `{name}` is assigned but never read"), crate::diag::Span::new(base + tokens[d].span.start, base + tokens[d].span.end))
                    .with_note("each value it gets is lost; remove it, or read it"),
            );
        }
    }
    out
}

/// For each token, the `}` that closes the innermost block around it.
struct Braces {
    close: Vec<Option<usize>>,
}

impl Braces {
    fn of(tokens: &[Token]) -> Braces {
        let mut close = vec![None; tokens.len()];
        let mut open: Vec<Vec<usize>> = vec![Vec::new()]; // tokens waiting for their block's `}`
        for (i, t) in tokens.iter().enumerate() {
            match t.tok {
                Tok::LBrace => {
                    open.last_mut().expect("never empty").push(i);
                    open.push(Vec::new());
                }
                Tok::RBrace if open.len() > 1 => {
                    for w in open.pop().expect("checked") {
                        close[w] = Some(i);
                    }
                }
                _ => open.last_mut().expect("never empty").push(i),
            }
        }
        Braces { close }
    }

    fn closing(&self, i: usize) -> Option<usize> {
        self.close.get(i).copied().flatten()
    }
}

/// Every method or field named `member`: this file's first.
fn members(units: &[Unit], me: usize, member: &str) -> Vec<Target> {
    let suffix = format!(".{member}");
    let mut found = Vec::new();
    for u in std::iter::once(me).chain((0..units.len()).filter(|&u| u != me)) {
        let mut keys: Vec<&String> = units[u].program.docs.keys().filter(|k| k.ends_with(&suffix)).collect();
        keys.sort();
        found.extend(keys.into_iter().map(|k| (u, k.clone())));
    }
    found
}

fn name(t: &Tok) -> Option<String> {
    match t {
        Tok::Ident(n) => Some(n.clone()),
        _ => None,
    }
}

/// A declaration and its text, as Markdown; a file's documentation under its `use`.
fn show(d: &Doc, file: Option<&str>, units: &[Unit], map: &SourceMap) -> String {
    let head = match file {
        Some(path) => format!("use {path};"),
        None => d.sig.clone(),
    };
    let link = |target: &str| find(units, target).and_then(|at| place(map, at)).map(|(file, line)| command(&file, line));
    let text = markdown(&d.text, &link);
    if text.is_empty() {
        return format!("```kestrel\n{head}\n```");
    }
    format!("```kestrel\n{head}\n```\n\n{text}")
}

/// Where the item an `@see` names is declared (a span position): `List.add`
/// (a member, or an item of the file `List`), `trim`, `Texts` (a file).
fn find(units: &[Unit], target: &str) -> Option<usize> {
    for u in units {
        if let Some(d) = u.program.docs.get(target) {
            return Some(d.at);
        }
    }
    let (owner, item) = target.rsplit_once('.').unwrap_or(("", target));
    for u in units.iter().filter(|u| u.info.alias() == owner || u.info.path == owner) {
        if let Some(d) = u.program.docs.get(item) {
            return Some(d.at);
        }
    }
    // A file (its start), even without documentation.
    units.iter().find(|u| u.info.alias() == target || u.info.path == target).map(|u| u.base)
}

/// The file and line (from 1) of a span position.
fn place(map: &SourceMap, at: usize) -> Option<(String, u32)> {
    let f = map.file_at(at)?;
    let local = at.checked_sub(f.base)?;
    let line = f.src.get(..local)?.matches('\n').count() as u32 + 1;
    Some((f.path.clone(), line))
}

/// A link that opens `file` at `line` in the editor (its `kestrel.open` command).
fn command(file: &str, line: u32) -> String {
    let args = format!("[{},{line}]", json_text(file));
    let mut encoded = String::new();
    for b in args.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("command:kestrel.open?{encoded}")
}

/// `s` as a JSON string.
fn json_text(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text with its tags (`@since 0.7`...) laid out as Javadoc does: a
/// section each, after the description. A tag's text goes on until the
/// next tag (it may take several lines).
fn markdown(text: &str, link: &dyn Fn(&str) -> Option<String>) -> String {
    let mut body = Vec::new();
    let mut tags: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('@') {
            let (tag, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            tags.push((tag.to_string(), value.trim().to_string()));
        } else if let Some((_, value)) = tags.last_mut().filter(|_| !trimmed.is_empty()) {
            value.push(' ');
            value.push_str(trimmed);
        } else if tags.is_empty() {
            body.push(line);
        }
    }
    let mut out = body.join("\n").trim_end().to_string();
    let all = |name: &str| tags.iter().filter(|(t, _)| t == name).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    let mut sections = Vec::new();
    if let Some(why) = all("deprecated").first() {
        sections.push(format!("**Deprecated.** {why}"));
    }
    let params = all("param");
    if !params.is_empty() {
        let items: Vec<String> = params
            .iter()
            .map(|p| match p.split_once(char::is_whitespace) {
                Some((name, what)) => format!("- `{name}`: {}", what.trim()),
                None => format!("- `{p}`"),
            })
            .collect();
        sections.push(format!("**Parameters:**\n{}", items.join("\n")));
    }
    if let Some(r) = all("return").first() {
        sections.push(format!("**Returns:** {r}"));
    }
    let see = all("see");
    if !see.is_empty() {
        let links: Vec<String> = see
            .iter()
            .map(|s| match link(s) {
                Some(to) => format!("[`{s}`]({to})"),
                None => format!("`{s}`"),
            })
            .collect();
        sections.push(format!("**See also:** {}", links.join(", ")));
    }
    if let Some(v) = all("since").first() {
        sections.push(format!("**Since:** {v}"));
    }
    let authors = all("author");
    if !authors.is_empty() {
        sections.push(format!("**Author:** {}", authors.join(", ")));
    }
    // Tags it does not know are shown as they are.
    let known = ["deprecated", "param", "return", "see", "since", "author"];
    for (t, v) in tags.iter().filter(|(t, _)| !known.contains(&t.as_str())) {
        sections.push(format!("**@{t}** {v}"));
    }
    for section in sections {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&section);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn tags_become_sections() {
        let text = "Adds two numbers.\n\n@param a the first\n@param b the second,\n  maybe negative\n@return their sum\n@see Math.sub\n@see Math.mul\n@since 0.7\n@author Edynu";
        assert_eq!(
            super::markdown(text, &|s| (s == "Math.sub").then(|| "command:x".to_string())),
            "Adds two numbers.\n\n**Parameters:**\n- `a`: the first\n- `b`: the second, maybe negative\n\n**Returns:** their sum\n\n**See also:** [`Math.sub`](command:x), `Math.mul`\n\n**Since:** 0.7\n\n**Author:** Edynu"
        );
    }
}
