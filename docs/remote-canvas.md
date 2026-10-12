# Remote Canvas

Draw in PhotoCraft from a tablet or phone with a pen or a finger, over your local network. The
desktop app serves a page that mirrors its window; the page sends pen, finger and gesture input
back. Nothing to install on the device: it runs in the browser (Safari on iPad and iPhone; Chrome,
Samsung Internet or Firefox on Android; Edge or Chrome on a Windows tablet). The desktop side runs
wherever PhotoCraft does (Windows, macOS, Linux).

## Use it

1. **Window › Remote Canvas…** › **Turn On** (or start PhotoCraft with `--remote [port]`, or set
   `PHOTOCRAFT_REMOTE=<port>`; the default port is 9876). It stays on across restarts until turned
   off (`remote-canvas.on` in the settings folder).
2. Scan the QR code with the device's camera, or open the address shown. Add it to the home screen
   for a full-screen app (Safari: Share › Add to Home Screen; Chrome: menu › Add to Home screen).
3. Both devices must be on the same network, and the desktop firewall must allow the port.
   Windows asks the first time PhotoCraft listens (allow Private networks). On Linux with ufw:
   `sudo ufw allow 9876/tcp`.

## Input

| Input | Does |
|---|---|
| Pen (Apple Pencil, S Pen, Surface Pen, Wacom…) | Draws, with pressure and tilt; hovering shows the brush outline (where the device reports hover) |
| Pen eraser end or eraser button | Draws as the eraser |
| One finger | Pans; a tap clicks |
| One finger, with the toolbar's ☝ toggle on | Draws (for phones and tablets without a pen); remembered by the browser |
| Hold one finger still | Picks the colour under it (the Eyedropper's Sample Size and Sample Layers) |
| Two fingers | Pan, pinch to zoom, turn to rotate (snaps to 90°) |
| Two-finger tap / three-finger tap | Undo / redo |

Touches while the pen is down, or just after it lifts, are taken for a resting palm. The toolbar
also has undo, redo, fit on screen and full window / canvas only.

## Security

The page and its WebSocket are plain HTTP on the local network. The WebSocket needs the pairing key
in the address (`?k=…`, 256 bits, stored in `remote-canvas.key` in the settings folder); without it
nothing connects. Only one page drives the app at a time: a newer one takes over.

## Building the page

The page is Rust compiled to WebAssembly (`apps/photocraft-remote`). Build it, then rebuild the app,
which embeds it:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked
cargo xtask remote-page
cargo build -p photocraft
```

Without that step the app serves a placeholder page saying the page wasn't built (and the build
prints a warning). Release builds need `cargo xtask remote-page` before building the app.

## How it works

- `apps/photocraft/src/remote_server.rs`: the HTTP and WebSocket server, the pairing key, and the
  frame encoder (only changed tiles are sent).
- `crates/ui-egui/src/remote.rs`: the window capture, turning page input into egui events and pen
  samples, colour picking, and the Remote Canvas window.
- `apps/photocraft-remote`: the page. `gesture.rs` (fingers), `finger.rs` (finger drawing),
  `smooth.rs` (stroke smoothing), `cursor.rs` (the hover outline) and `frame.rs` are platform-free
  and tested natively; `web.rs` is the browser glue.
