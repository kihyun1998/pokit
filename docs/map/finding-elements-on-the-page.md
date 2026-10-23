# Finding elements on the page

How a command that takes an element (a selector or a snapshot ref) turns it into a remote object on the page, and how those objects are released, on Windows over CDP. Through the macOS plugin there is no remote object: `read` finds and reads in one evaluation ([[reaching-the-page-through-the-plugin]]). Code: `State::resolve`, `object_group`, `Resolved`, `RESOLVED`, `State::release_objects`, `State::handle` (session/commands.rs).

## Design model

- **An element becomes a remote object in an object group of the command's own** (`pokit-<n>`, `Resolved`). A selector is `document.querySelector` through `Runtime.evaluate`, and a ref is `DOM.resolveNode` on its backend node, both with that group (`object_group`).
- **After the command, the group is released on the targets it resolved on, and only there** (`release_objects`). A command that resolved nothing sends nothing.
  - The command's group and targets live in a tokio task-local (`RESOLVED`) set around the command in `State::handle`. Every resolve runs in that task.
  - Until #103 one group, `pokit`, was released on every target the session knew, each awaited up to the CDP call's 15 s. A page stuck in a script then held up every element command by 15 s, even one for another page. Two commands at once on one page could also drop each other's objects, since a group is per page and not per command.
  - #102 had kept `capture --window` out of that release by hand. It resolves no element, so it now releases nothing, by construction.
- **A target the command did resolve on and that is stuck** has already held the command up when it resolved. The release adds no time limit of its own.

## Measured

- **With the fixture's second page stuck for 40 s, `read #name` on the main page took 15.4 s and 16.3 s before #103** (202 ms otherwise; `eval`, which resolves nothing, 568 ms). After it, under 2 s (2026-10-22).

## Testing

- `an_element_command_does_not_wait_on_another_page_that_does_not_answer` (tests/fixture.rs) stalls the second page and times a `read` on the main page.
- `the_objects_an_element_command_resolved_are_released_on_its_page` (tests/fixture.rs) makes an element held only by the page and a `WeakRef`, reads it with pokit, removes it, forces a collection over CDP (`HeapProfiler.collectGarbage`, through `Pokit::cdp_call`, which talks to the debugging port directly), and expects the `WeakRef` empty. The element is made inside a function: a top-level `const` in `eval` stays a global binding and keeps it alive whatever pokit does.
- `object_groups::two_commands_at_once_have_groups_and_targets_of_their_own` (unit) runs two commands' scopes at once.
- Reddened 2026-10-22: releasing nothing, and noting no target, turned the WeakRef test red; releasing on every target turned the timing test red (46 s run); one group for every command turned the unit test red.
- No test runs two real element commands on one page at once: the window in which one could drop the other's objects is too short to hit on purpose. The unit test covers the separation instead.
- `Pokit::cdp_call` reads the `/json/list` reply up to its `Content-Length`: the debugging port keeps the connection open despite `Connection: close`, and reading to the end hung. It connects to `127.0.0.1` rather than the `webSocketDebuggerUrl`'s host, which was refused.
