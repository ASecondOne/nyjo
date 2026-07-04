# Nyjo Roadmap

`nyjo` should become an advanced Rojo-style sync tool for Linux with:

- A real local project format that feels natural to edit
- A Roblox-aware parser instead of a plain folder walker
- A Studio-side snapshot/parser so sync works in both directions
- Incremental sync instead of full rebuilds for every change
- Conflict detection instead of silent overwrites
- Good CLI ergonomics and editor quality-of-life

The important design rule is:

> `nyjo` should have one canonical in-memory instance tree and two adapters:
> local filesystem <-> instance tree <-> Studio snapshot/plugin

That is the piece that makes true two-way sync possible.

## Product Goal

From a user point of view, the end state should feel like this:

1. Run `nyjo init`
2. Get a full Roblox-shaped local project structure
3. Add or edit files locally using meaningful names and custom extensions
4. Push those changes into Studio quickly
5. Notice Studio-side edits too
6. Pull supported Studio changes back to disk
7. Detect and resolve conflicts intentionally when both sides changed

## Core Principles

- Local-first editing should feel fast and obvious
- The parser should be deterministic and debuggable
- Unsupported instance types should degrade gracefully, not corrupt data
- Two-way sync must be explicit about what round-trips safely
- Conflict detection is more important than pretending everything merges cleanly
- CLI and plugin should expose state clearly instead of hiding sync decisions

## Current Status

Implemented right now:

- [x] `nyjo init`
- [x] `nyjo install`
- [x] `nyjo tree`
- [x] `nyjo doctor`
- [x] `nyjo serve`
- [x] Local folder scanning into an in-memory instance tree
- [x] Roblox-aware extension mapping for the current supported file types
- [x] Directory-backed script containers for scripts that also own children
- [x] Compact subtree parsing for `.instance.json`, `.model.json`, and `.service.json`
- [x] `.nyjoignore` support
- [x] Embedded header metadata for fixed-shape file-backed nodes plus typed directory self files
- [x] Legacy `.meta.json` overrides for class name, properties, attributes, and tags
- [x] Local-only HTTP API with basic tree/create/update/delete endpoints
- [x] `GET /api/health` plus `GET /api/info` for version/capability reporting
- [x] Browser dashboard served by the Rust process for control, status, and logs
- [x] Studio plugin install into Vinegar local plugin folders
- [x] Studio preview and push from the plugin
- [x] Studio plugin bridge polling for browser-queued commands
- [x] Explicit Studio session targeting so multiple open places do not steal each other's commands
- [x] First-pass Studio snapshot upload and Studio-to-local sync
- [x] Studio pull preview/apply/force flow with changed-path reporting
- [x] Pull-time local conflict blocking for files, directories, and metadata
- [x] Studio pull compaction into `.instance.json` fallback files for generic instance trees
- [x] Typed fixed-shape files and typed container directories for common UI, value, and visual instances
- [x] Typed property translation for common UI/layout/value/basepart cases
- [x] Extra property translation for buttons plus `Decal` and `Texture`
- [x] First-pass `UIShadow` typed file/directory support plus Studio property translation
- [x] `UICorner` individual corner radii round-tripping for the new UI capabilities surface
- [x] Header-backed `init.*` metadata files for typed container directories plus `.nyjo` generic directory headers
- [x] Automatic local project backups before apply/force pull plus `nyjo restore`
- [x] Automatic Studio tree backups before push plus a Studio-side restore button
- [x] Managed-child tracking so Nyjo only deletes children it actually synced earlier
- [x] Round-trip coverage for `CFrame`, `MeshPart`, `SpecialMesh`, and `SurfaceAppearance` map-critical properties
- [x] Safer refusal path for classes like `UnionOperation` and `Terrain` that Nyjo cannot recreate faithfully yet
- [x] Zsh install/helper scripts for real `nyjo` CLI usage

Still missing:

- [ ] Watch mode
- [ ] Incremental diff engine
- [ ] Smarter Studio snapshot filtering and reconciliation
- [ ] Full two-sided conflict detection and resolution
- [ ] Lossless support for binary/CSG-heavy classes that cannot be reconstructed from text snapshots alone
- [ ] Robust config and richer project bootstrap

