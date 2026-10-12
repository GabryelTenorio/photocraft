# Short gradient scale compatibility (draft, #1243)

This proposal removes PhotoCraft's canvas-relative 10% minimum for live handles,
retaining the existing 150% maximum and a half-document-pixel geometric guard.
The stationary handle remains fixed through that guard. Preview, Properties,
CPU/GPU rendering and native/PSD reopening use the same geometry.

The PSD round-trip regression exports and reimports every gradient style at
8, 16 and 32 bit. This checks PhotoCraft's PSD writer/reader, not Photoshop itself.

Adobe's current documentation says the drag direction and length control a
gradient, and the developer manifest describes fill.scale as a percentage:
- https://helpx.adobe.com/photoshop/desktop/adjust-color/color-effects-techniques/apply-gradient-fill.html
- https://developer.adobe.com/firefly-services/docs/photoshop/guides/photoshop-v2/manifest-response-reference

Neither source guarantees that desktop Photoshop preserves arbitrary sub-10%
gradient-layer Scale values. The 1e-9 numeric guard is not a Photoshop UI limit.
This stays draft pending a direct Photoshop comparison; no parity claim is made.

Manual interop check: on a 600 x 400 transparent document, draw a live gradient
from (100,100) to (100.5,100), inspect Properties Scale, save as PSD, reopen it
in Photoshop, then reopen the Photoshop-saved file in PhotoCraft. Repeat for all
five styles and record the Photoshop version, handles, percentage and appearance.
Also open the PhotoCraft-generated sub-10% PSDs and compare their appearance
before/after a Photoshop save. Clamping, rasterization or handle displacement
must be resolved before this draft is ready.
