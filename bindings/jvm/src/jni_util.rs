//! Small helpers shared by every native method: string conversion, and running a method body
//! so a Rust panic never unwinds across the FFI boundary (undefined behaviour in JNI) and an
//! `AppError` always becomes the right Java exception instead of a generic native failure.

use crate::error::{self, AppError};
use jni::objects::JString;
use jni::sys::{jboolean, jlong, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub fn get_string(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s)
        .map(|s| s.into())
        .unwrap_or_else(|_| String::new())
}

pub fn get_opt_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        None
    } else {
        Some(get_string(env, s))
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Runs `f`, returning the JSON string it produces or throwing the matching exception; `null`
/// on error (the JVM raises the pending exception as soon as the native call returns).
pub fn run_string(env: &mut JNIEnv, f: impl FnOnce() -> Result<String, AppError>) -> jstring {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(s)) => env
            .new_string(s)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        Ok(Err(e)) => {
            error::throw(env, e);
            std::ptr::null_mut()
        }
        Err(p) => {
            error::throw_panic(env, &panic_message(p));
            std::ptr::null_mut()
        }
    }
}

/// Like [`run_string`], but for a method with no return value.
pub fn run_unit(env: &mut JNIEnv, f: impl FnOnce() -> Result<(), AppError>) {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error::throw(env, e),
        Err(p) => error::throw_panic(env, &panic_message(p)),
    }
}

/// Like [`run_string`], but for a `boolean`-returning method.
pub fn run_bool(env: &mut JNIEnv, f: impl FnOnce() -> Result<bool, AppError>) -> jboolean {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(b)) => {
            if b {
                JNI_TRUE
            } else {
                JNI_FALSE
            }
        }
        Ok(Err(e)) => {
            error::throw(env, e);
            JNI_FALSE
        }
        Err(p) => {
            error::throw_panic(env, &panic_message(p));
            JNI_FALSE
        }
    }
}

/// Like [`run_string`], but for a `long`-returning method.
pub fn run_long(env: &mut JNIEnv, f: impl FnOnce() -> Result<i64, AppError>) -> jlong {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(n)) => n as jlong,
        Ok(Err(e)) => {
            error::throw(env, e);
            0
        }
        Err(p) => {
            error::throw_panic(env, &panic_message(p));
            0
        }
    }
}

/// Runs a fallible constructor, boxing its `Backend`/handle-yielding output; `0` on error.
pub fn run_create(env: &mut JNIEnv, f: impl FnOnce() -> Result<jlong, AppError>) -> jlong {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            error::throw(env, e);
            0
        }
        Err(p) => {
            error::throw_panic(env, &panic_message(p));
            0
        }
    }
}
