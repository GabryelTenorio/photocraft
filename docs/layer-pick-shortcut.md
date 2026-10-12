# Optional layer-under-cursor shortcut

This proposes a PhotoCraft extension for #2298, not a Photoshop factory shortcut.
Assign Select Layer Under Cursor under Preferences > Keyboard Shortcuts > Layer.
There is no default binding. The separate engine command is layer.selectUnderCursor
with explicit x/y coordinates for automation; keyboard invocation supplies the cursor.

The existing layer.pickAt label, default binding, and Auto-Select behavior are unchanged.
The optional command replaces the selection with the topmost eligible layer, preserves
the active tool, uses the latest canvas mapping, and ignores empty or outside points.
Typing, modal dialogs, active painting and transforms retain their input.
