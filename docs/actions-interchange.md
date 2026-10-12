# Loading and saving actions

In **Window > Actions**, open the footer's **Actions** menu and choose **Load…** or **Save…**.
Load accepts Photoshop `.atn` files and PhotoCraft `.pcactions` / `.json` action sets.
File > Open and drag-and-drop also recognize `.atn` and `.pcactions`.
Loading opens the Actions panel without editing the active document. Select an action and
press Play to apply it through the existing engine. Stop recording or playback before loading
or saving.

Save writes the entire library as `.pcactions` (JSON), retaining action names, order, parameters,
disabled steps and nested action calls. `.json` is also accepted as a save extension. Calls
using numerical action indices are converted to names so loading beside another library cannot
redirect them. The same version-1 JSON shape is used by PhotoCraft's existing preset store.
Save does not export Photoshop `.atn` files.

Loading appends a validated set. Empty or conflicting action names, unsupported commands,
invalid step indices, broken nested references and malformed files fail without adding any
actions. Rename an existing action before loading a set with the same names. Cancellation and
failed writes leave the library intact. Files are limited to 32 MiB, 10,000 actions and 10,000
total steps; descriptors use the existing bounded PSD parser.
Names are limited to 1,024 UTF-16 units in `.atn` and 4,096 UTF-8 bytes in `.pcactions`;
event/command identifiers to 256 bytes and Photoshop step labels to 4,096 bytes. Unknown
parameter keys are abbreviated in errors so hostile labels cannot create enormous UI messages.

## Photoshop compatibility

This is a conservative **version-16 import subset**, not full Photoshop action compatibility.
The parser reads the layout documented by the contributor in [PR #1257](https://github.com/storytold/photocraft/pull/1257)
and uses the existing PSD ActionDescriptor implementation. The new parser implements that
documented layout independently; it does not include code or commits from that open PR.

| Event | PhotoCraft command | Parameters |
|---|---|---|
| Invert | `image.adjustments.invert` | none |
| Desaturate | `image.adjustments.desaturate` | none |
| Flatten Image | `layer.flattenImage` | none |
| Merge Visible | `layer.mergeVisible` | none |
| Merge Layers | `layer.mergeLayers` | none |
| Gaussian Blur | `filter.blur.gaussianBlur` | radius in pixels |
| Brightness/Contrast | `image.adjustments.brightnessContrast` | brightness, contrast, legacy algorithm |
| Exposure | `image.adjustments.exposure` | exposure, offset, gamma |
| Posterize | `image.adjustments.posterize` | levels |
| Threshold | `image.adjustments.threshold` | level |

String and four-character event identifiers are accepted for the documented aliases. Unknown
events, unknown or duplicate parameter keys, unsupported value types/units, non-finite or
out-of-range values, and enabled steps that require a Photoshop dialog reject the **entire set**.
The error names the action and step. Even disabled unsupported events are rejected, since
users can later enable them. Enabled/disabled flags are retained. Function-key bindings,
button colours and expansion state are omitted with an import warning.

Levels, Curves, Hue/Saturation, recorded selections/targets, Photoshop scripts and plug-ins
remain unsupported. Format and mapping tests use synthetic files; actual Photoshop-authored
file acceptance and pixel parity remain unverified.

## CLI, control channel and MCP

Both operations are registered engine commands, including in the WebAssembly build:

```json
{"command":"actions.import","params":{"format":"atn","data":"<base64 file bytes>"}}
{"command":"actions.import","params":{"format":"json","data":"<base64 file bytes>"}}
{"command":"actions.export","params":{}}
```

Import returns `{count, index, imported, warnings}`. Export returns
`{filename, data, count}`; decode `data` to write the `.pcactions` file. These commands
accept and return bytes rather than reading or writing paths. Interactive pickers use the
existing desktop/browser file services. Playback keeps the existing per-step authorization,
Undo and disabled-step behavior, and neither interchange operation is recorded as an action
step.

## Request evidence

- [Reddit: importing Photoshop actions](https://www.reddit.com/r/ArtCraftAI/comments/1x173gp/importing_actions/)
- [GitHub: missing action import UI](https://github.com/storytold/photocraft/issues/3419)
- [GitHub: no action load/save](https://github.com/storytold/photocraft/issues/1951)

PDF import is a separate [existing contribution, #2751](https://github.com/storytold/photocraft/pull/2751).
The [ComfyUI bridge](https://github.com/the-aSak/ComfyUI-Photocraft-Bridge) is a separate community project.
Neither is included in this change.
