//! Moving around the code, for an editor (`ksc nav`): where a name is
//! declared, which classes implement an interface (and its methods), which
//! interface method a class's method fills in, and the interfaces of a
//! class. By name, as [`crate::docs`]; `@impl` tells classes and interfaces apart.

use crate::ast::{ClassDecl, InterfaceDecl, Item, short_name};
use crate::diag::SourceMap;
use crate::docs::Target;
use crate::modules::Unit;

/// What a [`Target`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Class,
    Interface,
    Struct,
    Enum,
    Proc,
    Const,
    Method,
    Field,
    /// A value of an enum (`TokenKind.Bang`).
    Value,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Class => "class",
            Kind::Interface => "interface",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::Proc => "proc",
            Kind::Const => "const",
            Kind::Method => "method",
            Kind::Field => "field",
            Kind::Value => "value",
        }
    }
}

/// A place to go: what it is, its name, file, line and column (from 1;
/// the column counts characters).
pub struct Place {
    pub kind: Kind,
    pub name: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// Names it again for a later question: `std.collections.List#List.add`.
    pub id: String,
}

/// The classes and interfaces of every file, by their full name.
fn classes(units: &[Unit]) -> impl Iterator<Item = (usize, &ClassDecl)> {
    units.iter().enumerate().flat_map(|(u, unit)| {
        unit.program.items.iter().filter_map(move |i| if let Item::Class(c) = i { Some((u, c)) } else { None })
    })
}

fn interfaces(units: &[Unit]) -> impl Iterator<Item = (usize, &InterfaceDecl)> {
    units.iter().enumerate().flat_map(|(u, unit)| {
        unit.program.items.iter().filter_map(move |i| if let Item::Interface(d) = i { Some((u, d)) } else { None })
    })
}

/// The interfaces a class names in `@impl(...)` (full names).
fn impls(c: &ClassDecl) -> Vec<&str> {
    c.annotations.iter().filter(|a| a.name.name == "impl").flat_map(|a| a.args.iter().map(|i| i.name.as_str())).collect()
}

/// What `(u, key)` is.
pub fn kind(units: &[Unit], (u, key): &Target) -> Option<Kind> {
    if key.is_empty() {
        return Some(Kind::File);
    }
    let program = &units[*u].program;
    let key = key.split('~').next().unwrap_or(key);
    let item = |name: &str| program.items.iter().find(|i| short_name(&crate::parser::item_name(i).name) == name);
    if let Some((owner, member)) = key.split_once('.') {
        return Some(match item(owner)? {
            Item::Class(c) if c.methods.iter().any(|m| m.decl.name.name == member) => Kind::Method,
            Item::Class(_) | Item::Struct(_) => Kind::Field,
            Item::Enum(_) => Kind::Value,
            Item::Interface(_) => Kind::Method,
            _ => return None,
        });
    }
    Some(match item(key)? {
        Item::Class(_) => Kind::Class,
        Item::Interface(_) => Kind::Interface,
        Item::Struct(_) => Kind::Struct,
        Item::Enum(_) => Kind::Enum,
        Item::Proc(_) => Kind::Proc,
        Item::Const(_) => Kind::Const,
    })
}

/// Where `target` is declared.
pub fn place(units: &[Unit], map: &SourceMap, target: &Target) -> Option<Place> {
    let (u, key) = target;
    let at = units[*u].program.docs.get(key).map_or(units[*u].base, |d| d.at);
    place_of(units, map, target, at)
}

/// A place where `target` is named: byte `offset` of unit `u`'s file.
pub fn reference(units: &[Unit], map: &SourceMap, target: &Target, u: usize, offset: usize) -> Option<Place> {
    place_of(units, map, target, units[u].base + offset)
}

/// `target`, shown at span position `at`.
fn place_of(units: &[Unit], map: &SourceMap, target: &Target, at: usize) -> Option<Place> {
    let (u, key) = target;
    let f = map.file_at(at)?;
    let before = f.src.get(..at - f.base)?;
    let line = before.matches('\n').count() as u32 + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1;
    let name = if key.is_empty() { units[*u].info.path.clone() } else { key.split('~').next().unwrap_or(key).to_string() };
    Some(Place { kind: kind(units, target)?, name, file: f.path.clone(), line, column, id: format!("{}#{key}", units[*u].info.path) })
}

