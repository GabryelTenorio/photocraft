# Edit copies handed over by another app

A photo manager's "Edit in…" command renders a copy of a photo, opens it in an image editor and
reads the file again once the editor has saved it. LightCraft's **Edit in PhotoCraft** does this
with a 16-bit TIFF next to the original. The copy exists to be changed, so asking where to save it
only gets in the way, and a Save As under another name leaves the photo manager watching a file
that never changes.

## What PhotoCraft does

File › Save writes a document back to its own file only for a layered format (PSD, PSB, .pcraft,
OpenRaster). A flat file goes through Save As, so an original is never written over
(issue #416). That rule stays.

A launch with the environment variable `PHOTOCRAFT_EDIT_IN_PLACE` set (to anything but an empty
value or `0`) marks the files **on that launch's command line** as edit copies:

```sh
PHOTOCRAFT_EDIT_IN_PLACE=1 photocraft "/photos/2026/beach-Edit.tif"
```

For such a document, File › Save (Ctrl/Cmd+S) writes back to the TIFF it was opened from:

- no Save As dialog and no TIFF Options prompt, like a save to a PSD;
- layers, masks and the rest go into the TIFF's layer data, so the file reopens in PhotoCraft with
  them; a document that is still a lone Background is written as the flat image it was;
- the depth and the embedded colour profile are kept.

The mark is narrow on purpose:

- **Only that launch's command-line files.** A file opened later through File › Open, a drop, a
  recent file or an Apple event is an ordinary file.
- **Only a TIFF.** A JPEG or PNG can't hold a layered document, so it keeps the Save As rule.
- **Only while a TIFF keeps the document's colour mode and channels.** The mode has to be RGB,
  Grayscale or CMYK: a Lab or Multichannel document is not written as one, and an Indexed, Bitmap
  or Duotone document would come back without its palette or inks. Alpha channels are not written
  either. Save then asks where to save, as it does for any flat file.
- **Until the document moves.** A Save As that makes another file the document's file ends it. A
  Save As that writes a copy (a PNG of a layered document) does not: the document is still the
  TIFF.

What the save keeps is what a save to that TIFF through Save As keeps, no more:

- Layers go through the PSD writer, with its warnings (a smart filter Photoshop's format has no
  place for, for example), as in a save to a PSD.
- A document that is still a lone Background is written flat, and a flat TIFF carries no guides,
  paths, notes or layer comps.

The control channel's `app.save` is unchanged: without a `path` it writes back to a layered file
only.

An environment variable was chosen over a command-line flag because a PhotoCraft that doesn't know
it ignores it, while an unknown flag would be opened as a file name.

## For the app handing a file over

- Start PhotoCraft with the variable set and the file as an argument. On Windows and Linux every
  start is its own process, so the files you pass are the ones marked.
- On macOS files reach a running app as Apple events, not as arguments, and `open` does not pass
  the environment on: files opened that way are ordinary files and go through Save As.
- Read the file again when your window is next focused: PhotoCraft replaces it in one step when
  the save finishes.
- Expect layer data in the TIFF after an edit. A reader that takes the first image gets the
  finished picture.

Code: `DocState::edit_in_place` and `DocState::own_file` (`crates/engine/src/file_cmds.rs`),
`PhotocraftApp::open_paths_in_place` (`crates/ui-egui/src/file_open.rs`),
`photocraft_io::tiff_layers::holds`.
