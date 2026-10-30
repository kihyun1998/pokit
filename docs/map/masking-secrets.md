# Masking secrets

## What it is

How text a caller marks as secret is kept out of what pokit emits: the caller registers it with `--secret` or `--secret-env` on `type` or `clipboard write`, the session remembers it, and from then on every exact occurrence is replaced with `***` in each command's printed output (logs included) and in the run record. It covers text only.

## Governing decisions

**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) is adjacent: because the session outlives a command, a secret registered once stays masked for the rest of the session, but the ADR decides the session, not masking.

## Design model

The rules come from reading the source; no maintainer's call or record decides them.

- **A secret is registered by the command that carries it.** `type` and `clipboard write` take the text from stdin with `--secret` (trailing CR/LF removed) or from an environment variable with `--secret-env`, which then never appears on a command line (`secret_text`, main.rs). `Request::secret` gives that text for a `Type` or `ClipboardWrite` request marked secret. `State::handle` adds it to the session's list (`State::secrets`) before the command runs, once per distinct text, and keeps it until the session ends.
- **Masking replaces every exact occurrence of every non-empty secret, in every string and every object key, longest secret first** (`redact::redact`, `MASK`). Longest first, so that of two overlapping secrets the longer is masked whole. Pinned by the unit tests in redact.rs (`a_secret_inside_any_string_becomes_stars`, `nested_arrays_and_objects_are_masked_too`, `object_keys_are_masked`, `an_empty_secret_masks_nothing`, `the_longer_of_two_overlapping_secrets_is_masked_whole`).
- **What is masked** (`State::handle`):
  - every command's printed output, after it is rendered, for as long as the session lasts: a secret read back from the page by `read`, `eval` or `clipboard read` prints as `***`;
  - `logs`' output, because it is a command's output: the session's log buffer holds the raw text and is masked when printed ([Collecting logs](collecting-logs.md));
  - the run record line: its `args.text` is replaced with `***` when the request carries the secret, and the arguments and output are masked as above before they are written ([The run record](the-run-record.md)).
  - Pinned by `a_secret_is_masked_in_output_logs_and_the_run_record` (tests/fixture.rs: the output, `read #password`, `logs` holding `password=***`, and every file the session wrote), by the clipboard check in tests/clipboard.rs (`--secret-env` written, read back as `***`, absent from the record), and on macOS by `input_refuses_what_macos_cannot_take_and_masks_a_secret` (tests/macos.rs).
- **A failed command carrying a secret leaves no screenshot** (`State::handle` skips `failure_screenshot` when `request.secret()` is set), since the field may show it. Pinned by `a_failed_secret_type_leaves_no_screenshot` (tests/fixture.rs).
- **A secret reaches the session only after the session has proved itself** ([Starting the session process](starting-the-session-process.md)); pinned by `a_process_squatting_on_the_session_port_never_receives_the_token_or_a_secret` (tests/fixture.rs).
- **Masking covers text, not pixels or variants.** A truncated or encoded form of a secret is not caught, and neither is the secret in a later screenshot of a visible text field (a `capture`, or the failure screenshot of a later command that does not carry the secret).
- **What is not masked:**
  - the trace and profile files `trace stop` and `profile stop` save ([Tracing the page](tracing-and-profiling.md) holds what they contain);
  - environment given to the app with `launch --env`, which is documented as not masked and not for secrets (main.rs);
  - text typed or written before it was registered as secret: run record lines already written are not rewritten.
- **Size and count.** Nothing bounds the number of secrets a session holds; each distinct secret adds one entry for its life, and every output is scanned once per secret.

## Code

- `crates/pokit/src/redact.rs` — `redact`, `MASK`, `mask`, `walk`
- `crates/pokit/src/session/commands.rs` — `State::handle`
- `crates/pokit/src/request.rs` — `Request::secret`, `CommandKind::screenshots_failure`
- `crates/pokit/src/main.rs` — `secret_text`
- `crates/pokit/src/session/mod.rs` — `State::secrets` (field)

## Reference behaviour

**None.**

## Cross-cutting invariants

**None.**

## Blast radius

- [The run record](the-run-record.md) — the record is masked line by line as it is written; a new field written to the record outside `State::handle` is not masked.
- [Collecting logs](collecting-logs.md) — the buffer holds raw text and relies on being printed only through a command's output; a new way out of the buffer (a file, the launch failure's backend tail) would carry secrets unmasked.
- [Capturing the page](capturing-the-page.md) — masking is text only; a capture of a page showing a secret holds it in pixels.
- [Tracing the page](tracing-and-profiling.md) and [Profiling the page](profiling-the-page.md) — their saved files are not masked; they are safe only while they hold no typed text.
- [Using the clipboard](using-the-clipboard.md) — `clipboard write --secret` registers its text the same way, and `clipboard read` prints it masked.
- [Sending input over CDP](sending-input-over-cdp.md) — `type --secret` sends the text like any other; its result must keep reporting counts and codes, never text.
- [Typing Korean through the IME](typing-korean-through-ime.md) — the composition stages of a Hangul secret are not its exact text and are not masked where a page logs them.
- [The command-line contract](the-command-line-contract.md) — masking runs on the rendered output; a field added after rendering would not be masked.

## Known holes / open

- A truncated, encoded or partly composed form of a secret is not masked.
- Pixels are not masked: a later `capture` or failure screenshot can show a secret in a visible field.
- Nothing bounds the number of secrets a session keeps.
- Text sent without `--secret` before the same text is registered stays in the run record lines already written.