## Safety and Restore

The current safety posture should now be:

- Every Studio push creates a server-side Studio backup first.
- The Studio widget can restore the most recent remembered Studio backup for the current place.
- Every apply/force pull creates a hidden local backup under `.nyjo-backups/local/`.
- `nyjo restore --root <project> [--backup-id <id>]` restores the newest local backup by default.
- Nyjo now refuses to create known lossy classes like `UnionOperation` and `Terrain` from plain local data instead of silently rebuilding broken defaults.

That still does not mean every Roblox class is magically lossless. For classes whose real data lives in binary engine state or CSG internals, the correct behavior is to preserve backups and fail loudly until Nyjo has a true representation for them.

## Local Project Format

Primary conventions:

- CLI name: `nyjo`
- Ignore file: `.nyjoignore`
- Preferred metadata storage: embedded in the file itself when the file type supports it
- Generic directory header file: `.nyjo`
- Legacy metadata file: `.meta.json`
- Local bind address: `127.0.0.1`
- Browser dashboard: `http://127.0.0.1:<port>/`

Current extension mapping:

- `.server.lua` -> `Script`
- `.client.lua` -> `LocalScript`
- `.lua` -> `ModuleScript`
- `.rf` -> `RemoteFunction`
- `.re` -> `RemoteEvent`
- `.bf` -> `BindableFunction`
- `.be` -> `BindableEvent`
- `.instance.json` -> generic compact subtree file, defaulting to `Folder` when `className` is omitted
- `.model.json` -> `Model`
- `.part` -> `Part` with embedded header metadata plus optional compact JSON children
- `.model` -> `Model` with embedded header metadata plus optional compact JSON children
- `.worldmodel` -> `WorldModel` with embedded header metadata plus optional compact JSON children
- `.screengui`, `.canvasgroup`, `.scrollingframe`, `.surfacegui`, `.billboardgui`
- `.frame`, `.textbutton`, `.textlabel`, `.textbox`, `.imagelabel`, `.imagebutton`
- `.uilistlayout`, `.uigridlayout`, `.uipadding`, `.uicorner`, `.uistroke`
- `.uishadow`
- `.texture`, `.decal`
- `.stringvalue`, `.numbervalue`, `.intvalue`, `.boolvalue`, `.color3value`, `.vector3value`
- `.folder` -> `Folder`
- `.service.json` -> compact top-level service subtree file

Embedded header convention:

- File-backed nodes can start with an inline Nyjo header instead of a sidecar metadata file.
- Header shape:
  - `--!nyjo`
  - `--HEADER`
  - JSON metadata lines prefixed with `--`
  - `--CONTENTS`
  - the real body after that marker
- Supported file-backed header targets:
  - scripts: `.server.lua`, `.client.lua`, `.lua`
  - structured/model files: `.part`, `.model`, `.worldmodel`
  - fixed-shape marker/value/object files: `.folder`, `.rf`, `.re`, `.bf`, `.be`
  - fixed-shape UI, value, visual, and modifier files like `.textbutton`, `.uicorner`, `.uishadow`, `.texture`, `.stringvalue`
- Fixed suffixes stay authoritative; headers and sidecar metadata can add properties, attributes, and tags but should not change a `.server.lua` into a different class.
- Compact `.instance.json`, `.model.json`, and `.service.json` files stay JSON-native.
- The parser still accepts legacy sidecar `.meta.json` files for backward compatibility, and generic unsuffixed directories can use `.nyjo` when there is no dedicated `init.*` self file shape.

Directory-backed init convention:

- `SomeScript.server.lua` still works for a flat `Script`
- `SomeLocal.client.lua` still works for a flat `LocalScript`
- `SomeModule.lua` still works for a flat `ModuleScript`
- If a script also needs children, use a directory plus an init file:
  - `Main/init.server.lua`
  - child instances inside `Main/`
- Script container metadata now lives in the init file header rather than requiring `Main/.meta.json`.
- Equivalent init files:
  - `init.server.lua`
  - `init.client.lua`
  - `init.lua`
