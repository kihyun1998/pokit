# Using the user's clipboard

## What it is
How `clipboard read` / `clipboard write` reach the Windows clipboard, and how the session gives the user's clipboard back when it ends. pokit shares one clipboard with the user, so it saves what the user had before its first write, keeps its own writes out of clipboard history, withholds what a password manager marked, and puts the user's clipboard back unless something else has written to it since. On macOS `clipboard` is not built (Tracked: #36).

## Governing decisions
- [ADR-0001](../adr/0001-commands-talk-to-a-session.md): the session outlives each command, which is what lets the user's clipboard be saved at the first write and given back at the session's end rather than after each command. The choice to give it back only at the end is a maintainer's call on #1 (Design model).

## Design model
The rules come from maintainer calls on #1 and #5, from the Win32 documentation read on 2026-10-06, and otherwise from the source. Size: the text `clipboard write` puts on the clipboard arrives in a request, which the session's request limit caps ([The command-line contract](the-command-line-contract.md)). Nothing bounds what `clipboard read` returns or what is saved of the user's clipboard: `snapshot` copies every restorable format whole into memory (see Known holes).

**The owner thread**
- **pokit's writes are owned by a message-only window on a thread of its own, which keeps pumping messages** (`Clipboard::start`, a `STATIC` window with parent `HWND_MESSAGE`). A clipboard opened with no window cannot be written: `EmptyClipboard` then sets the owner to NULL and `SetClipboardData` fails (`OpenClipboard`, `EmptyClipboard` and `SetClipboardData` remarks, sdk-api docs, read 2026-10-06). The window's thread must pump, because the system sends messages to the clipboard owner, and another program emptying the clipboard waits on them. Every clipboard operation runs on that thread, posted to it with `WM_APP` (`Clipboard::run`). The window is started at the session's first clipboard command (`State::clipboard_owner`).
- **Saving, writing and giving back are decided on that thread, one job at a time.** The saved clipboard and the sequence number of pokit's last write live there (`Saved`), not in the session. So two `clipboard write` commands that arrive together cannot both save, and a command that gives up waiting (10 s, `Clipboard::run`) does not lose what its job went on to do. A write that fails after emptying the clipboard still counts as pokit's (`put` reports that it touched the clipboard; `Clipboard::write_text`), so the saved clipboard comes back at the end; a format that cannot be put back is skipped and the rest are restored (`put`), and the failure is logged as a session warning (`State::restore_clipboard`).
- **Opening the clipboard waits up to 1 s while another window holds it** (`Open::new`: 100 tries, 10 ms apart), then fails with "another program kept the clipboard open for 1s".

**Saving and giving back**
- **The user's clipboard is saved before the session's first write and given back when the session ends**, by `close` before it answers (`Request::Close`, session/commands.rs) and by every shutdown path (idle timeout, the app exiting; `State::shutdown`), in both launch and attach modes. `clipboard write` reports `saved_user_clipboard`: true only on the write that saved. **Maintainer's call (2026-10-02, #1):** it is given back only if the clipboard still holds what pokit last wrote, and never after each command, because a test writes in one command and pastes in a later one. Prior art shown: ego-browser's `keyboard.paste`, which restores the clipboard after pasting on macOS. A clipboard not given back is logged as a session note (`State::restore_clipboard`).
- **"Still holds what pokit wrote" is the clipboard sequence number.** It is recorded right after each write (`Saved::written`) and compared at the end (`Clipboard::restore`). The system bumps it whenever the contents change or the clipboard is emptied (`GetClipboardSequenceNumber` remarks, read 2026-10-06).
- **What is saved is every format whose data is plain memory.** Bitmaps, metafiles, palettes, owner-display formats, and the private and GDI-object ranges are handles of other kinds and are left out (`clipboard::restorable`: 2, 3, 9, 14, 0x80–0x8F, 0x200–0x3FF; `handles_that_are_not_plain_memory_are_not_restored`). An image usually survives anyway, as `CF_DIB`. Reading a format that its owner renders on demand makes that owner render it (`snapshot` reads every format).
- **What pokit puts on the clipboard, its writes and the restore alike, is kept out of clipboard history and cloud sync.** Each `put` adds the registered format `ExcludeClipboardContentFromMonitorProcessing` (Clipboard Formats, "Cloud Clipboard and Clipboard History Formats", win32 docs, read 2026-10-06). The user's Win+V list therefore gets neither test text nor a duplicate of what was restored.

**Reading**
- **`clipboard read` withholds what another program marked as not for monitoring.** When the clipboard carries `ExcludeClipboardContentFromMonitorProcessing` and pokit did not write it, `read` returns `text: null` and a `withheld` reason instead of the text (`Clipboard::read_text`, `State::clipboard_read_cmd`). That is the mark password managers put on a copied password. **Maintainer's call (2026-10-06, #5):** the alternatives shown were reading it as it is, and leaving it to an issue. pokit's own writes carry the same mark and are always read; "pokit's own" is the sequence number of its last write. The older `Clipboard Viewer Ignore` mark is not checked; the call covered only this format.
- Text is read and written as `CF_UNICODETEXT`, UTF-16 up to the first NUL (`clipboard::utf16_bytes`, `clipboard::text_from_utf16`; `text_survives_the_round_trip_through_utf16_and_stops_at_nul`).
- **Written text can be secret.** `clipboard write --secret` / `--secret-env` mask the text like `type --secret` does (`Request::secret`, request.rs), including when `clipboard read` prints it back ([Masking secrets](masking-secrets.md)).

**Testing**
- **Every clipboard case runs in one test, in order** (`the_clipboard_is_written_pasted_kept_out_of_history_and_given_back`, tests/clipboard.rs), because there is one clipboard per desktop and tests run in parallel. The test sets a known value first, plays "another program" with PowerShell's `Set-Clipboard`, and gives the developer's clipboard text back when it ends (`KeepClipboard`). It covers the withheld read (and that the withheld text is not in the run record), the write and its history mark, a CDP paste, the give-back at `close`, a secret written and read back masked, and no give-back after someone else wrote. `Set-Clipboard` does land in the developer's clipboard history.

## Code
- crates/pokit/src/clipboard.rs — Read, Item, restorable, utf16_bytes, text_from_utf16, Clipboard, Clipboard::start, Clipboard::run, Clipboard::read_text, Clipboard::write_text, Clipboard::restore, Saved, snapshot, put, exclude_format, sequence, Open, Open::new, read_format, write_format
- crates/pokit/src/session/clipboard.rs — ClipboardState, State::clipboard_read_cmd, State::clipboard_write_cmd, State::restore_clipboard, State::clipboard_owner, blocking
- crates/pokit/src/session/mod.rs — State::shutdown (gives the clipboard back)
- crates/pokit/tests/clipboard.rs — the_clipboard_is_written_pasted_kept_out_of_history_and_given_back

## Reference behaviour
- **Win32 clipboard rules** (sdk-api and win32 docs, read 2026-10-06): a write needs an owner window; the owner receives messages others wait on; `GetClipboardSequenceNumber` changes on every change or empty; `ExcludeClipboardContentFromMonitorProcessing` keeps data out of history and cloud sync. As cited above.
- **A CDP `Ctrl+KeyV` pastes.** `key Ctrl+KeyV --into #clip` put the clipboard's text into the fixture's field without the app in front (2026-10-06), so pasting needs no OS input. The clipboard test repeats it.
- **The restore is exact for text.** Across a session that wrote twice, the clipboard afterwards matched what it held before byte for byte, read by PowerShell's `Get-Clipboard -Raw` (2026-10-06).
- **Prior art:** ego-browser's `keyboard.paste` restores the clipboard after each paste on macOS (shown on #1, 2026-10-02).

## Cross-cutting invariants
**None.**

## Blast radius
- [Starting the session process](starting-the-session-process.md) — the user's clipboard comes back only if every way the session ends calls `State::restore_clipboard`; a new exit path that skips it leaves test text on the user's clipboard.
- [Masking secrets](masking-secrets.md) — `clipboard read` prints whatever the clipboard holds; a secret written with `--secret` stays hidden only while the session masks that text in every output and in the run record.
- [Sending input over CDP](sending-input-over-cdp.md) — tests and agents paste with a CDP `Ctrl+KeyV`, which assumes the page's paste reads the system clipboard without the app in front.
- [Sending OS input](sending-os-input.md) — an OS paste or copy changes the clipboard's sequence number like any other program, so after it pokit no longer gives the user's clipboard back.
- [The run record](the-run-record.md) — the withheld text must never reach it; `clipboard read` returning the text would record it.

## Known holes / open
- **macOS: clipboard read, write and give-back are not built.** Tracked: #36.
- **Nothing bounds what is saved or read.** The user's clipboard is copied whole, every restorable format, into memory at the first write, and `clipboard read` returns the whole text; a very large clipboard is unmeasured.
- **The older `Clipboard Viewer Ignore` mark is not checked** by `clipboard read`; the call on #5 covered only `ExcludeClipboardContentFromMonitorProcessing`.
- **Formats outside plain memory are not given back**; an image survives only if its owner also offered `CF_DIB`.
