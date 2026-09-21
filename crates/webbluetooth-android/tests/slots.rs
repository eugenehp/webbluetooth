//! Check every JNI vtable index against the JDK's own `jni.h`.
//!
//! The crate reaches JNI by slot index, because Android gives us a C struct of
//! function pointers and no linker symbols to bind against. An index that is
//! off by one does not fail to compile and does not usually crash — it calls
//! the neighbouring function. `REGISTER_NATIVES` pointing at 216 instead of
//! 215 meant every call landed in `UnregisterNatives`, which accepts any class
//! and returns `JNI_OK`, so nothing looked wrong while no callback was ever
//! bound.
//!
//! A behavioural test only covers the slots it thinks to exercise, and only
//! when it is actually running. `jni.h` declares the whole table in order, so
//! parsing it checks every constant at once against the definition the VM was
//! built from. It needs no JVM — only the header.

/// Find the JDK headers. Skips when there is no JDK, like the JVM test does.
fn jni_h() -> Option<String> {
    let mut roots: Vec<String> = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        roots.push(home);
    }
    if let Ok(entries) = std::fs::read_dir("/usr/lib/jvm") {
        for entry in entries.flatten() {
            roots.push(entry.path().display().to_string());
        }
    }
    // macOS keeps them somewhere else entirely.
    roots.push("/Library/Java/JavaVirtualMachines/Contents/Home".into());
    for root in roots {
        if let Ok(text) = std::fs::read_to_string(format!("{root}/include/jni.h")) {
            return Some(text);
        }
    }
    None
}

/// The function pointers of one `struct`, in declaration order.
///
/// Every entry is either a `void *reservedN;` placeholder or a
/// `RET (JNICALL *Name)(args);` pointer, and both occupy a slot.
fn table(header: &str, name: &str) -> Vec<String> {
    let start = header
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("{name} not declared in jni.h"));
    let body = &header[start..];
    let end = body.find("\n};").expect("unterminated struct");
    let body = &body[..end];

    let mut slots = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("void *reserved") {
            // `void *reserved0;`
            if rest
                .trim_end_matches(';')
                .chars()
                .all(|c| c.is_ascii_digit())
            {
                slots.push(format!("reserved{}", rest.trim_end_matches(';')));
                continue;
            }
        }
        if let Some(at) = line.find("(JNICALL *") {
            let rest = &line[at + "(JNICALL *".len()..];
            let close = rest.find(')').expect("unterminated function pointer");
            slots.push(rest[..close].to_string());
        }
    }
    slots
}

/// `GetStringUTFChars` → `GET_STRING_UTF_CHARS`, matching how the crate spells
/// its constants: break before a capital that starts a new word, and keep runs
/// of capitals (`UTF`, `ID`, `VM`) together.
fn screaming(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_is_lower)
            {
                out.push('_');
            }
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Every `pub const NAME: usize = N;` inside `mod <module> {`.
fn constants(source: &str, module: &str) -> Vec<(String, usize)> {
    let start = source
        .find(&format!("mod {module} {{"))
        .unwrap_or_else(|| panic!("mod {module} not found"));
    let body = &source[start..];
    let end = body.find("\n}").expect("unterminated module");
    let mut found = Vec::new();
    for line in body[..end].lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": usize = ") else {
            continue;
        };
        let value = value.trim_end_matches(';');
        found.push((name.to_string(), value.parse().expect("slot is a number")));
    }
    assert!(!found.is_empty(), "mod {module} declared no constants");
    found
}

const SOURCE: &str = include_str!("../src/jni.rs");

fn check(module: &str, struct_name: &str, header: &str) {
    let slots = table(header, struct_name);
    let mut wrong = Vec::new();
    for (name, value) in constants(SOURCE, module) {
        let want = slots.iter().position(|s| screaming(s) == name);
        match want {
            None => wrong.push(format!("{name} names no function in {struct_name}")),
            Some(want) if want != value => wrong.push(format!(
                "{name} = {value}, but jni.h puts it at {want} — slot {value} is {}",
                slots.get(value).map_or("past the end of the table", |s| s)
            )),
            Some(_) => {}
        }
    }
    assert!(wrong.is_empty(), "{struct_name}:\n  {}", wrong.join("\n  "));
}

#[test]
fn every_slot_index_matches_the_jdk_header() {
    let Some(header) = jni_h() else {
        eprintln!("no jni.h — skipping");
        return;
    };
    check("slot", "JNINativeInterface_", &header);
    check("vm_slot", "JNIInvokeInterface_", &header);
}

/// The crate types the tables as fixed-size arrays, so their lengths are part
/// of the ABI too: indexing past the end would be undefined rather than wrong.
#[test]
fn table_lengths_match_the_jdk_header() {
    let Some(header) = jni_h() else {
        eprintln!("no jni.h — skipping");
        return;
    };
    assert_eq!(
        table(&header, "JNINativeInterface_").len(),
        234,
        "JNIEnv table size changed"
    );
    assert_eq!(
        table(&header, "JNIInvokeInterface_").len(),
        8,
        "JavaVM table size changed"
    );
    // …and the crate agrees.
    assert!(SOURCE.contains("[*const c_void; 234]"), "Env array size");
    assert!(SOURCE.contains("[*const c_void; 8]"), "Vm array size");
}

#[test]
fn screaming_matches_how_the_crate_spells_things() {
    assert_eq!(screaming("RegisterNatives"), "REGISTER_NATIVES");
    assert_eq!(screaming("GetStringUTFChars"), "GET_STRING_UTF_CHARS");
    assert_eq!(screaming("GetMethodID"), "GET_METHOD_ID");
    assert_eq!(screaming("GetJavaVM"), "GET_JAVA_VM");
    assert_eq!(screaming("NewDirectByteBuffer"), "NEW_DIRECT_BYTE_BUFFER");
    assert_eq!(screaming("DestroyJavaVM"), "DESTROY_JAVA_VM");
    assert_eq!(screaming("GetEnv"), "GET_ENV");
}
