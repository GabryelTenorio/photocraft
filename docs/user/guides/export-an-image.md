---
title: Export an image
description: Export a PNG, JPG, WebP, TIFF or TGA from PhotoCraft with a preview, a size estimate and a scale.
order: 50
verified: 0.7.0
---

Choose **File › Export › Export As…**, set the format, and click **Export**. The dialog shows a preview and an estimate of the file size before you save. Your layered document is not changed.

## Export with settings

1. Choose **File › Export › Export As…**.
2. Under **File Settings**, pick a **Format**: **PNG**, **JPG**, **WebP**, **TIFF** or **TGA**.
3. Set the options for that format:
   - **JPG** and lossy **WebP** have a **Quality** slider from 1 to 100.
   - **WebP** also has a **Lossless** checkbox.
   - **Transparency** keeps transparent areas. JPG has no transparency, so the image is flattened for JPG.
   - **Metadata** is **None** or **All**.
4. Under **Image Size**, set **Scale** in percent. The size in pixels shows below it.
5. Check the preview on the right. The line under it shows the format and the estimated file size.
6. Click **Export**, then choose a name and a place for the file.

PhotoCraft shows "Exported" with the file path and size in the status bar when it is done.

## Export in one step

**File › Export › Quick Export as PNG** exports without asking about settings. It uses the format, quality, metadata and location you set in **File › Export › Export Preferences…**.

## Export one layer

Choose **Layer › Export As…** (or **Layer › Quick Export as PNG**) to export only the active layer.

## Keep your layers

Exporting makes a flat copy. To keep the layers, use **File › Save As…** with **PSD Document** or **PhotoCraft**; see [Open and save files](open-and-save-files.md).
