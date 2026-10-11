# External editor integration

Photo managers, scripts and other tools often hand an image to an editor and take it back when
the user is done: a DAM's "Edit in", git's `core.editor`, `code --wait`, a build step that opens a
file for retouching. PhotoCraft supports that workflow directly:

| Need | How |
|---|---|
| Save writes the file that was handed over | File › Save writes back to an opened TIFF (with its layers), JPEG, PNG or WebP, keeping bit depth, ICC profile, EXIF and XMP ([Raster formats](../formats/raster-formats.md)) |
| Know when it was saved or closed | `events.subscribe` on the [control protocol](control-protocol.md) pushes `document.opened`, `document.saved` and `document.closed` |
| Block until the user is done | `photocraft --wait <file>…` |
| One window, however often it is launched | Preferences › General › Single Instance or `--single-instance` (opt-in) |
| Feature-detect | `app.capabilities` lists `features` (`saveInPlace`, `events`, `wait`, `singleInstance`) and the `version` |

## `--wait`

```sh
photocraft --wait photo-edit.tif && echo "saved" || echo "closed without saving"
```

The process opens the files and exits once every one of them has been closed:

- exit status **0** when any of them was saved at least once (File › Save, Save As over it, or a
  confirmed flatten into a JPEG, PNG or WebP);
- exit status **1** when they were all closed without saving, or could not be opened;
- **2** for a usage error (`--wait` without files).

When the last waited file closes and no other document is open, the window closes by itself. If
other documents are open the app stays, and the process exits when the user quits it, with the
status the waited files earned. Quitting before closing them works too: the status is 0 if any of
them had been saved.

As a git editor for an image-producing step, or from a photo manager's external-editor setting:

```sh
git config core.editor "photocraft --wait"      # for tools that pass an image path
```

## Single instance

Off by default. Turn on **Preferences › General › Single Instance**, or pass `--single-instance`
for one launch, and launching PhotoCraft while it already runs hands the files to the running
window instead of opening a second one (with no files, it just brings that window forward).
`--new-instance` always starts a separate process.

`--wait` works across the hand-over: the second launch stays until the running window reports
that its files are closed, and exits with the same status as above (0 saved, 1 unsaved; 2 if the
running window refused the files). If the running window quits first, the status is 1.

How it works: the running instance listens on a random `127.0.0.1` port and records it in
`instance.json` in the settings directory, beside `instance.token`, a fresh 256-bit token per
instance (mode 0600 on Unix). A second launch authenticates with that token and sends
`app.openFiles {paths, wait}`. That socket accepts only `app.openFiles`, `app.capabilities`,
`events.subscribe` and `ui.focus`; anything else is refused. A stale record (the instance
crashed) is detected by the failed connection, and the new launch becomes the instance. It is
plain loopback TCP, so Linux, Windows and macOS behave alike (on macOS, Finder and Dock opens
already reach the running app as Apple events).
