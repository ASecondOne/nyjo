# Nyjo

`nyjo` is a Rojo-inspired Roblox Studio local file syncer for Linux.

It scans a Linux-friendly project folder, turns it into a Roblox-style instance tree, serves that tree over a local HTTP server, and syncs supported changes between disk and Roblox Studio through a local plugin bridge.

`nyjo` is unofficial and is not affiliated with Roblox or Rojo.

## What It Does

- Parses a local project into one canonical in-memory instance tree
- Maps Roblox-aware file extensions like `.server.lua`, `.client.lua`, `.lua`, `.re`, `.rf`, `.bf`, and `.be`
- Supports compact subtree files like `.instance.json`, `.model.json`, and `.service.json`
- Supports embedded metadata headers inside file-backed nodes, so many cases no longer need sidecar `.meta.json` files
- Runs a local server and browser dashboard for status, logs, preview, and sync control
- Installs a Roblox Studio plugin bridge for Linux/Vinegar-based Studio setups
- Supports Studio push plus Studio pull with preview/apply/force modes

## How It Works

`nyjo` is built around one core idea:

`local filesystem <-> instance tree <-> Studio snapshot`

The local parser reads your project folder and converts it into an internal Roblox-shaped tree. The local server exposes that tree, the browser dashboard shows state and logs, and the Studio plugin bridge polls the server for commands and sends supported Studio snapshots back.

In practice, the flow looks like this:

1. You edit local files on Linux.
2. `nyjo` parses those files into an instance tree.
3. The local server serves that tree at `127.0.0.1`.
4. The browser dashboard lets you preview and control sync.
5. The Studio plugin bridge talks to the local server and moves supported data between Studio and disk.

## Requirements

To use `nyjo`, you currently need:

- Linux
- Rust and Cargo
- A web browser
- Roblox Studio on Linux if you want Studio sync
- A Studio setup that matches the Vinegar plugin layout if you want `nyjo install` to auto-copy the plugin
- Optional: `zsh` if you want the included shell install helpers and completions

Important notes:

- The local server binds to `127.0.0.1` by default.
- The plugin installer currently looks for Vinegar-style Studio roots under:
  - `~/.var/app/org.vinegarhq.Vinegar/data/vinegar/prefixes/studio/drive_c/users/*/AppData/Local/Roblox`
- If that path is not found, `nyjo install` still packages the plugin file into `dist/Nyjo.lua`.

## Install

### Option 1: Run With Cargo

```bash
cargo build --release
cargo run -- serve --root . --port 34872
```

You can also run the other commands the same way:

```bash
cargo run -- init --root my-game
cargo run -- tree --root my-game
cargo run -- doctor --root my-game
cargo run -- install --output-dir dist --port 34872
```

### Option 2: Install a Real `nyjo` Command

This repo includes Zsh helpers in `shell/`.

Install the binary and completions:

```zsh
./shell/install-nyjo.zsh
source ./shell/nyjo-env.zsh
```

Then you can use:

```zsh
nyjo init --root my-game
nyjo serve --root my-game --port 34872
nyjo install --output-dir dist --port 34872
```

For repo-local development without copying into `~/.local/bin`:

```zsh
source ./shell/use-nyjo-repo.zsh
cargo build --release
nyjo serve --root . --port 34872
```

## Quick Start

Create a project:

```bash
nyjo init --root my-game
cd my-game
```

Start the local server:

```bash
nyjo serve --root . --port 34872
```

Open the browser dashboard:

```text
http://127.0.0.1:34872/
```

Install the Studio plugin:

```bash
nyjo install --output-dir dist --port 34872
```

Then open Roblox Studio on Linux, load the Nyjo plugin, and use the browser dashboard or plugin bridge to preview/push/pull changes.

## Commands

- `nyjo init --root <path> [--force]`
  Creates a default Roblox-style project scaffold and `.nyjoignore`.
- `nyjo install --output-dir <path> [--port <port>]`
  Packages `Nyjo.lua` and tries to install it into Vinegar Studio plugin folders.
- `nyjo tree --root <path> [--json]`
  Prints the parsed instance tree.
- `nyjo serve --root <path> [--port <port>]`
  Starts the local sync server and browser dashboard.
- `nyjo doctor --root <path>`
  Prints a quick project summary and class counts.

## Local Project Format

Some of the main local shapes are:

- `Boot.server.lua` -> `Script`
- `Hud.client.lua` -> `LocalScript`
- `Shared.lua` -> `ModuleScript`
- `Ping.re` -> `RemoteEvent`
- `Ask.rf` -> `RemoteFunction`
- `Signal.be` -> `BindableEvent`
- `Invoke.bf` -> `BindableFunction`
- `Empty.folder` -> `Folder`
- `Spawn.part` -> `Part`
- `Rig.model` -> `Model`
- `Viewport.worldmodel` -> `WorldModel`
- `Hud.instance.json` -> compact generic subtree
- `StarterGui.service.json` -> compact top-level service subtree

### Embedded Headers

Many file-backed nodes can keep their metadata inside the file itself:

```lua
--!nyjo
--HEADER
--{
--  "attributes": {
--    "Stage": "Boot"
--  },
--  "tags": ["Startup"]
--}
--CONTENTS
print("hello from nyjo")
```

This header style is supported for:

- `.server.lua`, `.client.lua`, `.lua`
- `.part`, `.model`, `.worldmodel`
- `.folder`, `.rf`, `.re`, `.bf`, `.be`

The file suffix still stays authoritative. For example, `.server.lua` must remain a `Script`, `.re` must remain a `RemoteEvent`, and `.folder` must remain a `Folder`.

Legacy `.meta.json` files still parse for compatibility, especially for directory-backed containers.

If you need an arbitrary non-script class that does not have its own dedicated suffix, prefer `.instance.json` or a directory-backed node with legacy `.meta.json`.

### Directory-Backed Scripts

If a script also owns children, use a directory plus an init file:

```text
ServerScriptService/
  Main/
    init.server.lua
    EnemyHandler.lua
```

Metadata for that script container can live in the `init.server.lua` header.

## Browser Dashboard

When `nyjo serve` is running, it serves a local dashboard at:

```text
http://127.0.0.1:34872/
```

The dashboard is meant to be the main control surface. It shows:

- Server status
- Studio bridge status
- Connected Studio sessions and the currently selected target place
- Last command result
- Log output
- Buttons for preview/push/pull-style actions

If you have multiple Studio places open at the same time, Nyjo now requires you to select the exact target place in the dashboard before it will queue a Studio write or pull command.

## Studio Plugin Bridge

The Roblox Studio plugin is a local bridge, not the main source of truth. Its job is to:

- Connect Studio to the local `nyjo` server
- Poll for commands
- Send Studio-side snapshot data back
- Run supported preview/apply operations

Each open Studio window now gets its own bridge session id, so queued dashboard commands are delivered only to the selected place instead of whichever plugin window polls first.

The local files and local parser remain the core model.

## Current Limitations

- This is still an early tool and does not claim full Roblox property coverage
- Round-tripping is strongest for the currently supported file types and translated properties
- Conflict handling exists, but full two-sided reconciliation is still a work in progress
- Auto-install of the plugin currently targets Vinegar-style Linux Studio paths

For a more detailed project status and roadmap, see [TASK.md](TASK.md).

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
