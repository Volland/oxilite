## Decisions

Recorded in the `oxilite-studio` repository's `lat.md/decisions.md` (S1–S13) and in this
repository's `lat.md/architecture.md` under "Studio server". Notable here: reads run on worker
threads; validation carries a generation and stops or is dropped when superseded; D1 is a
blocking backend behind a store handle, so every request works on it unchanged.