- Typed directory-backed nodes now prefer matching self files like `init.part`, `init.screengui`, `init.textbutton`, or `init.folder`.
- Generic unsuffixed directories without a dedicated self-file shape can use `.nyjo`.

Typed container directories:

- Any supported fixed-shape non-script node can also be represented as a directory when it needs children.
- Examples:
  - `Spawn.part/init.part` plus nested children like `Surface.texture`
  - `Hud.screengui/Play.textbutton/Corner.uicorner`
  - `Hud.screengui/Play.textbutton/Shadow.uishadow`
- Preferred typed-directory metadata now lives in matching `init.*` header files, while generic unsuffixed directories can fall back to `.nyjo`.
- Studio subtrees with repeated child names fall back to compact `.instance.json` so duplicate children are not collapsed into one local path.

Compact subtree convention:

- If a subtree would otherwise explode into many tiny folders and metadata files, define it in one JSON file instead.
- `Hud.instance.json` is the generic compact instance format.
- `StarterGui.service.json` is the compact top-level service format.
- Studio pull now uses `.instance.json` automatically as the generic fallback for non-script instance trees that do not have a more specific local file shape.
- These files support:
  - `className`
  - `properties`
  - `attributes`
  - `tags`
  - `source`
  - `children`
- `children` can be either:
  - an array of node objects with explicit `name`
  - an object map where the key becomes the child name
- If a compact child omits `className` and has no `source`, it defaults to `Folder`
- If a compact child includes `source`, it should declare `className` explicitly

Likely default scaffold created by `nyjo init`:

- `ReplicatedStorage/`
- `ReplicatedFirst/`
- `ServerScriptService/`
- `ServerStorage/`
- `StarterGui/`
- `StarterPlayer/StarterPlayerScripts/`
- `StarterPlayer/StarterCharacterScripts/`
- `Teams/`
- `TextChatService/`
- `Workspace/`
- `SoundService/`
- `Lighting/`

## Current Translation Coverage

Structural items currently translated:

- Top-level services: `Lighting`, `ReplicatedFirst`, `ReplicatedStorage`, `ServerScriptService`, `ServerStorage`, `SoundService`, `StarterGui`, `StarterPlayer`, `Teams`, `TextChatService`, `Workspace`
- Local script files: `Script`, `LocalScript`, `ModuleScript`
- Local marker files with embedded headers: `RemoteEvent`, `RemoteFunction`, `BindableEvent`, `BindableFunction`
- Structured instance files with embedded headers: `Part`, `Model`, `WorldModel`, `Folder`
- Typed fixed-shape files and directories for common UI, value, visual, and modifier instances including `UIShadow`
- Directory-backed init files including `init.server.lua`, `init.client.lua`, `init.lua`, and typed self files like `init.part` or `init.textbutton`
- Compact subtree files: `.instance.json`, `.model.json`, `.service.json`
- Generic compact fallback files via `.instance.json` for unsupported or intentionally compact instance subtrees
- Legacy directory-backed instances via `.meta.json` class overrides still parse for backward compatibility

Property/value shapes currently translated:

- JSON primitives: `boolean`, `number`, `string`
- Tagged typed values: `Color3`, `Vector2`, `Vector3`, `CFrame`, `UDim`, `UDim2`, `EnumItem`
- Attributes and tags when they fit the same supported value model

Common Studio property groups currently snapshotted and pushed back:

- `BasePart` including transform-oriented fields like `CFrame`
- `ValueBase`
- `ScreenGui`
- `GuiObject`
- `TextLabel`, `TextButton`, `TextBox`
- `ImageLabel`, `ImageButton`
- Button-specific state on `TextButton` and `ImageButton`
- `ScrollingFrame`
- `UIListLayout`
- `UIGridLayout`
- `UIPadding`
- `UICorner`, including individual corner radii when Studio exposes them
- `UIStroke`
- `UIShadow`
- `Decal`, `Texture`

Current Roblox Creator Hub docs still list UI shadows and individual `UICorner` radii behind Studio's **File > Beta Features > New UI Capabilities** toggle. Nyjo now preserves those values whenever the active Studio build exposes them.

