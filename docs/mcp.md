# MCP conventions

PhotoCraft follows the same conventions as FilmCraft #28. Start a headless session:

```sh
photocraft-cli mcp \
  --automation-read-root /work/project \
  --automation-read-root /library/reference \
  --automation-write-root /work/project
```

Repeat `--automation-read-root` to grant ordered read directories in headless `mcp`, `serve`,
or desktop `photocraft --control`. Both `--flag <dir>` and `--flag=<dir>` work and can be mixed;
duplicate read roots are allowed. Relative file requests search the roots in launch order.
Headless reads select the first existing regular file; only a missing file tries the next root.
Directory, permission, path-escape and decoding errors stop headless requests instead of falling back.
Desktop `app.open`, including bridged `doc_open`, also supports native directory `.pcraft`
documents through the selected read capability. An earlier root's ZIP file wins over a later
root's directory; loading errors in a selected directory are terminal. Directory opens have
no implicit save target, even through the write root's capability, and need explicit layered
Save As beneath the write root.
All configured roots must be valid existing directories; an empty, missing or invalid root
fails startup rather than being dropped.

There is still only one `--automation-write-root`. Repeating it is a usage error (exit 2),
even with an identical value or mixed flag syntax. Writes are relative to that root, with no
fallback to any read root. Read and write authority are independent; a write root does not grant
reads, and omitting either kind of root denies that direction of filesystem access.
Absolute request paths must be beneath a corresponding granted root and are opened through
its directory capability, without ambient filesystem resolution of the requested path.

A document opened through a read root that does not share the write root's held directory
capability has no implicit save target. The same configured root shares that capability;
canonical filesystem identity is not inferred. Use an explicit `doc_save {path:"copy.pcraft"}`
(or another supported layered format) beneath the write root to establish one. Aliases and nested roots conservatively also
require explicit Save As; a flat export does not establish a save target. Existing restrictions
on pathless saves still apply. These grants do not enable ambient asset reads or headless
directory-bundle access; desktop directory `.pcraft` reads remain scoped to the granted capabilities.

Bridge mode uses `--bridge` and `--control-token-file` as described in
[development.md](development.md). Configure all roots on the desktop process; passing root flags
to `photocraft-cli mcp --bridge` is a usage error because the bridge cannot grant them.
UI tools require a running desktop app. The desktop environment variable
`PHOTOCRAFT_AUTOMATION_READ_ROOT` supplies one fallback directory only when no explicit read-root
flags are present; explicit flags replace it, preserving CLI order. Similarly, one explicit
write-root flag overrides `PHOTOCRAFT_AUTOMATION_WRITE_ROOT`. These variables do not grant
headless CLI file access.

## Tools and resources

- `command_list`: discover engine ids, parameter docs and current enabled state; accepts
  `filter` and `enabled_only`.
- `command_run`: `id`, optional `params` and `wait`. Command parameters remain engine-defined.
- `command_batch`: ordered `steps`, optional `stop_on_error`; one history step per command,
  not an atomic transaction. Existing per-step results and reply budgets are preserved.
- `doc_inspect`: layer tree, history and selection; an empty session returns `document:null`.
- `render_preview`: bounded PNG content; `index` and `max_side` (default 1024, maximum 2048).
- `ui_inspect` and `ui_screenshot`: live app state/window in bridge mode.

Existing document/session/job/UI tools remain listed, including the documented
`doc_render_preview` spelling. There are no hidden legacy aliases. Every tool has a title
and all four annotation hints. Generic command/control tools conservatively advertise writes;
file saves may replace existing targets. Hints do not grant permission.

Unknown tool arguments return JSON-RPC `-32602`, naming the key, before execution. Batch
step envelopes are strict too. Nested engine `params` retain their command-specific handling.
Tool execution failures return `isError:true`. A worker panic becomes a tool error and the
existing poisoned-session recovery keeps serving. Invalid JSON lines receive `-32700` with
`id:null`; the next line is still processed.

Embedded Smart Objects support `command_run` with `layer.smartObjects.editContents` and
`layer.smartObjects.convertToLayers`, optionally targeting `params.layer`. Use
`layer.smartObjects.saveContents` in an opened contents document to update its parent in memory;
save nested contents from the inside outward, then `doc_save` the parent to a scoped path.
`doc_save` on a contents document exports that document rather than updating its parent.
No filesystem root is needed for the in-memory commands. External linked sources that require
a file read remain refused. See the [control policy](control-protocol.md#engine-commands)
for the shared restrictions.

`photocraft://document` and `photocraft://commands` return live JSON matching `doc_inspect`
and `command_list`. Resource reads are uncached; tool/resource catalogs are private and cached
for ten minutes. List/read responses include the MCP 2026-07-28 result/cache fields.

## Progress and cancellation

A direct headless `command_run` of `file.export.renderVideo` reports frame counts when
`_meta.progressToken` is present (a string or number). Notifications increase strictly,
are throttled to at most ten per second except completion, and include `total`. No token
means no progress notifications. Ping remains responsive while rendering.

```json
{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"file.export.renderVideo","params":{"dir":"frames","format":"png"}},"_meta":{"progressToken":"export-20"}}}
{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":20}}
```

Create a document and timeline first (`doc_new`, then `timeline.create`). Export uses the
configured write root and creates `dir` beneath it. Filenames must be single components;
absolute paths, traversal and escaping symlinks are rejected. Existing target filenames
are refused, including symlinks, so a cancelled or failed job cannot replace an earlier export.
Only files actually created by this job are removed on cancellation, error or an escaped panic;
unrelated files and the destination directory remain. PNG sequences and animated GIF share
the engine renderer. The document and playhead are unchanged.

Cancellation suppresses the request response and stops at a frame boundary (the current
frame/encoding may finish first). Unknown request ids are ignored. Render Video stays a
synchronous command even with `wait:false`; other job-capable commands keep their existing
`jobs_list`/`jobs_cancel` behavior. Batch exports do not emit MCP progress or support per-step
MCP cancellation. Nested engine actions and bridge Render Video remain denied by the existing
ambient-filesystem policy; the new scoped writer is available for direct headless commands.
