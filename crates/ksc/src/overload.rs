//! Overloading: procs (and methods of a class) with the same name and
//! different parameters.
//!
//! Before the checker, each one of a name declared more than once gets a
//! name of its own with its parameter types, `area(real)` and
//! `area(real, real)`, and [`Program::overloads`] keeps which names a call
//! may mean. The checker picks the one whose parameters the arguments fit
//! (see `sema`). A name declared once keeps its name.

use std::collections::HashMap;

use crate::ast::*;
use crate::diag::Diagnostic;

/// `area(real, real)`: a proc's name with its parameter types.
fn signed(name: &str, params: &[Param]) -> String {
    let types: Vec<String> = params
        .iter()
        .map(|p| {
            // `x: &int` (written back to the caller) is not `x: int`.
            let by_ref = p.mode == Some(SelfMode::Lent) && !p.ty.ty.to_string().starts_with('&');
            if by_ref { format!("&{}", p.ty.ty) } else { p.ty.ty.to_string() }
        })
        .collect();
    format!("{name}({})", types.join(", "))
}

/// Gives each overloaded proc and method its own name.
pub fn rename(mut program: Program) -> Result<Program, Diagnostic> {
    let mut overloads: HashMap<String, Vec<String>> = HashMap::new();
    let mut count: HashMap<String, usize> = HashMap::new();
    // A generic proc is named `f<T>` by now: it counts as an `f`.
    let base_of = |name: &str| name.split('<').next().unwrap_or(name).to_string();
    // (Its copies for each type, `f<int>`, are one proc with it.)
    for item in &program.items {
        if let Item::Proc(p) = item
            && !p.instance
        {
            *count.entry(base_of(&p.name.name)).or_default() += 1;
        }
    }
    let mut renamed_external = Vec::new();
    for item in &mut program.items {
        match item {
            Item::Proc(p) if !p.instance && count[&base_of(&p.name.name)] > 1 => {
                if !p.generics.is_empty() || p.name.name.contains('<') {
                    return Err(Diagnostic::new(format!("generic proc `{}` cannot be overloaded", base_of(&p.name.name)), p.name.span)
                        .with_note("give the other procs another name"));
                }
                let base = p.name.name.clone();
                p.name.name = signed(&base, &p.params);
                // A proc of another file stays one with its new name.
                if program.external.contains(&base) {
                    renamed_external.push(p.name.name.clone());
                }
                let list = overloads.entry(base).or_default();
                if list.contains(&p.name.name) {
                    return Err(Diagnostic::new(format!("`{}` is defined twice", p.name.name), p.name.span)
                        .with_note("overloads of a proc must differ in their parameter types"));
                }
                list.push(p.name.name.clone());
            }
            Item::Class(c) => {
                let mut count: HashMap<String, usize> = HashMap::new();
                for m in &c.methods {
                    *count.entry(m.decl.name.name.clone()).or_default() += 1;
                }
                for m in &mut c.methods {
                    if count[&m.decl.name.name] < 2 {
                        continue;
                    }
                    if let Some(a) = m.annotations.iter().find(|a| a.name.name == "override") {
                        return Err(Diagnostic::new(format!("`{}` fills in an interface method, so it cannot be overloaded", m.decl.name.name), a.name.span)
                            .with_note("an interface calls its methods by name: give the other methods another name"));
                    }
                    let base = m.decl.name.name.clone();
                    m.decl.name.name = signed(&base, &m.decl.params);
                    let list = overloads.entry(format!("{}.{base}", c.name.name)).or_default();
                    if list.contains(&m.decl.name.name) {
                        return Err(Diagnostic::new(format!("`{}` is defined twice", m.decl.name.name), m.decl.name.span)
                            .with_note("overloads of a method must differ in their parameter types"));
                    }
                    list.push(m.decl.name.name.clone());
                }
            }
            _ => {}
        }
    }
    program.external.extend(renamed_external);
    program.overloads = overloads;
    Ok(program)
}
