# CLAUDE.md

Guidance for Claude Code (and other AI agents) working in this repository.

## What this is

**Wyvencraft** — a Minecraft-style voxel game in Rust using **Vulkan via
`vulkano` 0.35** (safe Vulkan), `winit` 0.30, `egui` 0.31 (through
`egui_winit_vulkano`), and `renet` 2.0 for multiplayer. Single binary crate,
edition 2024. See [README.md](README.md) for the player-facing overview.

## Commands

```sh
cargo build            # build
cargo run              # build + run (opens a window)
cargo test             # unit tests (pure logic: protocol round-trip, etc.)
cargo clippy           # lint
cargo fmt              # format
```

Run with logging: `RUST_LOG=info,wyvencraft=debug cargo run`.

**Dev boot env vars** (skip the menus — invaluable for headless testing):
- `WYVEN_BOOT_INGAME=1` → singleplayer world directly
- `WYVEN_HOST=1` → host a session (port 25565)
- `WYVEN_JOIN=127.0.0.1:25565` → join a session
- `WYVEN_WORLD=name` → load-or-create this named world under `saves/` (combines
  with BOOT_INGAME/HOST). Without it, boot worlds are ephemeral — never saved.
- `WYVEN_SEED=…` → seed if `WYVEN_WORLD` creates a new world (number, hex, or text)
- `WYVEN_CLIENT_ID=…` → override the profile identity (run two clients from one dir)

## Toolchain prerequisites (important — non-obvious)

This needs a native Vulkan toolchain. On this macOS machine it was installed via
Homebrew: `molten-vk vulkan-loader vulkan-tools shaderc glslang cmake`.

[`.cargo/config.toml`](.cargo/config.toml) sets the required `[env]`:
- `SHADERC_LIB_DIR` — so `vulkano-shaders` links the prebuilt `libshaderc_combined.a`
  instead of rebuilding shaderc from source (huge build-time saving).
- `VK_ICD_FILENAMES` / `VK_DRIVER_FILES` — point the Vulkan loader at MoltenVK.

**MoltenVK is a "portability subset" device.** Two device features MUST stay
enabled in `VulkanoConfig.device_features` in [src/app.rs](src/app.rs) or the app
aborts at runtime:
- `dynamic_rendering` — the world pass uses dynamic rendering (no `VkRenderPass`).
- `image_view_format_swizzle` — egui uploads font textures with a swizzle.

## Architecture

Domain modules with **one-directional dependencies** (do not introduce cycles):

```
core      ← everything        coordinate/voxel types, AABB/Ray/Frustum, timing
render    ← core              Vulkan: context, pipelines, mesh upload, camera, atlas, tile registry
world     ← core, render      blocks, chunks, generation, meshing, raycast, loader
inventory ← core, world       item/stack/inventory data model (no rendering)
entity    ← core, render, inventory   player, swept-AABB physics, humanoid model, dropped items
content   ← render, world, inventory, entity   GameContent: registries loaded from assets/*.toml
input     ← core, config, entity   winit events → frame-coherent input
ui        ← inventory, egui   HUD + inventory egui views
net       ← core              renet host/client, protocol, remote-player interp
save      ← core, world, inventory, entity   world/player persistence (saves/ dir)
state     ← all of the above  game-state machine
app       ← state, content, render     window + event loop (owns everything)
```

Key rule: **`render` never depends on `world`.** The active game state builds plain
`render::CpuMesh` data and hands the renderer a `SceneFrame` (camera + mesh
references). This keeps the GPU layer decoupled from gameplay.

### Design patterns in use
- **Data-driven content** — game content lives in TOML under `assets/`
  (`blocks.toml`, `items.toml`, `entities.toml`, `worldgen.toml`,
  `recipes.toml`), loaded once at startup into `content::GameContent` and
  shared via `Arc`. Behavior is *components + code hooks*: data declares typed
  components (`drops`, `fluid`, `tool`, `food`, entity params); each component
  is implemented once in Rust and dispatched on — never on block/item identity.
  Every file has an embedded `include_str!` fallback and degrades fail-soft
  with a logged warning (worldgen is strict: any bad name rejects the file).
- **State pattern** — `state::GameState` trait + `StateStack` (menu → loading →
  in-game → pause overlay). `app` only drives the stack.
- **Registry** — `world::BlockRegistry`, `inventory::ItemRegistry`,
  `entity::EntityRegistry`, `render::TileRegistry` (texture name → atlas tile).
- **Strategy** — `world::generation::WorldGenerator` (default `NoiseGenerator`).
- **Producer/consumer** — `world::loader::ChunkLoader` (crossbeam worker pool).
- **Command/message** — `net::protocol` (`ClientMessage` / `ServerMessage`).

