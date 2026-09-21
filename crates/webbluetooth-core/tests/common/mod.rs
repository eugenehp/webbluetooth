//! Reading an adapter's declared surface out of its source, so that several
//! adapters can be compared against each other.
//!
//! Every platform in this workspace is written twice over: once as FFI and
//! once as an adapter presenting the portable model. Exactly one adapter is
//! ever compiled, which means the compiler checks one of them and the other
//! five are whatever they were last time somebody targeted them.
//!
//! This is the machinery both surface tests use. It compares names and types.
//! Whether the bodies behave the same is what the round-trip examples are for.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

/// One adapter's public declarations.
pub struct Surface {
    /// `Type::method` → `async method(Type, Type) -> Type`.
    pub methods: BTreeMap<String, String>,
    /// The types it declares.
    pub types: BTreeSet<String>,
}

/// A member an adapter is allowed not to have, and why.
///
/// Kept deliberately short: "this platform is different" is the answer these
/// tests exist to stop being given casually.
pub struct Exception {
    pub member: &'static str,
    pub absent_from: &'static [&'static str],
    pub why: &'static str,
}

/// Read every `pub fn` and `pub async fn` in an inherent `impl`, plus the
/// types declared at the top level.
pub fn read(src: &str) -> Surface {
    let mut methods = BTreeMap::new();
    let mut types = BTreeSet::new();
    let mut current: Option<String> = None;
    let mut lines = src.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line
            .strip_prefix("pub struct ")
            .or(line.strip_prefix("pub enum "))
        {
            types.insert(
                rest.split(['(', '<', '{', ' ', ';'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            );
        }
        // An `impl` at column zero opens a block; a trait impl is somebody
        // else's contract, so only inherent ones count.
        if let Some(rest) = line.strip_prefix("impl ") {
            current = (!rest.contains(" for ")).then(|| {
                rest.trim_start_matches('<')
                    .split(['<', '{', ' '])
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string()
            });
        }

        let trimmed = line.trim_start();
        if !line.starts_with("    pub ") {
            continue;
        }
        let Some(rest) = trimmed
            .strip_prefix("pub async fn ")
            .or_else(|| trimmed.strip_prefix("pub fn "))
        else {
            continue;
        };
        let Some(ref owner) = current else { continue };

        let is_async = trimmed.starts_with("pub async fn ");
        let mut sig = rest.to_string();
        // A signature wraps when its argument list is long.
        while !sig.contains('{') && !sig.ends_with(';') {
            match lines.next() {
                Some(more) => {
                    sig.push(' ');
                    sig.push_str(more.trim());
                }
                None => break,
            }
        }
        let sig = sig.split('{').next().unwrap_or(&sig).trim().to_string();
        let name = sig
            .split(['(', '<'])
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        methods.insert(format!("{owner}::{name}"), normalise(&name, &sig, is_async));
    }
    Surface { methods, types }
}

/// Reduce a signature to what has to match.
///
/// Argument *names* are dropped: Apple calls a characteristic handle
/// `characteristic` and BlueZ calls it `handle`, which is a difference in
/// prose rather than in surface.
fn normalise(name: &str, sig: &str, is_async: bool) -> String {
    // The argument list's own closing paren, found by depth — `rfind` lands on
    // the one inside `-> Result<()>`, and then every signature reads the same
    // kind of wrong, which is the sort of agreement these tests must not reach.
    let open = sig.find('(').unwrap_or(0);
    let mut depth = 0;
    let mut close = sig.len();
    for (i, c) in sig.char_indices().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let args = &sig[open + 1..close];
    let ret = sig[close + 1..].trim().trim_start_matches("->").trim();

    let mut types: Vec<String> = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in args.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                types.push(arg_type(&current));
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        types.push(arg_type(&current));
    }
    types.retain(|t| !t.is_empty());
    format!(
        "{}{name}({}) -> {}",
        if is_async { "async " } else { "" },
        types.join(", "),
        canonical(ret)
    )
}

