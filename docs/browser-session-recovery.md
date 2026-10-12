# PhotoCraft browser recovery

Status: Rust implementation under review. No parity score increase is claimed.

Recovery uses Rust futures and web-sys IndexedDB APIs. Controls use egui theme visuals.
There is no handwritten JavaScript, inline JavaScript, npm dependency or DOM stylesheet.
Storage loading never blocks the editor. Every load uses a fresh random writer id so
reloads and duplicated tabs cannot overwrite another page's checkpoint. A reload reads
its previous committed id and a later visit offers the latest checkpoint. Snapshot,
previous checkpoint and discovery pointer commit atomically. Failed saves show Retry.
Restore/import require closing current documents in PhotoCraft and VectorCraft.
LightCraft replaces its loaded catalog on explicit restore. Its original photos remain
in existing content-addressed storage and recovery copies contain references to them.

Checkpoints preserve serializable workspace state and undo/redo history. Capture waits
for inactivity and pointer release. They are bounded to 512 MiB without history truncation.
Hard termination may lose changes since the last completed write. Storage eviction and
clearing site data remove recovery. Full-history capture can be expensive.
The legacy draft's JavaScript database is retained without migration or deletion.
Recovery import/export and persistent-storage requests are explicit user actions.
No fonts, images, external assets, brand changes or launcher code are included.

Validation and outstanding browser/platform checks are recorded in the PR description.
Revision history: 2026-10-12 — replace the noncompliant JavaScript coordinator with Rust.

Recovery operations are exposed through the existing engine command registry as
`session.recovery.capture {ui?}`, `session.recovery.inspect {bytes}` and `session.recovery.restore {bytes}`.
They are available to existing CLI/control/MCP dispatchers. JSON byte-array transport
is capped at 8 MiB. Browser storage uses the same engine serializer through the binary
adapter with a 512 MiB bound. Command restore requires an empty engine session and
returns UI metadata for the calling shell to apply.

## Contribution review and validation

Reviewed the public `AGENTS.md`, `docs/contributing.md` and architecture rules.
Formatting, native Clippy for engine/UI/web (all targets, warnings denied), WebAssembly
compilation and strict WebAssembly Clippy passed. The native engine integration tests
passed (three tests covering history round trips, wrong-app/malformed envelopes and
registry-dispatched recovery commands). All 15 native web-host tests passed. The
workspace layer checker passed. No new assets, fonts or branding changes are included.
Contributor credit remains @mmmbeer as requested. No lint or quality gate was weakened.

Full workspace CI is not certified locally. Its formatting and Clippy stages passed,
but the full workspace test build was not completed within this runner's resources.
An earlier all-lib test compiler was killed with SIGKILL. The browser storage probe is
compiled behind `browser-storage-tests` but has not been executed in a real browser.
Real browser clipboard/storage QA and visual screenshots remain required before merge.
The referenced private craftrules repository was inaccessible (404), so its additional
standards cannot be certified from the published repository instructions alone.
