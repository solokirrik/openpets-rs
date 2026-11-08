# openpets-rs: agent notes

A minimal native rewrite of the OpenPets desktop pet (the Electron app lives in
`OpenPetsHQ/openpets`). Read `README.md` for usage; this file records
decisions and pitfalls that the code does not make obvious. Do not add
project layout or file listings here or in the README: they go stale
immediately.

## Scope (decided, do not expand without asking)

- macOS only. AppKit through `objc2` crates; no cross-platform UI layer.
- Exactly one pet on screen. Multiple simultaneous pets were explicitly rejected.
- No plugin platform, catalog or marketplace. Small features (the timer) are
  built in and stay minimal: no forms, no clickable overlays, nothing added to
  the menu bar beyond the pet icon.
- Compatible with the OpenPets pet format and the `@open-pets/mcp` tool
  contract (`openpets_status`, `openpets_react`, `openpets_say`: names,
  arguments, titles, annotations, `structuredContent`, status semantics).
- Few dependencies: no tokio, no rmcp, no image crates. MCP is hand-written
  JSON-RPC over stdio (the spec requires JSON-RPC); the app socket uses
  newline-delimited JSON because messages are tiny.

## Architecture rules

- Pure, testable logic lives outside `src/app/`; `src/app/` is the AppKit
  adapter and converts platform objects at its boundary.
- All UI state is `Rc<RefCell<App>>` on the main thread. AppKit callbacks go
  through `with_app` (uses `try_borrow_mut`); the timer block also uses
  `try_borrow_mut`. A panic inside an Objective-C callback aborts the process,
  so never `borrow_mut()`/`unwrap()` there. Build menus inside the borrow,
  run modal things (`popUpContextMenu`) after releasing it.
- `.unwrap()`/`.expect()` only for invariants that cannot fail at runtime
  (static regexes, startup content view/layer). Everything else logs and
  degrades (e.g. a broken pet falls back to the built-in one).
- Hooks (`openpets-rs hook`) must never disturb Claude Code: swallow every
  error, always exit 0, never launch the app. The MCP server may auto-launch
  the app, but only for `react`/`say`, not `status`.
- Speech is validated in one place (`reaction::validate_message`): 1–140
  chars, one line, no code/URLs/paths/secrets. Hooks only send canned lines.
- MCP stdout carries protocol only; all diagnostics go to stderr.

## macOS pitfalls already hit

- `CALayer.contentsRect` origin is bottom-left; spritesheet rows count from
  the top (see `PetWindow::show_frame`). Wrap layer changes in
  `CATransaction` with actions disabled or frames animate.
- The panel ignores mouse events except while the cursor is over the pet or a
  press is active (`update_click_through`); anything outside the pet rect
  (bubble, timer pill) is therefore never clickable.
- `NSImage::alloc()` needs `use objc2::AnyThread` (NSImage is not main-thread-only).
- CoreGraphics bitmap contexts must own their memory (null data pointer) and
  be copied row by row honouring `bytesPerRow`; wrapping a borrowed `Vec` was
  unsound because CG may keep referencing it.
- Menu bar icon: not a template image. Templates are tinted by the menu bar's
  own appearance (wallpaper-driven on recent macOS), but the requirement is to
  follow the system light/dark theme, polled once a second via
  `NSApp.effectiveAppearance`. Eyes/dark details are cut out as holes only
  when ≥60% of solid pixels are light; threshold after downscaling.
- Finder icon is set with `NSWorkspace.setIcon(forFile:)` on the bundle (as
  Ghostty does). It writes an `Icon\r` file into the bundle, which is fine for
  ad-hoc signing but would break notarization.
- Unix socket paths max out at 104 bytes: `OPENPETS_RS_HOME` must be short
  (scratch/temp dirs are often too long). Data dir is created with mode 0700.
- `Instant` does not advance during sleep on macOS; wall-clock deadlines (the
  timer) use `SystemTime`.
- An auto-hiding Dock has no API or notification. The app mirrors the
  Dock's rule from the cursor (bottom edge reveals, leaving the Dock area
  hides) and estimates its size from `com.apple.dock` (`tilesize`, pinned
  apps, recents, stacks) plus running unpinned apps. Only divider/padding
  widths are guesses. Exact geometry needs the Accessibility permission,
  which was judged not worth it. The pet steps up when any part of it
  overlaps the Dock (using its centre let the first icon cover it).
- Commands run from the agent shell are sandboxed: other apps' windows and
  the cursor position are invisible there (window lists come back empty,
  cursor reads 0). Probe such things from inside the app and log to stderr
  (`open --stderr <file> ~/Applications/OpenPets.app`).
- Memory: ~32 MB steady. ~11 MB is the decoded spritesheet, the rest is the
  AppKit baseline. Right after launch it reads ~45 MB because a transient
  full-size spritesheet copy leaves a freed "Malloc Large" block cached;
  `malloc_zone_pressure_relief` did not release it (tried, reverted).

## Dev loop

```sh
cargo fmt && cargo clippy --all-targets && cargo test
scripts/bundle.sh                       # -> target/bundle/OpenPets.app
pkill -x openpets-rs; rm -rf ~/Applications/OpenPets.app \
  && cp -R target/bundle/OpenPets.app ~/Applications/ && open ~/Applications/OpenPets.app
```

- Install to `~/Applications`, not `/Applications`: the Electron
  `openpets.app` clashes on a case-insensitive filesystem.
- Replacing the bundle kills the binary Claude Code's `openpets` MCP server
  runs; reconnect with `/mcp` afterwards.
- Check the app without the UI:
  `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"openpets_status","arguments":{}}}' | ~/Applications/OpenPets.app/Contents/MacOS/openpets-rs mcp`
- `screencapture` has no Screen Recording permission here; ask the user to
  look at visual changes.
- Do not edit `~/.claude.json` / `~/.claude/settings.json` directly; give the
  user the `claude mcp …` / `openpets-rs install-hooks` commands to run.

## Tests

Test observable behaviour and plausible regressions (pure modules), not
AppKit calls or internal constants. When fixing a bug, add the narrowest test
that fails without the fix.

## Assets and licensing

`assets/glitchcat.webp` is the built-in pet, compiled in with
`include_bytes!`. It comes from petdex (assets.petdex.dev) and its
redistribution licence is unconfirmed: check before publishing the repo or
a build. Hoodie Cat (from the OpenPets repo) was removed for the same reason.

## Commits

Commit or push only when asked. Never mention Claude, Claude Code or
Anthropic in commit messages or PRs (no co-author trailers or session links).
