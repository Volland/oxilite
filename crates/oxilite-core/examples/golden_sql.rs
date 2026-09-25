//! Writes the golden SQL hashes of an unversioned store (see `tests/golden/dump.rs`):
//! `cargo run -p oxilite-core --example golden_sql -- <rdf-tests dir> > tests/golden/off-sql.tsv`.

#[path = "../tests/golden/dump.rs"]
mod dump;

fn main() {
    let root = std::env::args().nth(1).expect("the rdf-tests directory");
    for line in dump::dump(std::path::Path::new(&root)) {
        println!("{line}");
    }
}