/// The type of one argument: everything after the colon, or the receiver.
fn arg_type(arg: &str) -> String {
    let arg = arg.trim();
    if arg.is_empty() {
        return String::new();
    }
    // `&self` and `self: &Arc<Self>` are the same contract as far as a caller
    // holding an `Arc` is concerned — which every caller here does.
    if arg == "&self" || arg == "&mut self" || arg == "self" || arg.starts_with("self:") {
        return "self".into();
    }
    canonical(arg.split_once(':').map_or(arg, |(_, t)| t))
}

/// Spell a type the same way whichever adapter wrote it.
pub fn canonical(t: &str) -> String {
    let mut t = t.trim().to_string();
    for prefix in ["webbluetooth_core::", "crate::", "std::", "super::"] {
        t = t.replace(prefix, "");
    }
    // Every adapter's attribute handle is its own type; Apple's is a retained
    // Objective-C pointer and the rest are structs. The contract is that there
    // is one, not what it is.
    t = t.replace("Retained", "Handle");
    for module in [
        "state::",
        "error::",
        "filter::",
        "uuid::",
        "gatt::",
        "adapter::",
        "defs::",
        "peripheral::",
        "l2cap::",
    ] {
        t = t.replace(module, "");
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn excused<'a>(exceptions: &'a [Exception], member: &str, adapter: &str) -> Option<&'a Exception> {
    exceptions
        .iter()
        .find(|e| e.member == member && e.absent_from.contains(&adapter))
}

/// Which members are missing somewhere without an entry saying why.
pub fn membership_problems(adapters: &[(&str, Surface)], exceptions: &[Exception]) -> Vec<String> {
    let all: BTreeSet<&String> = adapters
        .iter()
        .flat_map(|(_, s)| s.methods.keys())
        .collect();
    let mut problems = Vec::new();
    for member in all {
        for (adapter, surface) in adapters {
            if surface.methods.contains_key(member) {
                continue;
            }
            if excused(exceptions, member, adapter).is_none() {
                problems.push(format!(
                    "  {adapter} has no `{member}`, and no exception says why"
                ));
            }
        }
    }
    problems
}

/// Which members are declared more than one way.
pub fn signature_problems(
    adapters: &[(&str, Surface)],
    polymorphic: &[(&str, &str)],
) -> Vec<String> {
    let all: BTreeSet<&String> = adapters
        .iter()
        .flat_map(|(_, s)| s.methods.keys())
        .collect();
    let mut problems = Vec::new();
    for member in all {
        if polymorphic.iter().any(|(m, _)| m == member) {
            continue;
        }
        let mut spellings: BTreeMap<&String, Vec<&str>> = BTreeMap::new();
        for (adapter, surface) in adapters {
            if let Some(sig) = surface.methods.get(member) {
                spellings.entry(sig).or_default().push(adapter);
            }
        }
        if spellings.len() > 1 {
            problems.push(format!(
                "\n  {member} is declared {} ways:",
                spellings.len()
            ));
            for (sig, who) in spellings {
                problems.push(format!("    [{}] {sig}", who.join(", ")));
            }
        }
    }
    problems
}

/// An exception that no longer applies is an exception that stops being read.
pub fn stale_exceptions(
    adapters: &[(&str, Surface)],
    exceptions: &[Exception],
    polymorphic: &[(&str, &str)],
) -> Vec<String> {
    let mut stale = Vec::new();
    for e in exceptions {
        for adapter in e.absent_from {
            match adapters.iter().find(|(n, _)| n == adapter) {
                None => stale.push(format!("  exception names an unknown adapter: {adapter}")),
                Some((_, surface)) if surface.methods.contains_key(e.member) => {
                    stale.push(format!(
                        "  {adapter} does have `{}` now — drop the exception",
                        e.member
                    ))
                }
                Some(_) => {}
            }
        }
    }
    for (member, _) in polymorphic {
        if !adapters
            .iter()
            .any(|(_, s)| s.methods.contains_key(*member))
        {
            stale.push(format!(
                "  nothing declares `{member}`, which is listed as polymorphic"
            ));
        }
    }
    stale
}

/// Which types are declared somewhere but not everywhere.
pub fn type_problems(adapters: &[(&str, Surface)], shared: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    for want in shared {
        for (adapter, surface) in adapters {
            if !surface.types.contains(*want) {
                problems.push(format!("  {adapter} does not declare `{want}`"));
            }
        }
    }
    problems
}
