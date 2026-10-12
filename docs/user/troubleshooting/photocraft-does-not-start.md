---
title: PhotoCraft does not start
description: PhotoCraft closes or crashes at startup on Windows PCs with Intel graphics (0.7.0 uses DirectX 12 and picks a safer graphics mode after a crash) or on older Intel Macs (no fix yet).
order: 30
verified: 0.7.0
---

On Windows with PhotoCraft 0.7.0, Intel graphics already start on DirectX 12. If PhotoCraft closed while it was starting, start it again: it picks a safer graphics mode after a crash. On an older Intel Mac that shows a `Device(Lost)` error, there is no fix yet.

## Symptoms

- Windows with Intel graphics (for example Intel UHD Graphics 630): PhotoCraft closes or crashes right after you start it. Windows Event Viewer shows an application error in `igvk64.dll` with exception `0xc0000005` ([issue 2320](https://github.com/storytold/photocraft/issues/2320), [issue 4](https://github.com/storytold/photocraft/issues/4)).
- Intel Mac: the app icon bounces in the Dock and the app disappears. Started from Terminal, it prints `Error: Wgpu(RequestDeviceError(RequestDeviceError { inner: Core(Device(Lost)) }))` ([issue 1859](https://github.com/storytold/photocraft/issues/1859)).

## Fix

### Windows

1. Use PhotoCraft 0.7.0 or later. Older builds do not have the steps below.
2. Start PhotoCraft again. Double-clicking its icon or a `.pcraft` file works. PhotoCraft notes that the last start did not finish and uses the next safer graphics mode this time:
   - A start on Vulkan is followed by one on DirectX 12.
   - A start on DirectX 12 is followed by one on the CPU. The CPU mode is slower but does not depend on the graphics driver.
3. If it started, nothing more to do. PhotoCraft remembers the mode that worked. You can see or reset it under **Preferences › Performance**.

If PhotoCraft still closes on every start, force DirectX 12 from PowerShell:

1. Open PowerShell in the folder that contains `photocraft.exe`.
2. Run these three lines:

        $env:WGPU_BACKEND = "dx12"
        & .\photocraft.exe
        Remove-Item Env:WGPU_BACKEND

The last line clears the setting so later launches use the default again. You need the first two lines each time you start PhotoCraft this way. While `WGPU_BACKEND` is set, PhotoCraft uses it and does not step down to a safer mode by itself ([issue 2320](https://github.com/storytold/photocraft/issues/2320)).

### Intel Mac

There is no fix yet. People on stock Intel Macs and on Macs patched with OpenCore Legacy Patcher report the same error, and setting `WGPU_BACKEND`, `--safe-gpu` and `PHOTOCRAFT_CPU_CANVAS=1` did not change it ([issue 1859](https://github.com/storytold/photocraft/issues/1859)). Add your Mac model and PhotoCraft version to that issue.

## Why this happens

On Windows, PhotoCraft draws with your graphics driver. Intel's Vulkan driver (`igvk64.dll`) crashed PhotoCraft on a UHD 630 PC with PhotoCraft 0.6.0, and the DirectX 12 setting above avoided the crash ([issue 2320](https://github.com/storytold/photocraft/issues/2320)). Since 0.7.0 PhotoCraft prefers DirectX 12 on Intel graphics. Before it opens the window, it writes a small file called `gpu-starting.json` in its settings folder and deletes it once the first frames are drawn. A start that crashed leaves the file behind, and the next start reads it and moves to the next safer mode.

## Still stuck?

See [Get help with a problem](get-help.md). Say which graphics card you have, which PhotoCraft version you use and how you started it. Add one of these:

- If you can open PhotoCraft: the text from **Help › System Info…**.
- If PhotoCraft does not open at all: the log file `photocraft.log`, plus the faulting module and exception code from Event Viewer on Windows. PhotoCraft keeps it in a `logs` folder inside its settings folder:
  - Windows: `%APPDATA%\Photocraft\logs\photocraft.log`
  - macOS: `~/Library/Application Support/Photocraft/logs/photocraft.log`
  - Linux: `~/.config/photocraft/logs/photocraft.log`
  - Portable copy (a `portable.txt` file next to the program): the `PhotoCraftData\logs` folder next to the program.

  Every start moves the previous log to `photocraft.1.log`. If you already started PhotoCraft again after the crash, attach `photocraft.1.log` too, because it holds the run that crashed.
