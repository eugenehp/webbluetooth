//! Turning a generated DEX into a live class.
//!
//! The Apple backend synthesises a delegate with `objc_allocateClassPair` and
//! installs Rust functions with `class_addMethod`. Android's equivalent is
//! three steps rather than two, because the class arrives as a *file*:
//!
//! 1. [`crate::dex`] emits a DEX defining the subclass, its overrides `native`.
//! 2. `InMemoryDexClassLoader` loads it — from a `ByteBuffer` over memory we
//!    own, so nothing is ever written to disk.
//! 3. `RegisterNatives` binds each `native` method to a Rust function.
//!
//! The result is a class Android cannot distinguish from a compiled one. It has
//! a real superclass, a real constructor, and real methods; they simply happen
//! to be implemented in Rust.
//!
//! # Two things that will bite
//!
//! * **`InMemoryDexClassLoader` is API 26+.** Below that the DEX has to reach a
//!   file, and `DexClassLoader` needs a writable optimised-output directory.
//!   There is no fallback here; the crate targets 26 and up.
//! * **The DEX bytes must outlive the loader.** `NewDirectByteBuffer` does not
//!   copy, so the buffer is leaked deliberately — one allocation per class, for
//!   the life of the process.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::jni::{Env, JClass, JObject, JValue, NativeMethod, Vm};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Mutex, OnceLock};

/// The pieces of the Android runtime this crate needs, resolved once.
pub struct Runtime {
    vm: Vm,
    /// The application `Context`, as a global reference.
    context: JObject,
    /// `dev/webbluetooth/…` → the loaded class, as a global reference.
    classes: Mutex<HashMap<String, JClass>>,
}

// Every field is a global reference or the VM itself, both valid on any thread.
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Why the runtime could not start.
#[derive(Debug, Clone)]
pub enum Error {
    /// No thread could be attached to the VM.
    NoEnv,
    /// A framework class was missing — almost always an API level below 26.
    MissingClass(String),
    /// A method or constructor was missing.
    MissingMethod(String),
    /// The generated class did not load.
    ClassLoad(String),
    /// `RegisterNatives` refused the table, meaning a signature disagrees with
    /// the DEX.
    RegisterNatives(String),
    /// No Android `Context` was supplied and none could be found.
    NoContext,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEnv => f.write_str("could not attach to the Java VM"),
            Self::MissingClass(c) => write!(f, "class not found: {c} (is minSdk at least 26?)"),
            Self::MissingMethod(m) => write!(f, "method not found: {m}"),
            Self::ClassLoad(c) => write!(f, "the generated class {c} did not load"),
            Self::RegisterNatives(c) => {
                write!(
                    f,
                    "RegisterNatives refused {c} — a signature disagrees with the DEX"
                )
            }
            Self::NoContext => f.write_str(
                "no Android Context: call Runtime::init(vm, context) from JNI_OnLoad or your \
                 Activity, or run inside an app where ActivityThread.currentApplication() works",
            ),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

impl Runtime {
    /// Start the runtime with a VM and an Android `Context`.
    ///
    /// `context` may be null, in which case the application context is looked
    /// up through `ActivityThread.currentApplication()` — which works inside a
    /// normal app process and nowhere else.
    pub fn init(vm: Vm, context: JObject) -> Result<&'static Runtime> {
        if let Some(existing) = RUNTIME.get() {
            return Ok(existing);
        }
        let env = vm.attach().ok_or(Error::NoEnv)?;

        let context = if context.is_null() {
            current_application(env).ok_or(Error::NoContext)?
        } else {
            context
        };
        let context = env.new_global_ref(context);

        let runtime = Runtime {
            vm,
            context,
            classes: Mutex::new(HashMap::new()),
        };
        Ok(RUNTIME.get_or_init(|| runtime))
    }

