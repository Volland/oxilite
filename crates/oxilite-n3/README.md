# oxilite-n3

Notation3 rules over [oxilite](https://github.com/Volland/oxilite), run by the
[eyeron](https://crates.io/crates/eyeron) reasoner: ask what follows, or store it as inferences.
Works on every oxilite backend, Cloudflare D1 included.

```rust
use oxilite::store::Store;

let store = Store::new()?;
store.update(
    "PREFIX ex: <http://example.org/>
     INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
)?;

let rules = "@prefix ex: <http://example.org/> .
             { ?x ex:parent ?y . ?y ex:parent ?z } => { ?x ex:grandparent ?z } .";

let r = store.n3_reason(rules)?;            // what follows, nothing written
assert_eq!(r.derived.len(), 1);

let stats = store.n3_materialize(rules)?;   // stored as inferences of the producer "n3"
assert_eq!(stats.inferred, 1);
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

Enable it with `oxilite = { version = "0.9", features = ["n3"] }`.

## What a run does

1. **Reads one scope**: every graph but the system graphs (or `Options::graphs`), merged, and only
   the predicates the rule bodies use. A variable predicate, or `prune: false`, reads the whole
   scope. With `include_inferred` it adds other producers' inferences, never its own.
2. **Reasons in memory** with eyeron: forward and backward rules, quoted formulas, `log:`, `math:`,
   `string:`, `list:` and `time:` builtins, `log:query`, and fuses (`{ … } => false`).
3. **Returns or stores**: `n3_reason` returns the conclusions; `n3_materialize` replaces the
   producer's inferences in `quads_inf` with them in one atomic request. SPARQL, Cypher and Datalog
   see them with `include_inferred`.

| `Options` | Default | |
|---|---|---|
| `graphs` | every graph but the system graphs | the scope |
| `include_inferred` | `false` | also read other producers' inferences |
| `prune` | `true` | load only the predicates rule bodies read |
| `producer` | `"n3"` | the name conclusions are stored under |
| `now` | the clock | milliseconds since the epoch that `time:localTime` reports |
| `proof` | `false` | return eyeron's proof |
| `require_complete` | `false` (`true` for materialization) | fail when a limit cut the run short |
| `limits` | eyeron's | fixpoint steps, matcher steps, backward depth, term size |

A conclusion the store cannot hold (a literal or list subject, a list object) is returned as N3
text under `unstorable` and never stored. A fired fuse is reported by `n3_reason` and aborts
`n3_materialize`.

From the command line: `oxilite n3 -l data.sqlite -f rules.n3` (add `--materialize`).
