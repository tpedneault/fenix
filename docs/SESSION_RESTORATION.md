# Session restoration

Fenix saves `session.json` beside `config.ini` and restores it at the next launch.
The checkpoint includes named documents, unsaved scratch buffers, workspace names,
split directions and ratios, focused windows/workspaces/panes, and per-pane cursors
and scroll positions. Documents shared by several panes remain shared after restart.
Files passed on the command line take focus while restored work stays available.

Session checkpoints run during the two-second maintenance sweep and on exit.
Writes use the atomic storage layer; unchanged JSON is not rewritten. Unsaved text
is stored in the session file, not written to the original document. This file and
recovery snapshots contain document contents in plain text in your config directory.

## Commands and quitting

- `:session-save` writes a checkpoint immediately.
- `:session-quit` writes a checkpoint and exits, preserving unsaved edits for the
  next launch. It stays open if the checkpoint cannot be written.
- Ordinary quit commands retain their unsaved-change confirmation.
- Force quit or confirming discard removes unsaved contents from the checkpoint
  and clears their recovery snapshots. If a valid checkpoint cannot be updated,
  the editor stays open and reports the failure rather than resurrecting those
  edits next time. An unreadable session file is retained for manual recovery.

Closing a document removes it from subsequent checkpoints. Undo history, selections,
marks, transient prompts, refactor transactions, and running processes are not
restored. Dashboards are regenerated with their current projects and recent-file
actions. Terminal, task, debugger, PDF, and integration panes return to a dashboard;
open the desired integration again. Normal language-service startup still applies
when editing restored files.

## Disk changes and recovery

Clean documents reload the current file contents. Dirty documents return as dirty
buffers. If their recorded disk fingerprint changed, or they already had a disk
conflict, the conflict indicator and normal save guard remain active. Use the
existing `:w!` or `:e!` workflow after reviewing the differences. Fingerprints use
file modification time and size; same-size changes with unchanged timestamps are
an existing detection limitation.

Missing or unreadable clean documents are skipped with a warning. Unsaved contents
from an unreadable named document become an unnamed buffer so they can be saved to
a new location. Restoration never creates missing files on disk.

Recovery snapshots remain an independent fallback. A matching snapshot newer than
the session file is restored in preference to older checkpoint text. Other recovery
candidates remain available through `SPC f v`. Recovery ordering depends on filesystem
modification times. Checkpoints are periodic, so edits since the last successful
write can still be lost in a crash.

Malformed, unsupported, or oversized sessions are left untouched and automatic
session writes are blocked. `:session-save` explicitly replaces the retained file
with the current editor state. A persistent `[session failed]` indicator reports
session read/write failures. The session limit is 64 MiB; save large unsaved files
before retrying. Loading also validates document references, split ratios, focus,
and bounded window/workspace/pane counts.

## Configuration

Under `[windows]` in `config.ini`:

```ini
[windows]
restore_session = false
```

This disables session reads and writes, including the explicit session commands.
The default is enabled. `restore_windows = false` still restores documents and
workspaces, combining saved windows into the primary window. Otherwise, native
window placement uses the existing saved window geometry.

Session serialization and disk writes currently run on the UI thread. Large-session
latency, live multi-monitor window recreation, and Linux-host smoke testing remain
follow-ups. The normal single-instance launch path owns the session; independently
launched standalone instances do not have a session-file coordination protocol.
