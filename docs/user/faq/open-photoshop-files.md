---
title: Can PhotoCraft open my Photoshop files?
description: PhotoCraft opens and saves layered PSD and PSB files and keeps what it does not understand yet.
order: 20
verified: 0.7.0
---

Yes. PhotoCraft opens, edits and saves layered PSD and PSB files, including 16-bit, 32-bit, CMYK and Lab documents.

When it saves the file again, the picture renders the same for 307 of the 309 files in the psd-tools test set the project checks against. Parts PhotoCraft does not model yet, such as raw blocks and descriptors, are carried over instead of being dropped. A re-saved file is not byte for byte the same as the original, because PhotoCraft rewrites image resources, layer records and the composite.

Some PSD text layers move when you start editing them in version 0.7.0; see [Text moves when I start editing a PSD text layer](../troubleshooting/text-moves-when-editing.md).

PhotoCraft also opens layered TIFF, Paint.NET, OpenRaster, SVG and Affinity files, and many flat formats. The full list is in [Open and save files](../guides/open-and-save-files.md).
