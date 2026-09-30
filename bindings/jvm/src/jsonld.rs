//! JSON-LD document and Verifiable Credential native methods: a single dispatch method, since
//! the operation set is JSON in, JSON out either way (mirrors `bindings/node`'s `jsonld_op`).

use crate::error::AppError;
use crate::handle;
use crate::jni_util::{get_opt_string, get_string, run_string};
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use jni::JNIEnv;
use oxilite::blocking::Store;
use oxilite_core::json::json_to_graph;
use serde_json::{json, Value};

fn jsonld_options(o: &Value) -> Result<oxilite::jsonld::JsonLdOptions, AppError> {
    jsonld_options_from(o, oxilite::jsonld::JsonLdOptions::default())
}

fn jsonld_options_from(
    o: &Value,
    base: oxilite::jsonld::JsonLdOptions,
) -> Result<oxilite::jsonld::JsonLdOptions, AppError> {
    let mut opts = oxilite::jsonld::json::options_from_json(o, base)?;
    if o.get("network").and_then(Value::as_bool).unwrap_or(false) {
        opts.fetcher = Some(oxilite::jsonld::http_fetcher());
    }
    Ok(opts)
}

/// One operation of a document handle; `args` is the operation's JSON arguments.
fn document_op<B, S>(
    h: &oxilite::jsonld::JsonLdStore<'_, B, S>,
    op: &str,
    args: &Value,
) -> Result<Value, oxilite::jsonld::JsonLdError>
where
    B: oxilite_core::SyncBackend + Send + Sync + 'static,
    S: oxilite::jsonld::Loader,
{
    use oxilite::jsonld::json::*;
    use oxilite::jsonld::JsonLdError;
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let key = || s("key").ok_or_else(|| JsonLdError::Invalid("missing `key`".into()));
    Ok(match op {
        "put" => outcome_to_json(&h.put_documents(inputs_from_json(&args["documents"])?)?),
        "get" => h
            .get_document(key()?)?
            .as_ref()
            .map_or(Value::Null, document_to_json),
        "remove" => json!(h.remove_document(key()?)?),
        "list" => documents_to_json(&h.list_documents(
            s("after"),
            args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
        )?),
        "find" => documents_to_json(&h.find_documents(&filter_from_json(args)?)?),
        "graphs" => graphs_to_json(&h.document_graphs(key()?)?),
        "documentForGraph" => {
            let g = json_to_graph(&args["graph"]).map_err(JsonLdError::Store)?;
            h.document_for_graph(&g)?
                .as_ref()
                .map_or(Value::Null, document_to_json)
        }
        "putContext" => {
            let ctx = match &args["context"] {
                Value::String(t) => t.clone(),
                v => v.to_string(),
            };
            h.put_context(s("iri").unwrap_or_default(), &ctx)?;
            Value::Null
        }
        "removeContext" => {
            h.remove_context(s("iri").unwrap_or_default())?;
            Value::Null
        }
        "contexts" => json!(h.contexts()?),
        "rebuild" => json!(h.rebuild_graph(key()?)?),
        "check" => drifts_to_json(&h.check_documents()?),
        other => return Err(JsonLdError::Invalid(format!("unknown operation {other}"))),
    })
}

/// A JSON-LD or (with `"credentials": true` in the options) a Verifiable Credentials operation.
fn jsonld_op<B>(store: &Store<B>, op: &str, args: &Value, opts: &Value) -> Result<Value, AppError>
where
    B: oxilite_core::SyncBackend + Send + Sync + 'static,
{
    use oxilite::jsonld::json::documents_to_json;
    if opts
        .get("credentials")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut co = oxilite::vc::CredentialOptions::default();
        co.jsonld = jsonld_options_from(opts, co.jsonld)?;
        co.embed_presentation_credentials = opts
            .get("embedCredentials")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let h = store.credentials_with(co)?;
        let json = || args.get("json").and_then(Value::as_str).unwrap_or_default();
        let r = match op {
            "putCredential" => match args.get("key").and_then(Value::as_str) {
                Some(k) => h.put_credential_with_key(k, json()).map(|k| json!(k)),
                None => h.put_credential(json()).map(|k| json!(k)),
            },
            "putPresentation" => h
                .put_presentation(json())
                .map(|k| json!({"key": k.key, "credentials": k.credentials})),
            "find" => oxilite::jsonld::json::filter_from_json(args)
                .and_then(|f| h.find_credentials(&f))
                .map(|d| documents_to_json(&d)),
            _ => document_op(h.documents(), op, args),
        };
        return Ok(r?);
    }
    let h = store.jsonld_with(jsonld_options(opts)?)?;
    Ok(document_op(&h, op, args)?)
}

#[no_mangle]
pub extern "system" fn Java_com_oxilitedb_oxilite_NativeStore_nativeJsonld<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    op: JString<'local>,
    args: JString<'local>,
    options: JString<'local>,
) -> jstring {
    let op = get_string(&mut env, &op);
    let args = get_string(&mut env, &args);
    let options = get_opt_string(&mut env, &options);
    run_string(&mut env, move || -> Result<String, AppError> {
        let backend = unsafe { handle::backend_ref(handle) };
        let args: Value = serde_json::from_str(&args)?;
        let opts: Value = match options {
            Some(o) => serde_json::from_str(&o)?,
            None => Value::Null,
        };
        let v = handle::with_store!(backend, s => jsonld_op(s, &op, &args, &opts))?;
        Ok(v.to_string())
    })
}
