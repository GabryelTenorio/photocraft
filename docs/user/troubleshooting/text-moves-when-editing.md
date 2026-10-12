---
title: Text moves when I start editing a PSD text layer
description: In PhotoCraft 0.7.0, a text layer from a Photoshop file can jump when you click it with the Type tool. There is no released fix yet.
order: 20
verified: 0.7.0
---

This is a known bug in PhotoCraft 0.7.0 and there is no released fix yet. If you only need the file to look right, do not click its text layers with the **Horizontal Type Tool**, and keep an untouched copy of the PSD.

## Symptoms

- You open a PSD that was made in Photoshop and click one of its text layers with the **Horizontal Type Tool**. The text shifts, often to the left, as soon as editing starts.
- Centered text may show as left-aligned in the options bar.
- In some files, text that you click can disappear.

## What to do

1. Keep the original file safe. Before you edit text in a PSD, make a copy with **File › Save a Copy…**.
2. Edit other layers with other tools. Only starting to edit the text redraws it.
3. Report your file. Attach the PSD (zipped) to [issue 3795](https://github.com/storytold/photocraft/issues/3795) and say which Photoshop version saved it.

## Why this happens

Clicking a PSD text layer with the Type tool starts editing it, and PhotoCraft then redraws the text with its own layout instead of the pixels Photoshop saved. A contributor found two causes in how PhotoCraft reads the file: it does not read the alignment and styles of each run of text the way Photoshop wrote them, and it does not use the text position Photoshop stores. A proposed fix is open as [pull request 3819](https://github.com/storytold/photocraft/pull/3819) and is not merged yet.

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or see [Get help with a problem](get-help.md).
