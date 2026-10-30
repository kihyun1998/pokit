# Typing Korean through the IME

## What it is

How `type` puts Hangul on the page over CDP: as a person with a 2-Set (Dubeolsik) IME would type it, key by key, with the composition the page sees growing and committing, rather than as inserted text. The rest of `type` (Latin keys, inserted text, the delivery check) is [Sending input over CDP](sending-input-over-cdp.md). Only the CDP route composes: `--route os` types without IME composition ([Sending OS input](sending-os-input.md)), and the macOS plugin route does not compose yet ([Sending input on macOS](sending-input-on-macos.md)).

## Governing decisions

**None.** [ADR-0002](../adr/0002-hand-written-cdp-client.md) is adjacent: it decides the client the IME events go through, not how Hangul is composed.

## Design model

The rules come from reading the source and from measurements on the fixture (2026-10-06); no maintainer's call or record decides them.

- **A Hangul character is composed when it is a precomposed syllable, or a lone jamo the IME types as itself** (`hangul::is_composed`). A compound consonant jamo (ㄳ) has no key of its own and is inserted as text, as anything else off the layout is (`type_text`).
- **Each syllable is split into the jamo keys a person presses** (`hangul::keys_for`), and `hangul::Composer` runs them through the 2-Set rules (`Composer::press`). Compound vowels and finals build up over two keys (`COMPOUND_V`, `COMPOUND_T`). A final consonant moves to the next syllable when a vowel follows: typing 가나 shows 간 before it becomes 가 + 나. A compound final splits. Pinned by the unit tests in hangul.rs (`syllables_split_into_the_keys_a_person_presses`, `hangul_is_composed_one_syllable_after_another`, `a_final_consonant_moves_to_the_next_syllable_when_a_vowel_follows`, `a_compound_final_splits_when_a_vowel_follows`, `compound_vowels_build_up_over_two_keys`, `a_consonant_that_cannot_end_a_syllable_starts_the_next`, `every_jamo_key_is_on_the_layout`).
- **A lone jamo in the text (ㅋㅋ) is composed on its own:** whatever is being composed is committed before it and after it, so it never joins its neighbours (`type_text`, `Composer::finish`), though a real IME would join 가ㄴ into 간. Pinned by `lone_jamo_land_as_typed_and_do_not_join_their_neighbours` (tests/ime.rs), with 네ㅋㅋ, 가ㄴ, 한ㅏ, ㄱㅏ, ㅗㅏ, ㄳ, ㅘ요.
- **Any character that is not composed commits the composition first** (`type_text`), so Hangul mixed with other text lands in order (pinned by `text_mixing_hangul_and_ascii_lands_in_order`, tests/ime.rs).
- **For each key pokit sends** (`send_ime_key`):
  - a `Process` key-down (`rawKeyDown`, virtual key 229, with the physical key's `code` from `hangul::key_for`);
  - `Input.insertText` for any syllable the key commits (`commit`);
  - `Input.imeSetComposition` for the composition left, the caret at its end;
  - a key-up carrying the jamo as `key`, with the physical key's virtual key.

  *Derivation, not measured against a real IME:* the key-up's `key` and virtual key.
- **A Hangul key that needs Shift (ㅃ, ㅆ) carries Shift as a flag, not as a key of its own** (`send_ime_key`, `chord::SHIFT`): unlike a chord's modifiers ([Sending input over CDP](sending-input-over-cdp.md)), no Shift key-down or key-up is sent around it.
- **When a CDP call fails mid-composition, the composition is cancelled before the error returns** (`type_text` sends an empty `Input.imeSetComposition`).
- **Each `Process` key-down is counted by the delivery check under its physical `code`** (`Typed::downs`); the committed text, sent with `Input.insertText`, raises no keydown and is not.
- **IME composition needs no focus.** `Input.imeSetComposition` and `Input.insertText` go straight to the renderer's IME handling, not through the OS (see Reference behaviour). Pinned by `composing_needs_no_focus_from_the_user` (tests/ime.rs): the foreground window is the same before and after typing 한글.
- `capabilities` declares `ime_composition: true` for the CDP route of `type` and `false` for the OS and plugin routes (`support::routes`).
- **Size and count.** One key per jamo, each three or four CDP calls; nothing bounds the text's length but the command's `--timeout`.

## Code

- `crates/pokit/src/hangul.rs` — `key_for`, `is_composed`, `is_syllable`, `keys_for`, `Composer`, `Composer::press`, `Composer::finish`, `Step`, `KEYS`, `COMPOUND_V`, `COMPOUND_T`
- `crates/pokit/src/session/commands.rs` — `type_text`, `send_ime_key`, `commit`, `Typed`
- `crates/pokit/src/support.rs` — `routes`

## Reference behaviour

- **Chromium's IME path for CDP:** `Input.imeSetComposition` and `Input.insertText` call `ImeSetComposition` / `ImeCommitText` on the widget input handler (`content/browser/devtools/protocol/input_handler.cc` 1281–1358, Chromium main, read 2026-10-06).
- **What the page saw, measured 2026-10-06 on the fixture.** Typing 한글 gave `compositionstart`, `compositionupdate` ㅎ 하 한, `compositionend` 한, then the same for ㄱ 그 글; a `Process` key-down per key; the value 한글, with the foreground window unchanged. Typing 가나 gave `compositionupdate` ㄱ 가 간, `compositionend` 가, then 나. Blink sends one more `compositionupdate` with the committed text just before each `compositionend`. Pinned by `hangul_is_composed_key_by_key_and_lands_as_typed` and `a_final_consonant_moves_on_as_a_real_ime_moves_it` (tests/ime.rs).
- The keydowns the page gets for 한 over CDP: `Process` with `KeyG`, `KeyK`, `KeyS` (probe 2026-10-12, in [Sending input over CDP](sending-input-over-cdp.md)).

## Cross-cutting invariants

- [Capabilities match behaviour](capabilities-match-behaviour.md) — `support::routes` declares `ime_composition: true` for `type` on the cdp route and `false` on the os and plugin routes; the composition here is what makes the cdp value true.

## Blast radius

- [Sending input over CDP](sending-input-over-cdp.md) — the delivery check counts each `Process` key-down by its physical `code`; sending a different `code` or skipping the key-down makes a composed `type` fail as `not_delivered`.
- [Sending input on macOS](sending-input-on-macos.md) — composing on macOS (#37) is to give the page the same composition events; the events measured here are what it is compared with.
- [Masking secrets](masking-secrets.md) — masking replaces a secret's exact text only; the stages of a Hangul secret's composition (ㅎ, 하, 한) are not that text, so a page that logs its `compositionupdate` events puts them in `logs` unmasked.

## Known holes / open

- The key-up's `key` and virtual key are derived, not measured against a real IME. An OS-input comparison in #5 can check them.
- What a real IME sends around a Shifted jamo key (ㅃ, ㅆ) has not been measured.
- A lone jamo never joins its neighbours, while a real IME would join 가ㄴ into 간.
- Composing on macOS through the text input client is not built. Tracked: #37.
