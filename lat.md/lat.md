This directory defines the high-level concepts, business logic, and architecture of this project using markdown. It is managed by [lat.md](https://www.npmjs.com/package/lat.md) — a tool that anchors source code to these definitions. Install the `lat` command with `npm i -g lat.md` and run `lat --help`.

- [[architecture]] — how oxilite works: crates, sans-IO core, encoding, schema, compiler, planner, backends, and the planned Cypher frontend.
- [[decisions]] — the architecture decisions and why they were made.
- [[milestones]] — delivery plan and done-criteria (mirrors `openspec/changes/`).
- [[tests]] — implemented test specifications, each referenced from test code.
- [[test-plan]] — planned test specifications per milestone.
