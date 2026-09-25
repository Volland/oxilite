//! JSON-LD documents and Verifiable Credentials as wasm jobs (`Engine::jsonld`).
//!
// @lat: [[architecture#Bindings]]

use crate::Job;
use oxilite_core::job::{Job as CoreJob, OneShot, Step};
use oxilite_core::{Capabilities, Error, Request};
use oxilite_jsonld::json::*;
use oxilite_jsonld::{
    document_for_graph_job, document_graphs_job, find_job, from_core, get_job, list_contexts_job,
    list_job, put_context_request, remove_context_request, schema, CheckJob, DocumentInput,
    JsonLdError, JsonLdOptions, Loader, NoContexts, RebuildJob, WriteJob,
};
use serde_json::{json, Value};

/// A JSON-LD error as the core error the JavaScript driver decodes
/// (`oxilite-jsonld:{"code", "message"}`).
fn wire(e: JsonLdError) -> Error {
    Error::Other(format!("oxilite-jsonld:{}", error_to_json(&e)))
}

/// Wraps a job whose errors carry JSON-LD errors.
fn job<J: CoreJob + 'static>(
    mut j: J,
    done: impl Fn(J::Output) -> Result<Value, JsonLdError> + 'static,
) -> Job {
    Job {
        ctx: None,
        pending: false,
        step: Box::new(move |r| match j.step(r) {
            Ok(Step::Execute(req)) => Ok(Step::Execute(req)),
            Ok(Step::Done(o)) => done(o).map(Step::Done).map_err(wire),
            Err(e) => Err(wire(from_core(e))),
        }),
    }
}

fn request(r: Request) -> Job {
    job(OneShot::new(r, |_| Ok(())), |()| Ok(Value::Null))
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a str, JsonLdError> {
    args.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| JsonLdError::Invalid(format!("missing `{k}`")))
}

fn write<S: Loader + 'static>(
    puts: Vec<DocumentInput>,
    removes: Vec<String>,
    options: JsonLdOptions,
    first: S,
    caps: &Capabilities,
    done: impl Fn(oxilite_jsonld::WriteOutcome) -> Result<Value, JsonLdError> + 'static,
) -> Result<Job, JsonLdError> {
    let w = WriteJob::new(puts, removes, options, first, caps.clone())?;
    Ok(job(w, done))
}

/// A document operation (see `@oxilite/node`'s `NativeStore::jsonld` for the list).
fn document_job<S: Loader + 'static>(
    op: &str,
    args: &Value,
    options: JsonLdOptions,
    first: S,
    caps: &Capabilities,
) -> Result<Job, JsonLdError> {
    Ok(match op {
        "schema" => request(schema::create_schema(&options.indexes)),
        "put" => write(
            inputs_from_json(&args["documents"])?,
            vec![],
            options,
            first,
            caps,
            |o| Ok(outcome_to_json(&o)),
        )?,
        "remove" => write(
            vec![],
            vec![arg(args, "key")?.to_owned()],
            options,
            first,
            caps,
            |o| Ok(json!(o.removed.first().copied().unwrap_or(false))),
        )?,
        "get" => job(get_job(arg(args, "key")?), |d| {
            Ok(d.first().map_or(Value::Null, document_to_json))
        }),
        "list" => job(
            list_job(
                args.get("after").and_then(Value::as_str),
                args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
            ),
            |d| Ok(documents_to_json(&d)),
        ),
        "find" => job(find_job(&filter_from_json(args)?), |d| {
            Ok(documents_to_json(&d))
        }),
        "graphs" => job(document_graphs_job(arg(args, "key")?), |g| {
            Ok(graphs_to_json(&g))
        }),
        "documentForGraph" => {
            let g =
                oxilite_core::json::json_to_graph(&args["graph"]).map_err(JsonLdError::Store)?;
            job(document_for_graph_job(g.as_ref()), |d| {
                Ok(d.first().map_or(Value::Null, document_to_json))
            })
        }
        "putContext" => {
            let ctx = match &args["context"] {
                Value::String(t) => t.clone(),
                v => v.to_string(),
            };
            request(put_context_request(arg(args, "iri")?, &ctx)?)
        }
        "removeContext" => request(remove_context_request(arg(args, "iri")?)),
        "contexts" => job(list_contexts_job(), |c| Ok(json!(c))),
        "rebuild" => job(
            RebuildJob::new(arg(args, "key")?, options, first, caps.clone()),
            |o| Ok(json!(o.is_some())),
        ),
        "check" => job(CheckJob::new(options, first), |d| Ok(drifts_to_json(&d))),
        other => return Err(JsonLdError::Invalid(format!("unknown operation {other}"))),
    })
}

/// A JSON-LD operation, or with `"credentials": true` a credentials operation.
pub fn jsonld_job(op: &str, args: &Value, opts: &Value, caps: &Capabilities) -> Result<Job, Error> {
    if opts
        .get("network")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(wire(JsonLdError::Invalid(
            "network context loading is not available in WebAssembly; persist contexts with putContext"
                .into(),
        )));
    }
    let credentials = opts
        .get("credentials")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if credentials {
        #[cfg(feature = "vc")]
        return credential_job(op, args, opts, caps).map_err(wire);
        #[cfg(not(feature = "vc"))]
        return Err(wire(JsonLdError::Invalid(
            "this build has no Verifiable Credentials support (feature `vc`)".into(),
        )));
    }
    let options = options_from_json(opts, JsonLdOptions::default()).map_err(wire)?;
    document_job(op, args, options, NoContexts, caps).map_err(wire)
}

#[cfg(feature = "vc")]
fn credential_job(
    op: &str,
    args: &Value,
    opts: &Value,
    caps: &Capabilities,
) -> Result<Job, JsonLdError> {
    let mut co = oxilite_vc::CredentialOptions::default();
    co.jsonld = options_from_json(opts, co.jsonld)?;
    co.embed_presentation_credentials = opts
        .get("embedCredentials")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let first = oxilite_vc::bundled_contexts();
    let key = args.get("key").and_then(Value::as_str).map(str::to_owned);
    Ok(match op {
        "putCredential" => {
            let input = oxilite_vc::credential_input(arg(args, "json")?, key)?;
            write(vec![input], vec![], co.jsonld, first, caps, |o| {
                Ok(json!(o.keys.first()))
            })?
        }
        "putPresentation" => {
            let inputs = oxilite_vc::presentation_inputs(arg(args, "json")?, key, &co)?;
            write(inputs, vec![], co.jsonld, first, caps, |o| {
                let mut keys = o.keys.into_iter();
                Ok(json!({"key": keys.next(), "credentials": keys.collect::<Vec<_>>()}))
            })?
        }
        _ => document_job(op, args, co.jsonld, first, caps)?,
    })
}

/// The schema plus the JSON-LD tables, as SQL; `indexes` is `{"issuer", "subject", "validUntil"}`.
pub fn schema_sql(options: &oxilite_core::StoreOptions, indexes: &Value) -> Result<String, Error> {
    let o =
        options_from_json(&json!({"indexes": indexes}), JsonLdOptions::default()).map_err(wire)?;
    Ok(schema::schema_sql(options, &o.indexes))
}