## Good Next Adds

- [ ] Add first-class `UIGradient` local file/directory support and property round-tripping
- [ ] Expand `CanvasGroup`-specific and advanced `UIStroke` property coverage
- [ ] Surface clearer plugin warnings when a beta-gated Roblox class or property is unavailable in the current Studio build

Current limitation:

- Unsupported properties still fall back to structure plus metadata where possible; the tool does not claim full Roblox property coverage yet.

## Architecture Plan

### 1. Canonical Instance Tree

Goal: define the one internal data model that every sync path uses.

- [x] Create a central `InstanceNode`
- [x] Represent services, folders, scripts, remotes, metadata-backed nodes
- [ ] Add stable node identity beyond path alone
- [ ] Distinguish source-of-truth fields from derived fields
- [ ] Add hash/revision fields for later diffing
- [ ] Add enough metadata to support round-tripping where possible

Exit criteria:

- Both filesystem parsing and future Studio snapshots can map into the same structure
- A node can be compared, diffed, and reconciled without guessing from display name alone

### 2. Local Project Bootstrap

Goal: make `nyjo init` create a project layout that actually helps users start quickly.

- [x] Add `nyjo init`
- [x] Create a base Roblox-style folder scaffold
- [x] Create `.nyjoignore`
- [ ] Expand the default scaffold to include more commonly used services
- [x] Expand the default scaffold to include the top-level services already supported by parser/plugin
- [ ] Add starter sample files for common flows
- [ ] Add optional templates:
  - empty game
  - UI-heavy game
  - framework-style game
- [ ] Support importing a Studio snapshot into the initial local layout

Exit criteria:

- A new user can run `nyjo init` and begin adding meaningful files immediately
- `nyjo init` is a useful starting point even before the plugin is finished

### 3. Local Filesystem Parser

Goal: make the local parser truly Roblox-aware and suitable as one side of two-way sync.

- [x] Parse known file extensions into Roblox classes
- [x] Recognize top-level service folders
- [x] Recognize some special nested Roblox containers
- [x] Ignore hidden files, system noise, and `.nyjoignore` matches
- [x] Apply embedded file headers plus legacy `.meta.json` overrides
- [ ] Expand special-case handling for more Roblox container classes
- [ ] Add better parser diagnostics with exact path and fix suggestion
- [ ] Add parser warnings for ambiguous or risky layouts
- [ ] Define precedence rules for file shape vs metadata vs inferred defaults
- [ ] Add serialization rules for writing supported nodes back to disk
- [x] Add serialization rules for directory-backed scripts with children

Exit criteria:

- A realistic local project parses predictably
- The parser can also act as the basis for writing pulled changes back to disk later

### 4. Studio Snapshot Model

Goal: build the Studio-side equivalent of the local parser.

- [ ] Define the minimal snapshot format the plugin sends to Rust
- [x] Define a first-pass Studio snapshot payload for supported round-trip types
- [x] Include class name, properties, attributes, tags, and source where relevant
- [ ] Include explicit instance path where relevant
- [ ] Preserve enough identity to compare snapshots across sync cycles
- [ ] Decide the full syncability policy; a first safe subset is now implemented
- [ ] Normalize Studio snapshots into the same internal `InstanceNode` model
- [x] Accept Studio snapshot upload and write supported nodes back to disk
- [ ] Store the latest known Studio snapshot on the Rust side for diffing instead of immediate write only

Exit criteria:

- The Rust app can ingest a Studio snapshot and treat it like a first-class tree
- Local and Studio trees can be compared with the same diff logic

### 5. Diff Engine

Goal: compare local and Studio trees in a structured, explicit way.

- [ ] Define node matching rules:
  - stable id when available
  - otherwise path/name/class fallback
- [ ] Detect create, update, delete, move, and rename operations
- [ ] Detect source edits separately from property edits
- [ ] Detect metadata-only changes
- [ ] Produce a structured sync plan instead of ad hoc mutations
- [ ] Make diff results inspectable from the CLI

Exit criteria:

- The app can say exactly what changed and what it plans to do before applying it

### 6. Push Sync MVP

