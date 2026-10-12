# Toolbar shortcut captures

Original PhotoCraft UI captures, with no artwork. MIT OR Apache-2.0, like the UI source.

- `toolbar-shortcuts-before.png`: production source at `3dd5fc1c`, searching for Pencil Tool
  in Edit › Keyboard Shortcuts. No matching row is available.
- `toolbar-shortcuts-after.png`: the same search with this change, after assigning Pencil to F6.

Both use the offscreen `snapshot` example at 1100 × 760, scale 1, with `--safe-gpu`.
The window is rendered by Vulkan llvmpipe; the canvas uses the CPU renderer.

Before script:

```json
[["ui.menu.invoke", {"id":"edit.keyboardShortcuts"}],
 ["ui.dialog.set", {"dialog":1,"field":"filter","value":"Pencil Tool"}]]
```

After script:

```json
[["engine.execute", {"command":"edit.keyboardShortcuts","params":{"set":{"tools.select.pencil":"F6"},"allowUnknown":true}}],
 ["ui.menu.invoke", {"id":"edit.keyboardShortcuts"}],
 ["ui.dialog.set", {"dialog":1,"field":"filter","value":"Pencil Tool"}]]
```
