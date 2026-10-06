# Using the user's clipboard

How `clipboard read` / `clipboard write` reach the Windows clipboard, and how the session gives the user's clipboard back. Code: `clipboard::Clipboard` (`start`, `read_text`, `write_text`, `restore`), `clipboard::restorable`, `State::clipboard_write_cmd`, `State::restore_clipboard` (session/clipboard.rs).

## Design model

- **pokit's writes are owned by a message-only window on a thread of its own, which keeps pumping messages.** A clipboard opened with no window cannot be written: `EmptyClipboard` then sets the owner to NULL and `SetClipboardData` fails (`OpenClipboard`, `EmptyClipboard` and `SetClipboardData` remarks, sdk-api docs, read 2026-10-06). The window's thread must pump, because the system sends messages to the clipboard owner, and another program emptying the clipboard waits on them. Every clipboard operation runs on that thread, posted to it with `WM_APP`.
- **Saving, writing and giving back are decided on that thread, one job at a time.** The saved clipboard and the sequence number of pokit's last write live there, not in the session. So two `clipboard write` commands that arrive together cannot both save, and a command that gives up waiting (10 s) does not lose what its job went on to do. A write that fails after emptying the clipboard still counts as pokit's, so the saved clipboard comes back at the end; a format that cannot be put back is skipped and the rest are restored.
- **The user's clipboard is saved before the session's first write and given back when the session ends**, by `close` before it answers and by every shutdown path (idle timeout, the app exiting), in both launch and attach modes. **Maintainer's call (2026-10-02, #1):** it is given back only if the clipboard still holds what pokit last wrote, and never after each command, because a test writes in one command and pastes in a later one. Prior art shown: ego-browser's `keyboard.paste`, which restores the clipboard after pasting on macOS.
- **"Still holds what pokit wrote" is the clipboard sequence number.** It is recorded right after each write and compared at the end. The system bumps it whenever the contents change or the clipboard is emptied (`GetClipboardSequenceNumber` remarks).
- **What is saved is every format whose data is plain memory.** Bitmaps, metafiles, palettes, owner-display formats, and the private and GDI-object ranges are handles of other kinds and are left out (`clipboard::restorable`). An image usually survives anyway, as `CF_DIB`. Reading a format that its owner renders on demand makes that owner render it.
- **What pokit puts on the clipboard, its writes and the restore alike, is kept out of clipboard history and cloud sync.** Each write adds the registered format `ExcludeClipboardContentFromMonitorProcessing` (Clipboard Formats, "Cloud Clipboard and Clipboard History Formats", win32 docs, read 2026-10-06). The user's Win+V list therefore gets neither test text nor a duplicate of what was restored.
- **`clipboard read` withholds what another program marked as not for monitoring.** When the clipboard carries `ExcludeClipboardContentFromMonitorProcessing` and pokit did not write it, `read` returns `text: null` and a `withheld` reason instead of the text. That is the mark password managers put on a copied password. **Maintainer's call (2026-10-06, #5):** the alternatives shown were reading it as it is, and leaving it to an issue. pokit's own writes carry the same mark and are always read; "pokit's own" is the sequence number of its last write. The older `Clipboard Viewer Ignore` mark is not checked; the call covered only this format.
- **Written text can be secret.** `clipboard write --secret` / `--secret-env` mask the text like `type --secret` does, including when `clipboard read` prints it back.

## Measured

- **A CDP `Ctrl+KeyV` pastes.** `key Ctrl+KeyV --into #clip` put the clipboard's text into the fixture's field without the app in front (2026-10-06), so pasting needs no OS input.
- **The restore is exact for text.** Across a session that wrote twice, the clipboard afterwards matched what it held before byte for byte, read by PowerShell's `Get-Clipboard -Raw` (2026-10-06).

## Testing

- **Every clipboard case runs in one test, in order** (`tests/clipboard.rs`), because there is one clipboard per desktop and tests run in parallel. The test sets a known value first, plays "another program" with PowerShell's `Set-Clipboard`, and gives the developer's clipboard text back when it ends. `Set-Clipboard` does land in the developer's clipboard history.
