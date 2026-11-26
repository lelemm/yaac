//! The controller's platform hooks ([`yaac_app::platform::PlatformHooks`]) answered by the
//! Kotlin object `dev.yaac.android.core.PlatformBridge` (`android/app/src/main/java/dev/yaac/
//! android/core/PlatformBridge.kt`).
//!
//! The hooks are called on the controller's worker threads, never its loop or the Android UI
//! thread, and they block until the user is done (a picker can stay open for minutes). The
//! Kotlin side runs the system UI on the main thread and completes a future the worker waits
//! on. Each call attaches its thread to the JVM for the duration of the call.
//!
//! The class is looked up once, from `NativeController.start` (a Java thread, so the app's
//! class loader is the one in effect; a thread the JVM did not start would only see the
//! system classes). Without the class (an app that does not ship it) the hooks stay unset and
//! every call answers "cancelled".
//!
//! The Kotlin methods are `@JvmStatic`, and every `String` result is a local path:
//!
//! | Method | Signature |
//! | --- | --- |
//! | `pickFiles(title)` | `(String) -> String[]?` |
//! | `pickFile(title, filterName, extensions)` | `(String, String?, String[]) -> String?` |
//! | `pickFolder(title)` | `(String) -> String?` |
//! | `saveFile(name, filterName, extensions, staged)` | `(String, String?, String[], String) -> Boolean` |
//! | `message(title, text)` | `(String, String) -> Unit` |
//! | `clipboardText()` | `() -> String?` |
//! | `clipboardFiles()` | `() -> String[]?` |
//! | `openUrl(url)` / `openDocument(path)` | `(String) -> Boolean` |

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};
use yaac_app::platform::{Filter, PlatformHooks};

const CLASS: &str = "dev/yaac/android/core/PlatformBridge";
const STRING: &str = "Ljava/lang/String;";

struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
}

static INSTALLED: OnceLock<bool> = OnceLock::new();

/// Looks the bridge class up and installs the hooks, once per process. Call from a Java
/// thread. Returns whether the hooks are in place.
pub(crate) fn install(env: &mut JNIEnv<'_>) -> bool {
    *INSTALLED.get_or_init(|| {
        let found = env.find_class(CLASS).and_then(|class| env.new_global_ref(class));
        let Ok(class) = found else {
            let _ = env.exception_clear();
            return false;
        };
        let Ok(vm) = env.get_java_vm() else { return false };
        yaac_app::platform::set_hooks(Box::new(Bridge { vm, class }))
    })
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlatformBridge")
    }
}

impl Bridge {
    /// Runs `f` on this thread attached to the JVM. A Java exception is cleared and is `None`.
    fn call<T>(&self, f: impl for<'a> FnOnce(&mut JNIEnv<'a>, &JClass<'a>) -> jni::errors::Result<T>) -> Option<T> {
        let mut env = self.vm.attach_current_thread().ok()?;
        let class: &JClass<'_> = <&JClass<'_>>::from(self.class.as_obj());
        let result = env.with_local_frame(16, |env| f(env, class).map(Some)).ok().flatten();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return None;
        }
        result
    }
}

fn jstr<'a>(env: &mut JNIEnv<'a>, s: &str) -> jni::errors::Result<JObject<'a>> {
    env.new_string(s).map(JObject::from)
}

fn opt_jstr<'a>(env: &mut JNIEnv<'a>, s: Option<&str>) -> jni::errors::Result<JObject<'a>> {
    match s {
        Some(s) => jstr(env, s),
        None => Ok(JObject::null()),
    }
}

fn jstrs<'a>(env: &mut JNIEnv<'a>, items: &[&str]) -> jni::errors::Result<JObject<'a>> {
    let len = i32::try_from(items.len()).unwrap_or(i32::MAX);
    let array = env.new_object_array(len, "java/lang/String", JObject::null())?;
    for (i, item) in (0..len).zip(items) {
        let s = env.new_string(item)?;
        env.set_object_array_element(&array, i, s)?;
    }
    Ok(JObject::from(array))
}

fn read_str(env: &mut JNIEnv<'_>, obj: JObject<'_>) -> jni::errors::Result<Option<String>> {
    if obj.is_null() {
        return Ok(None);
    }
    let s = JString::from(obj);
    let text: String = env.get_string(&s)?.into();
    Ok(Some(text))
}

