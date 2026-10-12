---
title: PhotoCraft does not start
description: PhotoCraft closes or crashes at startup on Windows PCs with Intel graphics (start it with DirectX 12) or on older Intel Macs (no fix yet).
order: 30
verified: 0.7.0
---

If PhotoCraft closes or crashes as soon as you start it on a Windows PC with Intel graphics, start it with DirectX 12 from PowerShell. On an older Intel Mac that shows a `Device(Lost)` error, there is no fix yet.

## Symptoms

- Windows with Intel graphics (for example Intel UHD Graphics 630): PhotoCraft closes or crashes right after you start it. Windows Event Viewer shows an application error in `igvk64.dll` with exception `0xc0000005` ([issue 2320](https://github.com/storytold/photocraft/issues/2320), [issue 4](https://github.com/storytold/photocraft/issues/4)).
- Intel Mac: the app icon bounces in the Dock and the app disappears. Started from Terminal, it prints `Error: Wgpu(RequestDeviceError(RequestDeviceError { inner: Core(Device(Lost)) }))` ([issue 1859](https://github.com/storytold/photocraft/issues/1859)).

## Fix

### Windows with Intel graphics

1. Open PowerShell in the folder that contains `photocraft.exe`.
2. Run these three lines:

        $env:WGPU_BACKEND = "dx12"
        & .\photocraft.exe
        Remove-Item Env:WGPU_BACKEND

The last line clears the setting so later launches use the default again. You need the first two lines each time you start PhotoCraft this way.

Double-clicking a `.pcraft` file starts PhotoCraft without this setting ([issue 4](https://github.com/storytold/photocraft/issues/4)). Start PhotoCraft from PowerShell first, then open the file with **File › Open…**.

### Intel Mac

There is no fix yet. People on stock Intel Macs and on Macs patched with OpenCore Legacy Patcher report the same error, and setting `WGPU_BACKEND`, `--safe-gpu` and `PHOTOCRAFT_CPU_CANVAS=1` did not change it ([issue 1859](https://github.com/storytold/photocraft/issues/1859)). Add your Mac model and PhotoCraft version to that issue.

## Why this happens

On Windows, PhotoCraft draws with your graphics driver. Intel's Vulkan driver (`igvk64.dll`) crashed PhotoCraft on a UHD 630 PC with PhotoCraft 0.6.0, and the DirectX 12 setting above avoided the crash ([issue 2320](https://github.com/storytold/photocraft/issues/2320)). Another user reported crashes on Intel UHD Graphics 620 when starting a new document and adding text; the cause there is not confirmed ([issue 3824](https://github.com/storytold/photocraft/issues/3824)).

## Still stuck?

See [Get help with a problem](get-help.md). Include the text from **Help › System Info…** and, on Windows, the faulting module and exception code from Event Viewer. If PhotoCraft does not open at all, say which graphics card you have and how you started it.
