# pokit

## Agent skills

Issues live in GitHub Issues, through `gh`.

- Before creating, reading, labelling or closing an issue or PR, read `docs/agents/issue-tracker.md`.
- Before applying or removing a triage label, read `docs/agents/triage-labels.md`.
- Before naming a domain concept or working in an area an ADR covers, read `docs/agents/domain.md`.

## Comments

A comment says what the code is. Why it is this way, what it deliberately leaves out, the trap and the measured value go to a territory note under `docs/map/`. Before writing a new note there, read its README. History goes to the commit message. Comments written before this rule still carry the rest: move what the map does not yet hold into it (`decant`) before deleting one.

## Map

The territory map's hub is [`docs/map/README.md`](docs/map/README.md). Before designing a change, open the territory it lands in and walk its blast radius and cross-cutting invariants; after it lands, bring the notes it falsified back in line. When a fix reveals a fact, ask whether it holds at every site that shares its assumption; if it does, the fix lands with an invariant note.

## Layout

The workspace `members` in `Cargo.toml` and each member's `description` say what each crate is.
