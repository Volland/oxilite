//! A SPARQL endpoint on Cloudflare D1, in Rust.
//!
//! `GET /sparql?query=…` or `POST /sparql` (body: the query) → SPARQL JSON results;
//! `POST /update` (body: a SPARQL update) → 204; `POST /load?format=text/turtle` → 204.
use oxilite::d1::D1Backend;
use oxilite::AsyncStore;
use worker::*;

fn fail(e: impl std::fmt::Display) -> Result<Response> {
    Response::error(e.to_string(), 400)
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    // The schema is applied by `wrangler d1 migrations apply` (see migrations/).
    let store = match AsyncStore::open_existing(D1Backend::new(env.d1("DB")?)).await {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let url = req.url()?;
    match (req.method(), url.path()) {
        (Method::Get, "/sparql") | (Method::Post, "/sparql") => {
            let query = if req.method() == Method::Get {
                url.query_pairs().find(|(k, _)| k == "query").map(|(_, v)| v.into_owned()).unwrap_or_default()
            } else {
                req.text().await?
            };
            match store.query_output(query.as_str(), &Default::default()).await {
                Ok(out) => match oxilite_core::json::output_to_sparql_json(&out) {
                    Ok(json) => {
                        let mut r = Response::ok(json)?;
                        r.headers_mut().set("content-type", "application/sparql-results+json")?;
                        Ok(r)
                    }
                    Err(e) => fail(e),
                },
                Err(e) => fail(e),
            }
        }
        (Method::Post, "/update") => match store.update(req.text().await?.as_str()).await {
            Ok(()) => Response::empty().map(|r| r.with_status(204)),
            Err(e) => fail(e),
        },
        (Method::Post, "/load") => {
            let format = url.query_pairs().find(|(k, _)| k == "format").map(|(_, v)| v.into_owned()).unwrap_or_else(|| "text/turtle".into());
            let Some(format) = oxilite::io::RdfFormat::from_media_type(&format) else {
                return fail(format!("unknown format {format}"));
            };
            match store.bulk_load(format, req.bytes().await?.as_slice()).await {
                Ok(()) => Response::empty().map(|r| r.with_status(204)),
                Err(e) => fail(e),
            }
        }
        (Method::Get, "/explain") => {
            let query = url.query_pairs().find(|(k, _)| k == "query").map(|(_, v)| v.into_owned()).unwrap_or_default();
            match store.explain(query.as_str()) {
                Ok(s) => Response::ok(s),
                Err(e) => fail(e),
            }
        }
        _ => Response::error("not found", 404),
    }
}