fn read_strs(env: &mut JNIEnv<'_>, obj: JObject<'_>) -> jni::errors::Result<Option<Vec<String>>> {
    if obj.is_null() {
        return Ok(None);
    }
    let array = JObjectArray::from(obj);
    let len = env.get_array_length(&array)?;
    let mut out = Vec::new();
    for i in 0..len {
        let item = env.get_object_array_element(&array, i)?;
        if let Some(s) = read_str(env, item)? {
            out.push(s);
        }
    }
    Ok(Some(out))
}

fn paths(items: Option<Vec<String>>) -> Option<Vec<PathBuf>> {
    let items: Vec<PathBuf> = items?.into_iter().map(PathBuf::from).collect();
    (!items.is_empty()).then_some(items)
}

fn filter_parts(filter: Option<Filter<'_>>) -> (Option<&str>, &[&str]) {
    filter.map_or((None, &[]), |f| (Some(f.name), f.extensions))
}

impl PlatformHooks for Bridge {
    fn pick_files(&self, title: &str) -> Option<Vec<PathBuf>> {
        let found = self.call(|env, class| {
            let title = jstr(env, title)?;
            let out = env.call_static_method(class, "pickFiles", format!("({STRING})[{STRING}"), &[JValue::Object(&title)])?.l()?;
            read_strs(env, out)
        });
        paths(found.flatten())
    }

    fn pick_file(&self, title: &str, filter: Option<Filter<'_>>) -> Option<PathBuf> {
        let (name, extensions) = filter_parts(filter);
        self.call(|env, class| {
            let title = jstr(env, title)?;
            let name = opt_jstr(env, name)?;
            let extensions = jstrs(env, extensions)?;
            let out = env
                .call_static_method(
                    class,
                    "pickFile",
                    format!("({STRING}{STRING}[{STRING}){STRING}"),
                    &[JValue::Object(&title), JValue::Object(&name), JValue::Object(&extensions)],
                )?
                .l()?;
            read_str(env, out)
        })
        .flatten()
        .map(PathBuf::from)
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        self.call(|env, class| {
            let title = jstr(env, title)?;
            let out = env.call_static_method(class, "pickFolder", format!("({STRING}){STRING}"), &[JValue::Object(&title)])?.l()?;
            read_str(env, out)
        })
        .flatten()
        .map(PathBuf::from)
    }

    fn save_file(&self, name: &str, filter: Option<Filter<'_>>, staged: &Path) -> std::io::Result<bool> {
        let (filter_name, extensions) = filter_parts(filter);
        let staged = staged.to_string_lossy();
        self.call(|env, class| {
            let name = jstr(env, name)?;
            let filter_name = opt_jstr(env, filter_name)?;
            let extensions = jstrs(env, extensions)?;
            let staged = jstr(env, &staged)?;
            env.call_static_method(
                class,
                "saveFile",
                format!("({STRING}{STRING}[{STRING}{STRING})Z"),
                &[JValue::Object(&name), JValue::Object(&filter_name), JValue::Object(&extensions), JValue::Object(&staged)],
            )?
            .z()
        })
        .ok_or_else(|| std::io::Error::other("the file could not be saved"))
    }

    fn message(&self, title: &str, text: &str) {
        self.call(|env, class| {
            let title = jstr(env, title)?;
            let text = jstr(env, text)?;
            env.call_static_method(class, "message", format!("({STRING}{STRING})V"), &[JValue::Object(&title), JValue::Object(&text)])?.v()
        });
    }

    fn clipboard_text(&self) -> Option<String> {
        self.call(|env, class| {
            let out = env.call_static_method(class, "clipboardText", format!("(){STRING}"), &[])?.l()?;
            read_str(env, out)
        })
        .flatten()
    }

    fn clipboard_files(&self) -> Option<Vec<PathBuf>> {
        let found = self.call(|env, class| {
            let out = env.call_static_method(class, "clipboardFiles", format!("()[{STRING}"), &[])?.l()?;
            read_strs(env, out)
        });
        paths(found.flatten())
    }

    fn open_url(&self, url: &str) -> std::io::Result<()> {
        self.open("openUrl", url)
    }

    fn open_document(&self, path: &Path) -> std::io::Result<()> {
        self.open("openDocument", &path.to_string_lossy())
    }
}

impl Bridge {
    fn open(&self, method: &str, what: &str) -> std::io::Result<()> {
        let opened = self.call(|env, class| {
            let what = jstr(env, what)?;
            env.call_static_method(class, method, format!("({STRING})Z"), &[JValue::Object(&what)])?.z()
        });
        match opened {
            Some(true) => Ok(()),
            _ => Err(std::io::Error::other("no app on this device can open it")),
        }
    }
}
