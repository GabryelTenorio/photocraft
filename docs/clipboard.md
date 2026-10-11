# Editable masks on the layer clipboard

Updated 2026-10-11.

With no pixel selection, Copy takes a pixel layer with a raster or vector mask
through the existing whole-layer clipboard path, just like smart objects, type,
shapes and groups. Multiple selected layers also use that path. The existing
layer-selection helper preserves stack order and excludes children already
included through a selected parent.

Paste and Paste in Place reuse the existing layer duplication, colour/profile/
depth conversion and single history step. Raster/vector masks, hidden colour,
mask settings and layer content stay editable. Pasted layers and group children
receive fresh IDs. Matching-format tiles retain the existing copy-on-write
sharing. Moving the pasted copy moves its unlinked masks too; normal layer
movement still follows each mask's link flag.

The clipboard also carries a composited bitmap that respects the masks for
native bitmap export and bitmap destinations. Whole-layer placement uses the
underlying content bounds, so a fully hidden mask can still be copied without
losing its hidden colour or changing the paste position.

A source pixel selection, Cut and Copy Merged keep bitmap semantics. Targeted
colour/alpha channel Copy continues through the existing channel path. Paste
Into/Outside use the clipboard bitmap and their destination selection mask;
`edit.paste {pixels: true}` requests bitmap-only paste explicitly.

The native clipboard bridge is unchanged. Editable layers remain within one
application session; external bitmap/text replacement uses the bridge's existing
ownership signature. Copying vector paths, cross-process layered exchange,
bitmap-mode layer conversion and clipboard-to-new-document layer preservation
remain outside this extension.

Adobe documents copying [one or multiple selected layers](https://helpx.adobe.com/photoshop/desktop/create-manage-layers/create-layer-compositions/copy-and-paste-layers.html)
and retaining [bitmap/vector masks and effects](https://helpx.adobe.com/photoshop/using/create-layers-groups.html).
