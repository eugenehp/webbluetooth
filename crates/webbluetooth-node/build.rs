//! Node-API symbols come from the host, not from a library.
//!
//! An addon is a shared library that Node `dlopen`s, and the `napi_*` functions
//! are exported by the `node` executable itself. So there is nothing to link
//! against at build time, and the linker has to be told that the undefined
//! symbols are deliberate — otherwise it refuses, which is what it should
//! normally do.
//!
//! Every runtime that loads the addon resolves them: Node, Deno and Bun all
//! export the same ABI.
fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Applied to every link target, not just the `cdylib`. The test binary is
    // an executable built from the same code, and an executable with undefined
    // symbols does not link at all on ELF — so without this the crate could
    // only be tested by standing in something fake for the runtime, which is a
    // worse answer than telling the linker the truth: these symbols arrive at
    // load time.
    match os.as_str() {
        // The Mach-O linker refuses undefined symbols by default. This is the
        // flag Node's own build documentation prescribes for addons.
        "macos" | "ios" => {
            println!("cargo::rustc-link-arg=-Wl,-undefined,dynamic_lookup");
        }
        // Windows needs nothing here: the imports are named by `raw-dylib` in
        // `napi.rs`, which synthesises the stubs from the declarations rather
        // than taking them from an import library.
        "windows" => {}
        // ELF: a shared object already tolerates undefined symbols, but the
        // test harness is an executable and does not.
        //
        // The unqualified form, deliberately. The `-bins` and `-tests`
        // variants both name target *kinds* this package does not have — it
        // is a `cdylib` plus an `rlib`, and its tests are `#[cfg(test)]`
        // modules compiled into the lib target rather than a separate test
        // target. Cargo rejects a link argument aimed at a kind that is
        // absent, and a build-script error fails the whole workspace, not
        // just this package.
        _ => println!("cargo::rustc-link-arg=-Wl,--unresolved-symbols=ignore-all"),
    }
}