Goal: get a reliable local -> Studio sync working first.

- [ ] Implement a sync plan that turns local tree changes into Studio actions
- [ ] Create missing instances in Studio
- [ ] Update script source
- [ ] Update supported properties, attributes, and tags
- [ ] Delete removed instances safely
- [ ] Add a manual `nyjo push`
- [ ] Add dry-run output so a user can inspect the planned actions

Exit criteria:

- A local project can be pushed into Studio end-to-end
- Pushes are deterministic and explainable

### 7. Local Server and Plugin Transport

Goal: make the Rust process and Studio plugin communicate cleanly.

- [x] Start a localhost-only server
- [x] Bind only to `127.0.0.1`
- [x] Return structured JSON responses
- [x] Serve a local browser dashboard
- [x] Add `GET /api/tree`
- [x] Add basic create/update/delete endpoints
- [x] Add a first-pass pull sync-plan response inside `POST /api/sync-from-studio`
- [x] Add a snapshot upload endpoint for Studio -> Rust
- [x] Add health/version/capability endpoints
- [x] Add bridge heartbeat, polling, and command-result endpoints
- [ ] Add protocol versioning so plugin and CLI can evolve safely
- [ ] Add event streaming or polling strategy for incremental sync

Exit criteria:

- The transport layer is stable enough to power both push and pull workflows

### 8. Studio Plugin MVP

Goal: make the first Roblox Studio plugin that can participate in sync, not just fetch JSON.

- [x] Connect the plugin to the local Rust server
- [x] Fetch the local tree for preview
- [x] Materialize folders, scripts, remotes, and supported metadata in Studio
- [x] Collect a Studio snapshot and send it back to Rust
- [x] Poll the Rust server for queued browser commands
- [x] Show connection state
- [x] Show sync state
- [x] Show last error
- [x] Add a manual push button
- [x] Add a manual pull/snapshot button
- [x] Add preview/apply/force pull controls
- [x] Improve plugin output so changed paths are listed explicitly

Exit criteria:

- The plugin can push local changes into Studio and return enough Studio state for future pull support

### 9. Watch Mode and Incremental Local Sync

Goal: stop forcing users to rescan everything after each save.

- [ ] Watch the project folder for create/update/delete/rename events
- [ ] Debounce noisy save bursts
- [ ] Re-parse only affected subtrees where practical
- [ ] Recompute a sync plan incrementally
- [ ] Push incremental changes to the plugin by default
- [ ] Add `nyjo serve --watch`
- [ ] Add logs that explain why an update fired

Exit criteria:

- Normal editing feels live
- Save storms do not spam Studio or rebuild the world unnecessarily

### 10. Pull Support

Goal: let supported Studio-side changes become local files again.

- [x] Define and document the currently translated item/property subset
- [x] Convert Studio scripts back into `.server.lua`, `.client.lua`, or `.lua`
- [x] Convert remotes and bindables back into their local marker files
- [x] Write metadata files when properties or classes cannot be expressed by filename alone
- [x] Use directory-backed script containers when Studio scripts have children
- [ ] Support `nyjo pull`
- [x] Support preview mode before writing files
- [x] Support apply/force modes with explicit changed-path reporting
- [ ] Preserve user formatting and file placement where possible
- [ ] Avoid mirroring default Roblox service noise unless the user wants it

Exit criteria:

- Supported Studio changes can be written back to disk in a stable format

### 11. Conflict Detection and Resolution

Goal: avoid destructive sync behavior once both directions are active.

- [ ] Track last-synced revision or hash per node
- [ ] Detect local-only changes
- [ ] Detect Studio-only changes
- [ ] Detect true two-sided conflicts
- [x] Block destructive pull-time overwrites unless `force=true`
- [ ] Separate harmless merges from destructive conflicts
- [ ] Add `nyjo conflicts`
- [ ] Add `nyjo take-local <path>`
- [ ] Add `nyjo take-studio <path>`
- [ ] Add `nyjo diff <path>`
- [x] Surface pull-time conflicts in the plugin UI

Exit criteria:

- Two-sided edits are surfaced clearly
- The user can resolve known conflicts without hand-editing internal state

