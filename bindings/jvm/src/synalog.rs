//! Synalog native methods: a program evaluated over the store as relational tables.

use crate::error::AppError;
use crate::handle;
use crate::jni_util::{get_opt_string, get_string, run_string};
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;

pub(crate) fn synalog_args(options: Option<String>) -> Result<oxilite::synalog::Options, AppError> {
    match options {
        Some(o) => oxilite::synalog::json::options_from_json(&serde_json::from_str::<
            serde_json::Value,
        >(&o)?)
        .map_err(AppError::from),
        None => oxilite::synalog::json::options_from_json(&serde_json::Value::Null)
            .map_err(AppError::from),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSynalog<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    program: JString<'local>,
    predicate: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let program = get_string(&mut env, &program);
    let predicate = get_string(&mut env, &predicate);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let opts = synalog_args(options)?;
        let r = handle::with_store!(backend, s => s.synalog_with(&program, &predicate, &opts))?;
        Ok(oxilite::synalog::json::result_to_json(&r).to_string())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeSynalogSql<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    program: JString<'local>,
    predicate: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let program = get_string(&mut env, &program);
    let predicate = get_string(&mut env, &predicate);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let opts = synalog_args(options)?;
        Ok(handle::with_store!(backend, s => s.synalog_sql_with(&program, &predicate, &opts))?)
    })
}
