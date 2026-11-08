# openpets-rs

A tiny macOS desktop pet written in Rust. The pet walks along the bottom of
your screen, and coding agents can make it react or talk through MCP.

It does one thing on purpose. There are no plugins, catalog, wardrobe or
settings window, and it supports macOS only.

## Run

```sh
cargo run                 # start the pet (menu bar 🐾, no Dock icon)
scripts/bundle.sh         # build target/bundle/OpenPets.app
```

- **Drag** the pet anywhere. When you let go, it drops back to the bottom of the screen.
- **Click** the pet to make it wave (and dismiss a `?`/`!` badge).
- With an auto-hiding Dock, the pet steps up onto the Dock while it is shown and drops back down when it hides.
- **Right-click** the pet, or use the **menu bar item**, to pick a pet, start a timer, pause walking, open the pets folder, or quit.
- **Timer** (5 min to 1 hour) shows a small countdown above the pet; when it ends the pet celebrates with a chime. It is not saved across restarts.

## Connect a coding agent (MCP)

The same binary is the MCP server, speaking stdio:

```sh
claude mcp add openpets -- /path/to/openpets-rs mcp
# or, after bundling:
claude mcp add openpets -- /path/to/OpenPets.app/Contents/MacOS/openpets-rs mcp
```

Tools (the OpenPets MCP package contract, plus a timer):

| Tool | Arguments |
|------|-----------|
| `openpets_status` | none |
| `openpets_react` | `reaction`: `idle`, `thinking`, `working`, `editing`, `running`, `testing`, `waiting`, `waving`, `success`, `error` or `celebrating` |
| `openpets_say` | `message` (max 140 chars, one line, no code, URLs, paths or secrets), plus an optional `reaction` |
| `openpets_start_timer` | `minutes` (whole number, 1–1440); replaces a running timer |

If the pet isn't running, the MCP server starts it on the first `react`,
`say` or timer call. `openpets_status` reports `appRunning: false` instead,
and isn't an error. Results include `structuredContent`, and tools carry titles and
read-only/idempotent annotations.

## Claude Code hooks

With MCP, the pet only reacts when the agent decides to call a tool. Hooks
make it react on its own while you work:

```sh
openpets-rs install-hooks     # adds entries to ~/.claude/settings.json ($CLAUDE_CONFIG_DIR is respected)
openpets-rs uninstall-hooks   # removes only those entries
```

| Claude Code event | Pet |
|-------------------|-----|
| `UserPromptSubmit` | thinking, held until the next reaction (or 10 min) |
| `PreToolUse` on Edit, Write or MultiEdit | editing |
| `PreToolUse` on a Bash test command (`cargo test`, `pnpm test`, …) | testing |
| `PermissionRequest` | waiting + "Approval needed" and a `!` badge (a question line and `?` for AskUserQuestion) |
| `Stop` | success |
| `StopFailure` | error + a short canned line |

Each reaction is throttled to once every 10 s, so a burst of edits doesn't
keep restarting the animation. Speech bubbles only ever show fixed canned
lines, never agent text. A hook never starts the app and always exits 0.
Set `OPENPETS_DEBUG=1` to see why an event was dropped.

The `?`/`!` badge stays above the pet until that Claude Code session's next
event (you answered, the tool ran, or the reply finished). Badges are tracked
per session, so another session carrying on doesn't clear one still waiting
on you.

## Pets

Pets live in `~/Library/Application Support/openpets-rs/pets/<id>/`, in the
OpenPets pet format:

```
pets/fox/
  pet.json          { "displayName": "Fox", "spritesheetPath": "spritesheet.webp" }
  spritesheet.webp  8 columns of 192×208 frames; rows are idle, run right, run left,
                    waving, jumping, failed, waiting, running, review
```

Glitchcat is bundled; a folder named `glitchcat` is ignored so it is not listed twice. Folders that are invalid are skipped and logged.

## Development

Set `OPENPETS_RS_HOME` to use another data directory. Keep that path short,
because Unix socket paths are limited to 104 bytes.

```sh
cargo test
cargo clippy --all-targets
```
