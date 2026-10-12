---
title: The Windows taskbar turns black when PhotoCraft is maximized
description: On Windows 11, the taskbar can turn black or disappear when PhotoCraft 0.7.0 is maximized or full screen. A related fix came after 0.7.0.
order: 10
verified: 0.7.0
---

This is a known bug in PhotoCraft 0.7.0 on Windows 11. A related fix is already merged but is not in 0.7.0, so update when a newer release is out. Until then, you can try the **System title bar** setting below, which is not confirmed to help.

## Symptoms

- You maximize the PhotoCraft window, or switch it to full screen, and the Windows taskbar turns completely black or is hidden.
- After you resize the window and maximize it again, a black bar the height of the taskbar stays at the bottom of the screen.
- One reporter tested FilmCraft, LightCraft and VectorCraft on the same PC and did not see it there.

## Fix

1. Check your version with **Help › System Info…**, which starts with "PhotoCraft" and the version. If it is 0.7.0, the related fix is not in your build.
2. Download a newer release from the [releases page](https://github.com/storytold/photocraft/releases) once one is published after 0.7.0, or update with [ArtCraft Launcher](https://getartcraft.com/docs/craft-launcher).

## Try the system title bar

PhotoCraft normally draws its own title bar. With **System title bar** switched on, Windows draws it instead. This is only a workaround to try. Nobody has confirmed that it fixes the black taskbar.

1. Choose **Edit › Preferences › Interface…**.
2. Switch on **System title bar**. The dialog says "Applies at next launch."
3. Click **Apply**, then close and start PhotoCraft again.
4. Maximize the window and check the taskbar.

If it does not help, switch **System title bar** off again the same way.

## Why this happens

A fix for the taskbar going missing when a window is maximized (issue 3144, which looks closely related) was merged into PhotoCraft's main branch on 11 October 2026 ([pull request 3308](https://github.com/storytold/photocraft/pull/3308)). PhotoCraft 0.7.0 was published before that, so it does not include the change.

## Still stuck?

Add your Windows version and PhotoCraft version to [issue 3864](https://github.com/storytold/photocraft/issues/3864) or [issue 3550](https://github.com/storytold/photocraft/issues/3550), or ask in [Discord](https://discord.gg/artcraft). See [Get help with a problem](get-help.md) for what to include.