/// The target an [`Place::id`] names.
pub fn by_id(units: &[Unit], id: &str) -> Option<Target> {
    let (path, key) = id.split_once('#')?;
    let u = units.iter().position(|u| u.info.path == path)?;
    Some((u, key.to_string()))
}

/// A file stands for its main item (`Shape` for `Shape.kstl`), when it has one.
fn main_item(units: &[Unit], (u, key): &Target) -> Target {
    let alias = units[*u].info.alias();
    if key.is_empty() && units[*u].program.docs.contains_key(alias) {
        return (*u, alias.to_string());
    }
    (*u, key.clone())
}

/// The full name of a class or interface named `key` in file `u`.
fn full(units: &[Unit], u: usize, key: &str) -> String {
    units[u].info.mangle(key)
}

/// The classes that implement interface `key` of file `u`, or (for an
/// interface method `I.m`) their methods `m`.
pub fn implementations(units: &[Unit], target: &Target) -> Vec<Target> {
    let target = &main_item(units, target);
    let (u, key) = target;
    let (iface, method) = match key.split_once('.') {
        Some((i, m)) => (i, Some(m)),
        None => (key.as_str(), None),
    };
    if kind(units, &(*u, iface.to_string())) != Some(Kind::Interface) {
        return Vec::new();
    }
    let name = full(units, *u, iface);
    let mut out = Vec::new();
    for (v, c) in classes(units).filter(|(_, c)| impls(c).contains(&name.as_str())) {
        let class = short_name(&c.name.name).to_string();
        match method {
            None => out.push((v, class)),
            Some(m) if c.methods.iter().any(|d| d.decl.name.name == m) => out.push((v, format!("{class}.{m}"))),
            Some(_) => {}
        }
    }
    out
}

/// The interfaces a class implements, or (for a class's method `C.m`) the
/// interface methods `m` it fills in.
pub fn supers(units: &[Unit], target: &Target) -> Vec<Target> {
    let target = &main_item(units, target);
    let (u, key) = target;
    let (class, method) = match key.split_once('.') {
        Some((c, m)) => (c, Some(m)),
        None => (key.as_str(), None),
    };
    let name = full(units, *u, class);
    let Some((_, c)) = classes(units).find(|(v, c)| *v == *u && c.name.name == name) else { return Vec::new() };
    let mut out = Vec::new();
    for i in impls(c) {
        let Some((v, d)) = interfaces(units).find(|(_, d)| d.name.name == i) else { continue };
        let iface = short_name(&d.name.name).to_string();
        match method {
            None => out.push((v, iface)),
            Some(m) if d.methods.iter().any(|x| x.name.name == m) => out.push((v, format!("{iface}.{m}"))),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::diag::SourceMap;
    use crate::modules::load;

    #[test]
    fn implementations_and_supers() {
        let files: HashMap<String, String> = [(
            "Shape".to_string(),
            "public interface Shape { proc area(&self) -> real; }".to_string(),
        )]
        .into();
        let main = "use Shape;\n@impl(Shape)\nclass Circle {\n    r: real;\n    @override\n    public proc area(&self) -> real { return self.r; }\n}\nproc main() {}\n";
        let mut map = SourceMap::default();
        let units = load("main.kstl", main, &files, &mut map).unwrap();
        let names = |ts: Vec<crate::docs::Target>| ts.into_iter().map(|(_, k)| k).collect::<Vec<_>>();
        let shape = (units.iter().position(|u| u.info.path == "Shape").unwrap(), "Shape".to_string());
        assert_eq!(names(super::implementations(&units, &shape)), ["Circle"]);
        assert_eq!(names(super::implementations(&units, &(shape.0, "Shape.area".into()))), ["Circle.area"]);
        assert_eq!(names(super::supers(&units, &(0, "Circle".into()))), ["Shape"]);
        assert_eq!(names(super::supers(&units, &(0, "Circle.area".into()))), ["Shape.area"]);
        // The `area` under the cursor in `class Circle` is that method.
        let at = main.find("area(&self)").unwrap();
        assert_eq!(names(crate::docs::targets(&units, &map, 0, at)), ["Circle.area"]);
        let place = super::place(&units, &map, &(0, "Circle.area".into())).unwrap();
        assert_eq!((place.line, place.column, place.kind), (5, 5, super::Kind::Method));
    }
}