### 12. CLI and Debugging Workflow

Goal: make the tool comfortable to operate from a terminal during development.

- [x] Add `nyjo init`
- [x] Add `nyjo tree`
- [x] Add `nyjo serve`
- [x] Add `nyjo doctor`
- [ ] Add `nyjo push`
- [ ] Add `nyjo pull`
- [ ] Add `nyjo watch`
- [ ] Add `nyjo diff`
- [ ] Add `nyjo snapshot`
- [ ] Improve logs and progress output
- [ ] Improve error readability and recovery suggestions
- [x] Add zsh install/helper scripts for PATH and completion setup
- [ ] Add config file support for:
  - project root
  - port
  - ignore patterns
  - extension overrides
  - template choice

Exit criteria:

- A user can inspect, sync, debug, and recover from problems from the CLI alone

### 13. Quality-of-Life Features

Goal: make `nyjo` nicer than a bare-minimum sync utility.

- [ ] Auto-create missing services when safe
- [ ] Auto-generate common folders for remotes/shared code/UI
- [ ] Support custom extension mappings
- [ ] Support alternate project layouts
- [x] Export the parsed tree as JSON
- [ ] Export the sync plan as JSON
- [x] Add richer plugin UI feedback
- [ ] Add optional project templates
- [ ] Add a doctor check for suspicious project structure
- [ ] Add a command to migrate older project layouts

Exit criteria:

- Power-user features improve flow without making sync behavior magical or unclear

### 14. Editor Tooling

Goal: make the local format pleasant to edit.

- [ ] Add VS Code file associations for custom extensions
- [ ] Add icon mappings for:
  - `.rf`
  - `.re`
  - `.bf`
  - `.be`
  - `.server.lua`
  - `.client.lua`
- [ ] Add snippets for common file templates
- [ ] Add lightweight schema help for embedded headers and `.meta.json`
- [ ] Keep editor support optional and separate from runtime logic

Exit criteria:

- The project feels natural in an editor without coupling editor tooling to sync logic

### 15. Release Readiness

Goal: make the project understandable to outside users.

- [x] Write a README
- [x] Add installation instructions
- [ ] Add a sample project
- [ ] Add screenshots or demo gifs
- [x] Choose a license
- [x] Document limitations clearly
- [x] Explain what currently round-trips and what does not
- [x] Add a warning that `nyjo` is unofficial Roblox tooling

Exit criteria:

- Someone new can install, understand, and evaluate `nyjo` without reading the code first

## Recommended Build Order

If focus needs to stay narrow, the most sensible order is:

1. Finish the local project shape and parser quality
2. Expand `init` so the scaffold is genuinely useful
3. Define the Studio snapshot format
4. Build the diff engine
5. Ship push sync first
6. Add watch mode and incremental sync
7. Add pull support
8. Add real conflict handling
9. Add QoL and editor polish

That order keeps the hard architectural work early and postpones polish until the core sync story is solid.

## Acceptance Scenarios

- [ ] `nyjo init` creates a useful Roblox-style project tree
- [x] Sample and unit-test coverage exists for supported extension parsing
- [x] Hidden files and `.nyjoignore` patterns are skipped correctly
- [x] Embedded headers and `.meta.json` can override class, properties, attributes, and tags where supported
- [x] `GET /api/tree` returns the current parsed tree
- [ ] A Studio snapshot can be ingested and normalized into the internal model
- [x] Dashboard/plugin push can create, update, and delete supported instances in Studio
- [ ] Watch mode emits incremental updates without flooding
- [x] Studio pull preview reports changed paths before writing
- [x] Supported Studio changes can be written back to disk
- [x] A differing local file on pull produces an explicit conflict instead of silent overwrite

## Notes

- Unsupported Roblox instance types should be skipped, represented via metadata, or marked unsupported, but should not crash the parser
- Not every Roblox property should sync in v1; only explicitly supported ones should be round-tripped
- Two-way sync should launch with a narrow safe subset instead of pretending every Studio object serializes cleanly
- The plugin transport should be treated as internal but versioned
- Editor tooling should stay optional and should not block sync-engine progress
