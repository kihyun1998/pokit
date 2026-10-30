# Finding elements on the page

## What it is

How a command that takes an element finds it, on Windows over CDP: `snapshot` writes the page's accessibility tree as text with a ref beside each element an agent can act on; a ref or a CSS selector is resolved to a remote object on the page; the command's remote objects are released after it; and `read` reports an element's tag, text, value and state. Through the macOS plugin there is no snapshot and no remote object: `read` finds and reads in one evaluation ([Reaching the page through the plugin](reaching-the-page-through-the-plugin.md)), and snapshot is [Seeing the page on macOS](seeing-the-page-on-macos.md).

## Governing decisions

- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): a snapshot's refs are kept alive into the next command because the session holds them; a command that connected on its own could not.

## Design model

The rules come from reading the source, the GLOSSARY's **Snapshot** and **Ref**, and the findings of #103.

- **A snapshot is the current page's full accessibility tree, rendered as an outline** (`State::snapshot_cmd`, `snapshot::render`). Each line is a role, its name, a ref where the element can be acted on, its value, and the states `focused`, `disabled`, `checked`, `expanded`, `selected` (`STATES`). Ignored nodes, inline text boxes, text repeating its parent's name and nameless containers (`TRANSPARENT`) are left out and their children take their place. Pinned by `the_fixture_page_renders_as_its_outline_with_refs_on_what_can_be_acted_on` (snapshot.rs unit test, on a recorded tree `tests/data/ax-fixture-main.json`).
- **A ref goes on every element whose role is one an agent acts on (`ACTIONABLE`) or that reports itself focusable,** and that has a backend DOM node; never on the document. The ref maps to that backend node id.
- **A ref names one element of the most recent snapshot, and only of that one.** Each snapshot replaces the session's ref map (`State::refs`), and numbering continues from the last snapshot (the `next` field of `Refs`), so no ref is reused and an old ref is never silently taken for a new element (pinned by `a_later_snapshot_continues_the_numbering_so_no_ref_is_reused`, snapshot.rs). A ref not in the latest snapshot fails with `stale_ref`, naming the ref, before anything is sent (pinned by `a_ref_from_an_earlier_snapshot_is_refused_by_name`, tests/fixture.rs). A ref is anything of the form `e` followed by digits (`snapshot::is_ref`); anything else is a CSS selector.
- **A ref resolves on the page its snapshot was taken on,** which may no longer be the current page (the `map` field of `Refs` holds the target id with the node). A selector resolves on the current page.
- **`snapshot` turns the accessibility domain on and off around the tree.** `getFullAXTree` alone leaves Blink keeping a full accessibility tree up to date for the rest of the session (`inspector_accessibility_agent.cc` 242, 687–693). Its `disable` only takes effect after an `enable`. Re-verified: `State::snapshot_cmd` sends `Accessibility.enable`, `Accessibility.getFullAXTree`, then `Accessibility.disable` whether the tree call succeeded or not.
- **An element becomes a remote object in an object group of the command's own** (`pokit-<n>`, `Resolved`). A selector is `document.querySelector` through `Runtime.evaluate`, and a ref is `DOM.resolveNode` on its backend node, both with that group (`object_group`, `State::resolve`). A selector that throws fails as an `error` naming it (`invalid_selector`); one that matches nothing is `not_found` (`no_match`). A ref whose node is gone is `stale_ref` ("no longer in the page").
- **After the command, the group is released on the targets it resolved on, and only there** (`State::release_objects`). A command that resolved nothing sends nothing.
  - The command's group and targets live in a tokio task-local (`RESOLVED`) set around the command in `State::handle`. Every resolve runs in that task. Outside a command, `object_group` gives the shared group `pokit` (`OBJECT_GROUP`; pinned by `outside_a_command_the_group_is_the_shared_one`).
  - Until #103 one group, `pokit`, was released on every target the session knew, each awaited up to the CDP call's 15 s. A page stuck in a script then held up every element command by 15 s, even one for another page. Two commands at once on one page could also drop each other's objects, since a group is per page and not per command. Measured: with the fixture's second page stuck for 40 s, `read #name` on the main page took 15.4 s and 16.3 s before #103 (202 ms otherwise; `eval`, which resolves nothing, 568 ms); after it, under 2 s (2026-10-22).
  - #102 had kept `capture --window` out of that release by hand. It resolves no element, so it now releases nothing, by construction ([Capturing a window](capturing-a-window.md)).
  - Pinned by `an_element_command_does_not_wait_on_another_page_that_does_not_answer` (tests/fixture.rs), which stalls the second page and times a `read` on the main page; by `the_objects_an_element_command_resolved_are_released_on_its_page` (tests/fixture.rs), which makes an element held only by the page and a `WeakRef`, reads it with pokit, removes it, forces a collection over CDP (`HeapProfiler.collectGarbage`, through the harness's `Pokit::cdp_call`), and expects the `WeakRef` empty — the element is made inside a function, because a top-level `const` in `eval` stays a global binding and keeps it alive whatever pokit does; and by `object_groups::two_commands_at_once_have_groups_and_targets_of_their_own` (commands.rs unit test), which runs two commands' scopes at once. Reddened 2026-10-22: releasing nothing, and noting no target, turned the WeakRef test red; releasing on every target turned the timing test red (46 s run); one group for every command turned the unit test red.
- **A target the command did resolve on and that is stuck** has already held the command up when it resolved. The release adds no time limit of its own.
- **`read` reports an element's `tag`, trimmed `text` (`innerText`, else `textContent`), `value` (where the element has one), and `state`: `focused`, `disabled`, `checked` (where it has one), `visible` (a non-zero rectangle)** (`READ_ELEMENT`, `State::read_cmd`). Over CDP it is called on the resolved object; through the plugin the same function runs inside one evaluation that also finds the element, and a ref is refused as `unsupported` (`refs_unsupported`, #14). What `read` prints passes through masking, so a secret typed into a field reads back as `***` ([Masking secrets](masking-secrets.md)).
- **Size and count.** Nothing bounds a snapshot's size: the whole tree comes back in one CDP reply and is rendered whole; the ref map holds one entry per actionable element of the latest snapshot. The selector text is the caller's.

## Code

- `crates/pokit/src/snapshot.rs` — `Snapshot`, `render`, `is_ref`, `ACTIONABLE`, `TRANSPARENT`, `STATES`
- `crates/pokit/src/session/commands.rs` — `State::snapshot_cmd`, `State::resolve`, `State::read_cmd`, `READ_ELEMENT`, `object_group`, `Resolved`, `RESOLVED`, `OBJECT_GROUP`, `State::release_objects`, `State::handle`, `invalid_selector`, `no_match`, `refs_unsupported`
- `crates/pokit/src/session/mod.rs` — `Refs`

## Reference behaviour

- Blink, `inspector_accessibility_agent.cc` 242, 687–693: `getFullAXTree` without `enable` keeps the tree updated afterwards, and `disable` acts only after `enable` (version and read date not recorded in the old note).

## Cross-cutting invariants

**None.**

## Blast radius

- [Sending input over CDP](sending-input-over-cdp.md) — `click --x/--y` aside, every element input resolves through `State::resolve` and runs inside the command's object group; a group released too early loses the element mid-command.
- [Capturing the page](capturing-the-page.md) — an element capture resolves the same way, and relies on resolving no element meaning no release.
- [Capturing a window](capturing-a-window.md) — it relies on a command that resolves nothing waiting on no other page afterwards.
- [The app's pages](the-apps-pages.md) — a ref holds the id of the page it was taken on and a selector goes to the current page; changing how targets are identified or which one is current changes where an element is looked for.
- [Seeing the page on macOS](seeing-the-page-on-macos.md) — the snapshot format and the ref rules here are what the macOS snapshot is to match.
- [Masking secrets](masking-secrets.md) — `read`'s text and value are where a typed secret comes back from the page.

## Known holes / open

- No test runs two real element commands on one page at once: the window in which one could drop the other's objects is too short to hit on purpose. The unit test covers the separation instead.
- Nothing bounds a snapshot's size, and a page with a very large tree could exceed the CDP message limit in [Speaking CDP](speaking-cdp.md) (not measured).
- Whether `Accessibility.getFullAXTree` without a frame id includes the contents of an `<iframe>` has not been checked; refs inside one are untested.
- A target whose connection closed and was reopened keeps its id and its refs; whether a backend node id still resolves over the new connection has not been checked.
- Snapshot and refs on macOS are not built. Tracked: #14.
