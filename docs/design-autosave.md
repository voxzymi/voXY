# Design note: autosave and crash recovery

**Status: implemented** (it was on the deferred list and must not be dropped).
The approved shape is in `design-proposal.md` §8; this note records the
decisions that were left open and where the code is.

## What the user sees

- While a project has unsaved changes, voXY writes a copy about 60 seconds after
  the first unsaved edit (not during a drag or while typing, and not behind a
  dialog), then again 60 seconds after any later edit. Nothing is written for a
  project that has no unsaved changes.
- After a crash, a killed process or a power loss, opening the project (or
  starting voXY with no project, for an untitled one) shows **"Recover unsaved
  changes?"** with two buttons: **Recover** loads the copy as an *unsaved*
  change to the project (the project file is untouched until you save, and the
  copy stays until you do); **Discard** deletes it. There is deliberately no
  "later": while the question is open nothing is written over the copy, since it
  may be the only record of the work.
- Saving, **Don't Save** at quit or before opening another project, and undoing
  everything back to the saved state all delete the copy. A clean exit leaves
  nothing behind, so **a copy that is still there means something went wrong**.

## Files

| Project | Autosave | Notes |
|---|---|---|
| Saved (`MyProject.voxy`) | `MyProject.voxy.autosave` beside it | Offered at open only if newer than the project file. |
| Untitled | `<config>/recovery/<session>.voxy.autosave` | `<config>` is the per-user config directory (`~/.config/voXY`, `~/Library/Application Support/voXY`, `%APPDATA%\voXY`). |

Both are the ordinary `.voxy` envelope, `ui_state` included, written atomically
(temp file, fsync, rename) by `persist::save_with_ui`. **The project file format
does not change**, and an autosave opens in any build that opens the project.

## Telling a crash from a window that is still open

Each window that needs an untitled autosave holds an exclusive operating-system
lock (`File::try_lock`, in the standard library) on `<session>.lock` in the
recovery folder. The OS releases it when the process ends for any reason, so an
untitled copy whose lock can be taken belongs to a dead session and is offered;
one whose lock is held belongs to a window that is open and is left alone. No
process ids and no extra dependency.

Not handled: two windows with the **same saved project** open at once (voXY has
no such guard yet); the second could be offered the first's live autosave.

## Settings

`autosave_seconds` in `preferences.toml` (default 60; `0` turns autosave off).
There is no settings dialog yet, so the file is the only way to change it; the
dialog belongs to the Preferences milestone (P-D).

## Choices worth a second look

- The interval and default are mine, not from the approved design (which says
  only "every N seconds after an edit").
- A close that the user did not confirm while there are unsaved changes (the
  system ending the session) keeps the copy, like a crash, instead of deleting
  it.
- Only the newest crashed untitled project is offered at launch; older ones stay
  in the recovery folder and are offered one per launch after that.

## Code

`crates/voxy-ui/src/recovery.rs` (paths, offers, orphans, the session lock),
`state.rs` (`autosave_tick`, `recover`, `discard_recovery`, `finish_session`),
`app.rs` (the per-frame tick, the prompt, `on_exit`), `prefs.rs`
(`autosave_seconds`), and `History::reset_unsaved` in `voxy-core` (a recovered
document starts as modified).
