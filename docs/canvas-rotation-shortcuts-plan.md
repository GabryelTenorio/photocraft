# Canvas view rotation shortcuts: issue #4911

Status: planning placeholder; implementation and tests pending.

Refs https://github.com/storytold/photocraft/issues/4911

Add keyboard commands to rotate the canvas view left/right by 15 degrees. Change view state only: do not resample pixels, mutate document content or add destructive image-rotation history entries.

Before implementation:

- Inspect existing view.rotateView, rotation/reset handling and command dispatch; reuse view state conventions.
- Review related #4042 (Shift + middle-mouse drag) and #4227 (flipped-view direction).
- Audit default shortcut collisions, text-input/modal focus and platform modifiers before selecting bindings.
- Expose both actions through configurable bindings; verify rebinding, persistence and discoverability.
- Define visual left/right direction in flipped views, angle wrapping and steps from non-multiples of 15 degrees.
- Verify repeated presses accumulate one 15-degree step per command; existing reset returns to zero without editing the document.

Pending verification: focused command/input regressions for both directions, repeated presses, wrapping, reset, configurable bindings, shortcut collisions and unchanged pixels/history; visual verification and relevant repository checks after implementation.

Documentation only. No feature implemented, build or feature tests run.
