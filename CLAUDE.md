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
- `WYVEN_SEED=...` → seed if `WYVEN_WORLD` creates a new world (number, hex, or text)
- `WYVEN_CLIENT_ID=...` → override the profile identity (run two clients from one dir)
- `WYVEN_USERNAME=...` / `WYVEN_PASSWORD=...` → sign in at boot. A boot plan never
  shows the login screen; without these it runs **offline**, which plays
  singleplayer but cannot join or be joined (no ticket to present, no keys to
  verify with).
- `WYVEN_AUTH_URL=...` → the auth server (default `http://127.0.0.1:8080`). Bring
  one up from the sibling `wcauthserver` repo with `make up`.
- `WYVEN_DEBUG_SPAWN=cow,zombie,...` → spawn the named mobs next to the player at
  boot (singleplayer/host), without waiting on the spawner. `WYVEN_DEBUG_SPAWN="vine
  sword"` places the file-loaded model prop (it has no `spawning.toml` entry, so
  it never appears on its own).

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

**Building needs read access to a private repo.** `wcauth-ticket` — the join-ticket
contract — is a git dependency on the **private**
[gustaavik/wcauthserver](https://github.com/gustaavik/wcauthserver), pinned to
`branch = "main"` with the exact commit recorded in `Cargo.lock`. `cargo fetch`
therefore needs a GitHub credential with `repo` scope; `.cargo/config.toml` sets
`net.git-fetch-with-cli = true` so the fetch goes through the system `git` and
picks up the `gh`/osxkeychain helper. Advance the pin deliberately with
`cargo update -p wcauth-ticket`. While co-editing the ticket crate itself,
uncomment the `[patch."https://github.com/gustaavik/wcauthserver.git"]` block in
`.cargo/config.toml` to resolve it from a sibling `wcauthserver/` checkout instead
— but leave the `Cargo.lock` churn that patch causes uncommitted.

## Architecture

Domain modules with **one-directional dependencies** (do not introduce cycles):

```
core      ← everything        coordinate/voxel types, AABB/Ray/Frustum, timing
render    ← core              Vulkan: context, pipelines, mesh upload, camera, atlas, tile registry
model     ← core, render      .gltf/.bbmodel files → ModelMesh + its own texture (pure, no GPU)
world     ← core, render, model   blocks, chunks, generation, meshing, raycast, loader
inventory ← core, world, model  item/stack/inventory data model (no rendering)
entity    ← core, render, inventory, model   player, swept-AABB physics, humanoid/quadruped models, dropped items, mobs (brain/spawning/projectiles)
content   ← render, world, inventory, entity, model   GameContent: registries loaded from assets/*.toml
input     ← core, config, entity   winit events → frame-coherent input
ui        ← inventory, egui   HUD + inventory egui views
auth      ← core              accounts: login client, session, join-ticket verify
net       ← core, auth        renet host/client, protocol, remote-player interp
chat      ← core, net         message log, commands (one per file), ops.toml authorization
save      ← core, world, inventory, entity   world/player persistence (saves/ dir)
state     ← all of the above  game-state machine
boot      ← core, net, save   pure env → BootPlan (no window/GPU)
app       ← state, boot, content, render   window + event loop (owns everything)
```

Key rule: **`render` never depends on `world`.** The active game state builds plain
`render::CpuMesh` data and hands the renderer a `SceneFrame` (camera + mesh
references). This keeps the GPU layer decoupled from gameplay.

All geometry — voxels, box models and file-loaded models alike — is `CpuMesh` of
`ChunkVertex`, `TriangleList` with culling off and an alpha-test `discard`. What
differs is only the texture bound as descriptor set 0, which is what splits
`SceneFrame` into its lists:

- `opaque` / `transparent` sample the shared **16px atlas** (`render::texture`) —
  the older block path, entity skins, armor and mob sheets. One bind per pass.
- `array_opaque` / `array_transparent` sample the **block texture array**
  (`render::block_textures`): one 256×256 layer per texture a Blockbench-authored
  block names, chosen per vertex by `ChunkVertex::layer`, mipmapped with nearest
  magnification. Also one bind per pass, however many block types are on screen.
  Drawn by the `voxel_array` pipeline, which shares `voxel.vert` with `voxel` and
  differs only in its fragment shader.
- `textured` carries meshes that bring their own `render::Texture` and rebind set
  0 **per draw** — file-loaded `.gltf`/`.bbmodel` models.

There is still **no model matrix** — every transform is baked on the CPU. Chunks
straddle all three: a `block_model` block goes into the chunk's own array buffers,
while a `[block.model]` block (ground cover) is baked into its cell and grouped by
`ModelId` into `ChunkMeshOutput::models`, which `SceneCache` uploads per chunk and
feeds to `SceneFrame::textured`.

The atlas/array split is **temporary**. Blocks are migrating to Blockbench models
one at a time; when the last one moves over, the atlas keeps only the entity
sheets and cracks, and `voxel` / `voxel_array` collapse back into one pipeline.

Its mirror: **only `state::ingame_state::view` touches `RenderContext`.** Chunk
streaming, mob AI, fluids and interaction are plain logic; `InGameState::refresh_view`
is the single per-frame seam that turns their results into GPU meshes. That is why
those systems are testable without a Vulkan device.

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
- **Ports & adapters** (at I/O boundaries only — never in per-frame/per-voxel hot
  paths): `content::ContentSource`, `save::WorldRepository`, `state::session::Session`,
  `boot::Environment`. Each has a real impl, a null/embedded impl, and a test double,
  which is what lets content loading, saving, session logic and startup be tested
  without a filesystem, socket, or GPU. `ContentSource` reads *bytes*, with text
  derived from them, because model files carry PNGs and vertex buffers.
  `chat::CommandContext` is a port for a different reason — not I/O, but to invert
  a dependency: commands are policy and live in `chat`, but they act on registries
  and inventories owned by `state`, which already depends on `chat`. Real impl
  `SessionContext` in `state::ingame_state::chat`, double `chat::FakeContext`.
- **Chat commands** — `chat::command::ChatCommand`, one impl per file, found
  through the `COMMANDS` registry, exactly like `ModelLoader`. There is no
  `Command` enum and no `match` over command kinds: both would need editing for
  every addition. Each command parses its own arguments and phrases its own
  messages; the dispatcher only resolves the name and checks `permission()`.
- **File-loaded models** — `model::ModelLoader` is one impl per format (`.gltf`,
  `.bbmodel`), all normalising to the same `ModelMesh` (Y-up, right-handed, one
  block = 1.0, top-left UVs), so callers cannot tell them apart. Both shipped
  exports of `assets/models/vine_sword` describe the same object, and a test
  asserts the two loaders agree vertex-for-vertex — that is what pins the
  bbmodel face-corner order, UV-rotation direction and 1/16 scale.
- **Registry** — `world::BlockRegistry`, `inventory::ItemRegistry`,
  `entity::EntityRegistry`, `render::TileRegistry` (texture name → atlas tile),
  `render::BlockTextureSet` (texture path → block-texture-array layer).
- **Strategy** — `world::generation::WorldGenerator` (default `NoiseGenerator`).
- **Producer/consumer** — `world::loader::ChunkLoader` (crossbeam worker pool).
- **Command/message** — `net::protocol` (`ClientMessage` / `ServerMessage`).

### Where to make common changes
| Task                    | Location                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------ |
| Add a block type        | Model it in Blockbench (**Java Block/Item** format, per-face UV, project texture size 256), export *Block/Item Model* to `assets/blocks/<name>.json` with its textures as separate **256×256** PNGs in `assets/textures/`, then one `[[block]]` in `assets/blocks.toml` with `block_model = "assets/blocks/<name>.json"`. A full cube is `from [0,0,0]` → `to [16,16,16]`; set `cullface` on every outward face or it draws all six even when buried; set `tintindex` only on faces that should take the biome colour. Parsing is `model::blockjson`, baking `world::blockmodel`, layers `render::block_textures`. The older `textures = "<name>"` atlas path still works for the blocks not yet re-authored |
| Biome tint (grass colour) | `tint = [r, g, b]` per biome in `assets/worldgen.toml`, answered by `WorldGenerator::biome_tint` and multiplied into the faces a model marked `tintindex`. Greyscale art (`grass_top`, `grass_block_side_overlay`) is what makes one texture serve every climate |
| A non-cube block (plant, prop) | `[block.model]` in `assets/blocks.toml` — same `path`/`scale`/`offset`/`rotation` spelling as `[item.model]`, plus `random_yaw`. The block then emits **no** cube faces (`textures` becomes optional) and is baked into its cell by `world::meshing::culled`. Give it `solid = false` to walk through: `World::is_solid` is collision only, `is_targetable` is what the crosshair uses, and `is_replaceable` decides whether placing swallows it. Add a matching `[item.model]` on the same path so the drop, the hand and the icon agree — the registry memoises by path, so both share one `ModelId` |
| Block hitbox (crosshair, outline, cracks) | Not authored — `content::placed_bounds` measures the placed model and `world::block::model_hitbox` turns it into a square, centred, cell-clamped box on `BlockModel::hitbox`, so it can never drift from what is drawn. The raycast predicate returns `world::Target::{Cell,Box}`; a `Box` the ray misses does **not** stop the march. `InGameState::{target_at,hitbox_at}` are the single source for targeting *and* both overlays. Mob line-of-sight deliberately stays `is_solid` + `Target::Cell` — a flower must not hide you |
| Add an item / tool / food / armor | `assets/items.toml` (`[item.tool]` with `harvests`/`dig_speed`/`durability` and optional `damage`, `[item.food]`, `[item.armor]` with `slot`/`defense`/`durability`, `[item.model]` with `path`/`scale`/`offset`/`rotation`); starter kit in the same file |
| Tool tiers / melee damage | Tiers are data only — `dig_speed` + `durability` (+ `damage` on swords and axes) in `assets/items.toml`. There is deliberately **no** harvest-level gate: `harvests` decides *what* a tool is for, never *whether* a block drops. A tool without `damage` swings for `mobs::PLAYER_ATTACK_DAMAGE` (the fist); the local swing resolves in `InGameState::melee_damage`, a client's in `client_melee_damage`, which reads the inventory that client last reported |
| Load a 3D model from a file | drop a `.gltf` or `.bbmodel` in `assets/models/`, then point at it: `[entity.visual] kind = "model"` in `assets/entities.toml`, or `[item.model]` in `assets/items.toml`. Parsing is `model::{gltf,bbmodel}` behind the `ModelLoader` trait (a new format = a new impl + one line in `ModelRegistry::LOADERS`); placement math in `model::mesh`; GPU textures uploaded lazily in `state::ingame_state::view`. Exports disagree on which plane a flat object lies in (the tiered tools are flat in XY, `vine_sword` in YZ), so `ModelSpec::rotation` turns a model about its own axes — applied after `offset` re-centres it |
| Armor (slots, defense, wear, render) | data in `assets/items.toml` `[item.armor]`; slots 36..42 in `inventory::inventory`; defense math in `entity::player::damage`; equip gate + wear in `state::ingame_state`; worn-model shells + cape in `entity::model::build_mesh_armored`; procedural sheets in `render::armor`; net via `ServerMessage::PlayerEquipment` |
| Item icons              | `ItemIcon` is computed in `content`: `Cube` (from block faces), `Flat` (painters in `render::tiles::paint_named`, PNG-overridable), or `Model` for items with `[item.model]`. Drawn by `ui::icon::draw_item_icon`; the atlas and the 3D icon sheet are registered with egui in `app`. A `block_model` block's cube faces are 16px stand-ins **downsampled from its own 256px art** (`content::derive_face_tiles`), so the icon and the dropped-item cube keep working unchanged — read them through `GameContent::face_textures`, never `Block::textures` |
| 3D item icons           | `render::icons` (cell layout, framing transform, ortho camera) + `Renderer::draw_icons`; the sheet is rendered **once** at startup by `app::build_icon_sheet`, one cell per `ModelId`. Tune presentation with `ICON_YAW`/`ICON_PITCH`/`ICON_ROLL`/`FILL` in `render::icons` |
| Live player preview     | offscreen pass `render::Renderer::draw_model` + `PreviewFrame`; mesh/camera in `state::ingame_state::{update_preview_mesh,preview_frame}`; image + egui `TextureId` in `app` (runs *before* the world pass) |
| Block drop rules        | `drops = ...` on the block in `assets/blocks.toml` (`"self"`, `"none"`, `{ requires_tool }`, `{ item, count }`) |
| Entity tuning / new kind | `assets/entities.toml` (physics/movement/vitals/item/mob components); a new *behavior* = one new component in `entity::kind` + its code hook |
| Add / tune a mob        | `assets/entities.toml` (`[entity.mob]`: health, speeds, `behavior`, `knockback_resistance`, `[entity.mob.ranged]`, `drops`; `[entity.visual]` humanoid `skin=`/`arms_forward` or quadruped) + a `[[spawn]]` entry in `assets/spawning.toml`; skin painter in `render::mobskin` (PNG override `assets/textures/mob_<name>.png`) |
| Mob AI behavior         | `entity::brain` (pure state machine: Idle/Wander/Chase/Flee, perception → intent); body/physics in `entity::mob`; state-layer tick/perception/combat in `state::ingame_state::mobs`. Disposition is the `entity::kind::Behavior` enum (`passive`/`hostile`/`inert`) — a new disposition is a variant plus its arm in `MobBrain::think`, never a new boolean |
| A static prop / statue  | an `[[entity]]` with `[entity.mob] behavior = "inert"` and no `spawning.toml` entry. `knockback_resistance` is the separate axis: `1.0` bolts it down, `0.0` lets a hit send it flying |
| Mob spawning rules      | `assets/spawning.toml` (caps, ring distances, weights, groups, night rules — strict: unknown entity rejects the file); planner in `entity::spawning` (pure, seeded); world sampling in `state::ingame_state::mobs::update_spawning` |
| Projectiles             | `entity::projectile` (ballistic `Arrow`); launch tuning in `[entity.mob.ranged]`; ticked in `state::ingame_state::mobs::update_arrows` |
| Change terrain          | `assets/worldgen.toml` (blocks, ores, sea level, biome surfaces — ⚠ alters existing worlds); noise/climate/mesas stay in `world::generation::{noise,biome,generator}` |
| Trees/boulders/features | shapes+chances in `assets/worldgen.toml`; canopy strategies in `world::generation::features` (jittered-grid anchors) |
| Ground cover / scatter  | per-biome `plants = [...]` + `plant_chance_per_mille` in `assets/worldgen.toml`; placement in `world::generation::features::try_plant`, which runs **after** trees and only into air so it can never punch a hole in a trunk |
| Meshing                 | `world::meshing::culled` (face culling; greedy is a TODO)                                  |
| Water / fluids          | `[block.fluid]` component in `assets/blocks.toml` (auto-registers flow blocks); sim in `world::fluid` (fluid-agnostic, ticked from `state::ingame_state`) |
| Player movement/physics | numbers in `assets/entities.toml`; formulas in `entity::player`, `entity::physics`         |
| Crafting recipes        | `assets/recipes.toml` (data); logic in `inventory::crafting`; panel in `ui::inventory`     |
| A new screen            | implement `state::GameState`, push/replace via `Transition`                                |
| HUD / inventory UI      | `ui::hud`, `ui::inventory`                                                                 |
| Add a chat command      | **one new file in `src/chat/command/` + one entry in `COMMANDS`** — nothing else changes (the `ModelLoader::LOADERS` pattern). Implement `ChatCommand` (`name`/`usage`/`permission`/`run`); the command parses its own arguments and phrases its own messages. It reaches the world only through the `CommandContext` port, so it never sees a `PlayerId` and works identically for the local player and a remote client. Test it against `chat::FakeContext` with no world, socket or GPU. **Caveat:** a command needing a capability the port doesn't expose yet also grows `CommandContext` + its two impls — and if that capability must reach a *client*, a `ServerMessage` too (`GrantItems`, `Teleport`) |
| Authorize a player      | `ops.toml` in the working directory (`ops = [{ id = "<account uuid>", name = "..." }]`), parsed by `chat::ops`. Keyed by the **account id from the verified join ticket**, never by anything a client asserts. The host/singleplayer player is always an op; only the authority loads the file |
| Chat log / input bar    | `chat::{log,composer}` (pure state), drawn by `ui::chat::draw_chat`; keys `chat`/`chat_command` in `config::Keybinds` (T and /) |
| Networking              | `net::{server,client,protocol}` transport; role behind `state::session::Session` (Singleplayer/Host/Client + `FakeSession`); message application in `state::ingame_state::net` |
| Accounts / login        | `auth::{client,session,account,keys,verifier}`. `AuthClient` is a port (`HttpAuthClient` via ureq / `FakeAuthClient`); `LoginState` is the first screen; the session persists in `profile.toml`. Server lives in the private repo [gustaavik/wcauthserver](https://github.com/gustaavik/wcauthserver) |
| Who may join            | `net::server::Host::verify_join` checks the Ed25519 ticket a client puts in netcode `user_data` **before** a `PlayerId` exists — a failure is disconnected with no `Welcome`. Keys come from `authkeys.toml` via `auth::KeyCache`; **no keys means no joins**, never "everyone joins". Ticket format is `wcauth-ticket`, shared verbatim with the server — literally the same crate, pulled from the wcauthserver repo as a git dependency |
| Player nameplates       | `ui::nameplate` painted from `InGameState::draw_nameplates`; projection is `render::Camera::project`. egui composites after the world pass with no depth, so occlusion is an explicit `world::raycast` against `is_solid` |
| Saving / world files    | `save` module (formats, `saves/<slug>/`) behind `save::WorldRepository` (File/Null/InMemory); triggers in `state::ingame_state::save_world` |
| Pipelines / passes      | `render::pipeline`, `render::renderer`                                                     |
| GPU meshes / camera     | `state::ingame_state::view` (`SceneCache`) — the only holder of `RenderContext`             |
| Startup / dev env vars  | `boot::BootPlan::from_env` (pure, tested); effects in `app::initial_state`. A dev-boot plan skips the login screen via `app::boot_account` |
| Loading `assets/*.toml` | `content::source::ContentSource` (Fs/Embedded/Map) + one `load_or_builtin` helper           |
| Shaders                 | `assets/shaders/*.{vert,frag}`, declared in `render::shaders`. `voxel.vert` is shared by both chunk pipelines, so a new vertex attribute means editing it plus `render::vertex` and every `ChunkVertex { .. }` site |

## Conventions & gotchas

- **Edition 2024.** Let-chains (`if x && let Some(y) = z`) are used; `gen` is a
  reserved keyword — don't use it as an identifier.
- The module is named `core` — always reference it as `crate::core`; never write a
  bare `core::` path (it would resolve to the std `core` crate).
- The crate-level `#![allow(dead_code)]` is **gone** — `cargo build` is
  warning-free and `cargo clippy --all-targets` is clean. Keep them that way
  rather than re-adding a blanket allow.
- **Visual data must stay off `Block`.** `Block`'s `Debug` repr feeds
  `content::content_hash`, which gates multiplayer joins — so anything derived
  from a texture or a model (block models, baked geometry, the stand-in atlas
  tiles a `block_model` block gets) lives on `GameContent` indexed by `BlockId`,
  never on `Block`. Otherwise two peers whose grass is drawn slightly differently
  would be refused a shared world. If a `content_hash` test starts failing after
  a visual change, that is the invariant breaking, not the test being stale.
- **Block textures are exactly 256×256.** An array image has one extent for every
  layer, so this is an equality, not a maximum: a differently sized PNG warns and
  renders magenta. The 16px atlas is the opposite — it hard-rejects anything that
  *isn't* 16×16. Two texture systems, two fixed sizes, until the atlas retires.
- **Vulkan correctness signal:** `vulkano`'s safe command-buffer/pipeline API
  validates state and *panics* on misuse (this is what catches bad pipelines even
  without Vulkan validation layers). A clean multi-frame run is strong evidence the
  GPU code is correct.
- **Screenshots don't work** in headless/sandboxed shells here (`screencapture`
  returns "could not create image from display"). Verify rendering by running the
  app (it renders on a real display) or by trusting vulkano validation + a stable
  run.
- **Multiplayer testing:** launch two processes with `WYVEN_HOST=1` and
  `WYVEN_JOIN=127.0.0.1:25565`; the client logs `connected; world seed ... player id ...`
  on a successful handshake.
- **Commands run on the authority, never on the client.** A client sends its
  raw chat line (command or not) as `ClientMessage::Chat`; the host parses it,
  checks `ops.toml`, and answers with `ServerMessage::Chat` / `GrantItems`. There
  is deliberately no client-side execution path to skip, which is what makes the
  ops list an actual permission rather than a suggestion. `ops.toml` is
  CWD-relative and gitignored like `profile.toml`, keyed by the **account uuid
  from a verified join ticket** (player ids are per-session and can't carry a
  permission). It used to be keyed by the client's own `profile.toml` id, which
  the client asserted for itself — so anyone who learned an op's number became
  that op via `WYVEN_CLIENT_ID`. A player with no verified account is never an op.
- **Identity comes from the auth server, not the machine.** A client presents an
  Ed25519 join ticket in netcode's `user_data`; the host verifies it offline
  against `authkeys.toml` and refuses anyone it cannot verify — *before* a
  `PlayerId` is assigned, so a rejected peer never sees a `Welcome`. The netcode
  `u64` is derived from the account uuid, so a world save follows the player
  rather than the machine, and the host checks that the id a client connects
  with matches its ticket. Run the auth server from the `wcauthserver` repo
  (`make up`); point the game at it with `WYVEN_AUTH_URL`. The game does not
  reimplement the ticket format — it compiles the server's own `wcauth-ticket`
  crate, fetched from that repo (see the toolchain prerequisites above).
- **Typing in chat is safe by construction.** `app::window_event` gives egui
  every event first and only forwards what egui didn't consume, so a focused
  `TextEdit` means gameplay keys never reach `InputState`. The `!typing` guards
  in `ingame_state::frame` only cover the frame between opening the bar and the
  widget taking focus.
- **Saves are name-based, not id-based.** `saves/<slug>/` stores blocks/items by
  registry *name* (numeric ids are insertion-order indices and shift across code
  changes). `saves/` and `profile.toml` are CWD-relative (like `assets/`) and
  gitignored. Worlds regenerate terrain from the seed; only the edit overlay,
  players, mobs (`mobs.dat`, kind-by-name, fail-soft), and metadata are
  persisted — so terrain-generator changes alter existing worlds' unedited
  terrain (edits still replay at their coordinates). Dropped items and arrows
  are never saved.
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
