//! Cypher at a past version of a versioned store (`CypherOptions::query.as_of`).

use futures::executor::block_on;
use oxilite::cypher::{CypherError, CypherOptions, Params};
use oxilite::rusqlite::RusqliteBackend;
use oxilite::store::Store;
use oxilite::version::Versioning;
use oxilite::{AsyncBackend, AsyncStore, Capabilities, QueryOptions, StoreOptions};
use oxilite_core::{Request, Response, SyncBackend};

struct D1Like(RusqliteBackend, Capabilities);

impl AsyncBackend for D1Like {
    async fn execute(&self, request: &Request) -> oxilite_core::Result<Response> {
        self.0.execute(request)
    }
    fn capabilities(&self) -> &Capabilities {
        &self.1
    }
}

fn at(version: &str) -> CypherOptions {
    CypherOptions {
        query: QueryOptions {
            as_of: Some(version.into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn statuses(r: &oxilite::cypher::CypherResult) -> Vec<String> {
    let mut v: Vec<String> = r.rows.iter().map(|row| format!("{:?}", row[0])).collect();
    v.sort();
    v
}

const CREATE: &str =
    "CREATE (:Ticket {name: 't1', status: 'open'}), (:Ticket {name: 't2', status: 'open'})";
const CLOSE: &str = "MATCH (t:Ticket {name: 't1'}) SET t.status = 'done'";
const READ: &str = "MATCH (t:Ticket) RETURN t.status ORDER BY t.status";
const NEIGHBOURS: &str = "MATCH (t:Ticket {name: 't1'}) RETURN t";

// @lat: [[tests#Versioning#Cypher reads a past version]]
#[test]
fn cypher_reads_a_past_version() {
    let options = StoreOptions {
        versioning: Versioning::Log,
        ..Default::default()
    };
    let native =
        Store::with_backend_and_options(RusqliteBackend::memory().unwrap(), &options).unwrap();
    native.cypher(CREATE).unwrap();
    native.cypher(CLOSE).unwrap();
    let now = statuses(&native.cypher(READ).unwrap());
    let before = statuses(
        &native
            .cypher_with(READ, &Params::new(), &at("HEAD~1"))
            .unwrap(),
    );
    assert_ne!(now, before);
    assert!(before.iter().all(|s| s.contains("open")), "{before:?}");
    // Node materialization (direct quad reads) sees the version too.
    let node = native
        .cypher_with(NEIGHBOURS, &Params::new(), &at("HEAD~1"))
        .unwrap();
    assert!(format!("{:?}", node.rows).contains("open"));
    // Writes apply to the current state.
    let err = native
        .cypher_with(CLOSE, &Params::new(), &at("HEAD~1"))
        .unwrap_err();
    assert!(matches!(err, CypherError::Unsupported(_)), "{err}");

    // The D1 code path.
    let d1 = block_on(AsyncStore::open_with_options(
        D1Like(RusqliteBackend::memory().unwrap(), Capabilities::d1()),
        &options,
    ))
    .unwrap();
    block_on(d1.cypher(CREATE)).unwrap();
    block_on(d1.cypher(CLOSE)).unwrap();
    let before_d1 =
        statuses(&block_on(d1.cypher_with(READ, &Params::new(), &at("HEAD~1"))).unwrap());
    assert_eq!(before_d1, before);
}