    /// The runtime, if [`Runtime::init`] has run.
    pub fn get() -> Option<&'static Runtime> {
        RUNTIME.get()
    }

    pub fn vm(&self) -> Vm {
        self.vm
    }

    /// The application `Context`, a global reference.
    pub fn context(&self) -> JObject {
        self.context
    }

    /// An environment for the calling thread, attaching it if necessary.
    pub fn env(&self) -> Result<Env> {
        self.vm.attach().ok_or(Error::NoEnv)
    }

    /// Define a class from a DEX and bind its native methods.
    ///
    /// Returns a global reference, cached — asking twice is free and yields the
    /// same class, which matters because two `Class` objects from two loaders
    /// are not interchangeable.
    pub fn define_class(
        &self,
        binary_name: &str,
        dex_bytes: Vec<u8>,
        natives: &[(&str, &str, *const std::ffi::c_void)],
    ) -> Result<JClass> {
        if let Some(existing) = self.classes.lock().unwrap().get(binary_name) {
            return Ok(*existing);
        }
        let env = self.env()?;

        // The buffer is not copied, so it must live as long as the loader. One
        // deliberate leak per class, for the life of the process.
        let leaked = Box::leak(dex_bytes.into_boxed_slice());
        let buffer = env.new_direct_byte_buffer(leaked.as_mut_ptr(), leaked.len());
        if buffer.is_null() {
            return Err(Error::ClassLoad(
                "could not wrap the dex in a ByteBuffer".into(),
            ));
        }

        // new InMemoryDexClassLoader(ByteBuffer, ClassLoader)
        let loader_class = env
            .find_class("dalvik/system/InMemoryDexClassLoader")
            .ok_or_else(|| Error::MissingClass("dalvik.system.InMemoryDexClassLoader".into()))?;
        let ctor = env
            .method_id(
                loader_class,
                "<init>",
                "(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
            )
            .ok_or_else(|| Error::MissingMethod("InMemoryDexClassLoader.<init>".into()))?;

        // Parent the loader on the app's, so framework classes resolve.
        let parent = self.class_loader(env).unwrap_or(std::ptr::null_mut());
        let loader = env.new_object(
            loader_class,
            ctor,
            &[JValue::object(buffer), JValue::object(parent)],
        );
        if loader.is_null() {
            return Err(Error::ClassLoad(binary_name.into()));
        }

        // loader.loadClass("dev.webbluetooth.GattCallback")
        let load_class = env
            .method_id(
                env.get_object_class(loader),
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
            )
            .ok_or_else(|| Error::MissingMethod("ClassLoader.loadClass".into()))?;
        let name = env.new_string(binary_name);
        let class = env.call_object(loader, load_class, &[JValue::object(name)]);
        if class.is_null() {
            return Err(Error::ClassLoad(binary_name.into()));
        }
        let class = env.new_global_ref(class);

        // Bind the native methods. The strings must outlive the call, which is
        // why they are collected before the table is built.
        let names: Vec<CString> = natives
            .iter()
            .map(|(n, _, _)| CString::new(*n).expect("method name"))
            .collect();
        let signatures: Vec<CString> = natives
            .iter()
            .map(|(_, s, _)| CString::new(*s).expect("signature"))
            .collect();
        let table: Vec<NativeMethod> = natives
            .iter()
            .enumerate()
            .map(|(i, (_, _, f))| NativeMethod {
                name: names[i].as_ptr(),
                signature: signatures[i].as_ptr(),
                function: *f,
            })
            .collect();

        if !table.is_empty() && !env.register_natives(class, &table) {
            return Err(Error::RegisterNatives(binary_name.into()));
        }

        self.classes
            .lock()
            .unwrap()
            .insert(binary_name.into(), class);
        Ok(class)
    }

    /// Instantiate one of the generated classes.
    pub fn new_instance(&self, class: JClass) -> Result<JObject> {
        let env = self.env()?;
        let ctor = env
            .method_id(class, "<init>", "()V")
            .ok_or_else(|| Error::MissingMethod("<init>".into()))?;
        let object = env.new_object(class, ctor, &[]);
        if object.is_null() {
            return Err(Error::ClassLoad(
                "could not instantiate the generated class".into(),
            ));
        }
        Ok(env.new_global_ref(object))
    }

    /// `context.getClassLoader()`.
    fn class_loader(&self, env: Env) -> Option<JObject> {
        let method = env.method_id(
            env.get_object_class(self.context),
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
        )?;
        let loader = env.call_object(self.context, method, &[]);
        (!loader.is_null()).then_some(loader)
    }
}

/// `ActivityThread.currentApplication()`, the standard way to reach a Context
/// with nothing but a VM.
///
/// Works in an ordinary app process. It is hidden API in the sense that no
/// public entry point returns it, but the method itself is public and stable,
/// and it is what every native library that needs a Context uses.
fn current_application(env: Env) -> Option<JObject> {
    let class = env.find_class("android/app/ActivityThread")?;
    let method =
        env.static_method_id(class, "currentApplication", "()Landroid/app/Application;")?;
    let application = env.call_static_object(class, method, &[]);
    (!application.is_null()).then_some(application)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_say_what_to_do() {
        // A missing class on Android almost always means the API level, and a
        // missing Context almost always means init was never called.
        assert!(
            Error::MissingClass("dalvik.system.InMemoryDexClassLoader".into())
                .to_string()
                .contains("minSdk")
        );
        assert!(Error::NoContext.to_string().contains("Runtime::init"));
        assert!(Error::RegisterNatives("X".into())
            .to_string()
            .contains("signature"));
    }

    #[test]
    fn the_runtime_is_absent_until_initialised() {
        // Every entry point checks this rather than assuming.
        assert!(Runtime::get().is_none());
    }
}