### Where to make common changes
| Task                    | Location                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------ |
| Add a block type        | `assets/blocks.toml` (pure data); texture = PNG in `assets/textures/<name>.png` or a painter in `render::tiles::paint_named` |
| Add an item / tool / food / armor | `assets/items.toml` (`[item.tool]` with `harvests`, `[item.food]`, `[item.armor]` with `slot`/`defense`/`durability`); starter kit in the same file |
| Armor (slots, defense, wear, render) | data in `assets/items.toml` `[item.armor]`; slots 36..42 in `inventory::inventory`; defense math in `entity::player::damage`; equip gate + wear in `state::ingame_state`; worn-model shells + cape in `entity::model::build_mesh_armored`; procedural sheets in `render::armor`; net via `ServerMessage::PlayerEquipment` |
| Item icons (2D)         | painters in `render::tiles::paint_named` (PNG-overridable); `ItemIcon` computed in `content` (cube from block faces vs flat tile); drawn by `ui::icon::draw_item_icon` (atlas registered with egui in `app`) |
| Live player preview     | offscreen pass `render::Renderer::draw_model` + `PreviewFrame`; mesh/camera in `state::ingame_state::{update_preview_mesh,preview_frame}`; image + egui `TextureId` in `app` (runs *before* the world pass) |
| Block drop rules        | `drops = ...` on the block in `assets/blocks.toml` (`"self"`, `"none"`, `{ requires_tool }`, `{ item, count }`) |
| Entity tuning / new kind | `assets/entities.toml` (physics/movement/vitals/item components); a new *behavior* = one new component in `entity::kind` + its code hook |
| Change terrain          | `assets/worldgen.toml` (blocks, ores, sea level, biome surfaces — ⚠ alters existing worlds); noise/climate/mesas stay in `world::generation::{noise,biome,generator}` |
| Trees/boulders/features | shapes+chances in `assets/worldgen.toml`; canopy strategies in `world::generation::features` (jittered-grid anchors) |
| Meshing                 | `world::meshing::culled` (face culling; greedy is a TODO)                                  |
| Water / fluids          | `[block.fluid]` component in `assets/blocks.toml` (auto-registers flow blocks); sim in `world::fluid` (fluid-agnostic, ticked from `state::ingame_state`) |
| Player movement/physics | numbers in `assets/entities.toml`; formulas in `entity::player`, `entity::physics`         |
| Crafting recipes        | `assets/recipes.toml` (data); logic in `inventory::crafting`; panel in `ui::inventory`     |
| A new screen            | implement `state::GameState`, push/replace via `Transition`                                |
| HUD / inventory UI      | `ui::hud`, `ui::inventory`                                                                 |
| Networking              | `net::{server,client,protocol}`; orchestration in `state::ingame_state` (`NetRole`)        |
| Saving / world files    | `save` module (formats, `saves/<slug>/`); triggers in `state::ingame_state::save_world`    |
| Pipelines / passes      | `render::pipeline`, `render::renderer`                                                     |
| Shaders                 | `assets/shaders/*.{vert,frag}`, declared in `render::shaders`                              |

## Conventions & gotchas

- **Edition 2024.** Let-chains (`if x && let Some(y) = z`) are used; `gen` is a
  reserved keyword — don't use it as an identifier.
- The module is named `core` — always reference it as `crate::core`; never write a
  bare `core::` path (it would resolve to the std `core` crate).
- `lib.rs` has a crate-level `#![allow(dead_code)]` left over from scaffolding.
  Prefer removing dead code over relying on it; drop the allow once the surface
  stabilises.
- **Vulkan correctness signal:** `vulkano`'s safe command-buffer/pipeline API
  validates state and *panics* on misuse (this is what catches bad pipelines even
  without Vulkan validation layers). A clean multi-frame run is strong evidence the
  GPU code is correct.
- **Screenshots don't work** in headless/sandboxed shells here (`screencapture`
  returns "could not create image from display"). Verify rendering by running the
  app (it renders on a real display) or by trusting vulkano validation + a stable
  run.
- **Multiplayer testing:** launch two processes with `WYVEN_HOST=1` and
  `WYVEN_JOIN=127.0.0.1:25565`; the client logs `connected; world seed … player id …`
  on a successful handshake.
- **Saves are name-based, not id-based.** `saves/<slug>/` stores blocks/items by
  registry *name* (numeric ids are insertion-order indices and shift across code
  changes). `saves/` and `profile.toml` are CWD-relative (like `assets/`) and
  gitignored. Worlds regenerate terrain from the seed; only the edit overlay,
  players, and metadata are persisted — so terrain-generator changes alter
  existing worlds' unedited terrain (edits still replay at their coordinates).
- **Save triggers:** `InGameState::on_exit` (fires on pause, quit-to-menu, app
  quit, and window close via `StateStack::shutdown`) + a 60 s autosave. Only
  singleplayer/host sessions of a *named* world hold a save handle; clients and
  `WYVEN_BOOT_INGAME`-without-`WYVEN_WORLD` boots never write.

## Verifying a change

1. `cargo build` / `cargo clippy` clean.
2. `cargo test` green.
3. Run it: `WYVEN_BOOT_INGAME=1 cargo run` (or host/join for net changes) and
   confirm no panic over several seconds. In a sandbox, launch in the background and
   poll the log rather than blocking on a foreground `sleep`.
