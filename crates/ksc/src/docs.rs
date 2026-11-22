//! Documentation for an editor: what `/* */` says about the name under
//! the mouse (`ksc doc`). Uses only names as written, no types: `obj.add`
//! shows every `add` the program's files document.

use crate::ast::Doc;
use crate::diag::SourceMap;
use crate::lexer::{self, Tok};
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

fn found(units: &[Unit], map: &SourceMap, me: usize, offset: usize) -> Vec<Target> {
    let unit = &units[me];
    let Some(file) = map.file_at(unit.base) else { return Vec::new() };
    let Ok(tokens) = lexer::lex(&file.src) else { return Vec::new() };
    let Some(at) = tokens.iter().position(|t| t.span.start <= offset && offset < t.span.end && matches!(t.tok, Tok::Ident(_))) else {
        return Vec::new();
    };
    // `A.B.c` up to the name under the mouse.
    let mut chain = vec![name(&tokens[at].tok).unwrap_or_default()];
    let mut k = at;
    while k >= 2 && tokens[k - 1].tok == Tok::Dot {
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
        // `obj.add`: a method or field of some class.
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
    // `List` (the main item of `List.ks`) or `Texts` (a file).
    if let Some(u) = used(&last) {
        return if has(u, &last) { one(u, last) } else { one(u, String::new()) };
    }
    members(units, me, &last)
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
