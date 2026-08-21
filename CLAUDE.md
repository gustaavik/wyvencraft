# CLAUDE.md

Guidance for Claude Code (and other AI agents) working in this repository.

## What this is

**Wyvencraft** — a Minecraft-style voxel game in Rust using **Vulkan via
`vulkano` 0.35** (safe Vulkan), `winit` 0.30, `egui` 0.31 (through
`egui_winit_vulkano`), and `renet` 2.0 for multiplayer. Edition 2024. See
[README.md](README.md) for the player-facing overview.

**A cargo workspace: engine and game are separate crates.** The nine `wyven-*`
members under `crates/` are the engine and know nothing about grass, zombies or
survival mode; the root package `wyvencraft` is the game built on them. The
direction is enforced by cargo — no engine crate lists the game, so a violation
stops compiling rather than being caught in review.

## Commands

```sh
cargo build --workspace   # build engine + game
cargo run                 # build + run the game (opens a window)
cargo test --workspace    # unit tests (pure logic: protocol round-trip, etc.)
cargo clippy --workspace --all-targets
cargo fmt --all

cargo test -p wyven-voxel # one engine crate, no GPU and no game content
```

`cargo run` still works from the repo root: the root package *is* `wyvencraft`
as well as the workspace root, so `assets/`, `saves/`, `profile.toml`,
`ops.toml` and `authkeys.toml` stay CWD-relative exactly as before.

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
enabled in `VulkanoConfig.device_features` in
[crates/wyven-app/src/runner.rs](crates/wyven-app/src/runner.rs) or the app
aborts at runtime:
- `dynamic_rendering` — the world pass uses dynamic rendering (no `VkRenderPass`).
- `image_view_format_swizzle` — egui uploads font textures with a swizzle.

**Building the *game* needs read access to a private repo; the engine does not.**
`wcauth-ticket` is reachable only from `wyven-auth`, so
`cargo build -p wyven-core -p wyven-assets -p wyven-render -p wyven-model -p
wyven-voxel -p wyven-net -p wyven-input -p wyven-app` needs no credential at all.
`wcauth-ticket` — the join-ticket contract — is a git dependency on the **private**
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

### The engine crates (`crates/`)

Nine workspace members, **one-directional** and enforced by cargo — none of them
lists the game, so a violation stops compiling:

```
wyven-core     ← nothing            coordinate/voxel types, AABB/Ray/Frustum, RNG, timing
wyven-assets   ← nothing            AssetSource port (Fs/Embedded/Map), Rgba8, decode_png
wyven-render   ← core, assets       Vulkan: context, pipelines, mesh upload, camera, atlas, texture array
wyven-model    ← core, assets, render   .gltf/.bbmodel/block JSON → ModelMesh + its own texture
wyven-voxel    ← core, render, model    chunk store, loader pool, culled mesher, raycast, World
wyven-net      ← core               renet transport, generic over <Protocol, JoinVerifier>
wyven-input    ← core               winit events → frame-coherent InputState
wyven-auth     ← nothing            accounts, key cache, Ed25519 ticket verify (the ONLY wcauth-ticket user)
wyven-app      ← core, render, input    window, egui, event loop, screen stack
```

**None of them knows a block's name, a mob's behaviour, or what survival mode
means.** Meaning crosses into the engine through six traits the game implements:

| Trait | Declared by | Implemented by |
| --- | --- | --- |
| `TileSource` (+ `ReservedTiles`) | `wyven-render` | `art::WyvencraftArt` |
| `BlockCatalog` | `wyven-voxel` | `content::BlockAppearance` |
| `BlockProperties` | `wyven-voxel` | `world::BlockRegistry` |
| `WorldGenerator` | `wyven-voxel` | `world::NoiseGenerator` |
| `Protocol` / `JoinVerifier` | `wyven-net` | `net::WyvenProtocol` / `net::TicketJoin` |
| `Game` (+ `Screen`) | `wyven-app` | `state::Wyvencraft` (+ every screen) |

If you add engine code that needs to know something Wyvencraft-specific, that is
the signal to grow one of these traits rather than to add a dependency. **Adding
the game as a dependency of a `wyven-*` crate is always wrong.**

### The game modules (`src/`)

```
core      ← wyven-core        re-export + GameMode and DayCycle (rules, not primitives)
art       ← render            procedural tiles, player skin, armor and mob sheets
world     ← voxel             block table, worldgen, fluid spreading rules
inventory ← world             item/stack/inventory data model (no rendering)
entity    ← inventory, model  player, swept-AABB physics, humanoid/quadruped models, drops, mobs
content   ← all of it         GameContent: registries loaded from assets/*.toml
chat      ← net               message log, commands (one per file), ops.toml authorization
save      ← world, entity     world/player persistence (saves/ dir)
ui        ← inventory, egui   HUD + inventory egui views
net       ← wyven-net         the wire protocol, and who may join
config    ← wyven-input       settings, keybinds, and raw keys → MovementInput
boot      ← save, net         plan: pure env → BootPlan; start: plan → first screen
state     ← everything        the screens, and the Game impl that starts them
app       ← state             ~20 lines: name the game, hand it to wyven_app::run
```

Key rule, unchanged in spirit and now enforced by the crate graph: **`render`
never depends on `world`.** The active screen builds plain `CpuMesh` data and
hands the renderer a `SceneFrame` (camera + mesh references).

Its mirror: **only `state::ingame_state::view` touches `RenderContext`.** Chunk
streaming, mob AI, fluids and interaction are plain logic; `InGameState::refresh_view`
is the single per-frame seam that turns their results into GPU meshes. That is why
those systems are testable without a Vulkan device.

### Rendering

All geometry — voxels, box models and file-loaded models alike — is `CpuMesh` of
`ChunkVertex`, `TriangleList` with culling off and an alpha-test `discard`. What
differs is only the texture bound as descriptor set 0, which is what splits
`SceneFrame` into its lists:

- `opaque` / `transparent` sample the shared **16px atlas** (`wyven_render::texture`) —
  the older block path, entity skins, armor and mob sheets. One bind per pass.
- `array_opaque` / `array_transparent` sample the **block texture array**
  (`wyven_render::block_textures`): one 256×256 layer per texture a Blockbench-authored
  block names, chosen per vertex by `ChunkVertex::layer`, mipmapped with nearest
  magnification. Also one bind per pass, however many block types are on screen.
  Drawn by the `voxel_array` pipeline, which shares `voxel.vert` with `voxel` and
  differs only in its fragment shader. An **animated** texture takes one layer
  per frame, consecutively; `ChunkVertex::flags` carries the frame count and fps
  (`wyven_render::vertex::anim_flags`) and `voxel_array.frag` steps the layer from
  `pc.sun_dir.w`. Water is the only user so far, at 128 layers.
- `textured` carries meshes that bring their own `wyven_render::Texture` and rebind set
  0 **per draw** — file-loaded `.gltf`/`.bbmodel` models.

There is still **no model matrix** — every transform is baked on the CPU. Chunks
straddle all three: a `block_model` block goes into the chunk's own array buffers,
while a `[block.model]` block (ground cover) is baked into its cell and grouped by
`ModelId` into `ChunkMeshOutput::models`, which `SceneCache` uploads per chunk and
feeds to `SceneFrame::textured`.

The atlas/array split is **temporary**. Blocks are migrating to Blockbench models
one at a time; when the last one moves over, the atlas keeps only the entity
sheets and cracks, and `voxel` / `voxel_array` collapse back into one pipeline.
Twelve blocks have moved so far — everything except `glass`, `bedrock`, `snow`,
`clay` and the four `.bbmodel` plants. Water is off the atlas too, by a third
route: it cannot be a `block_model` (its surface height is per-corner and
per-fluid-level, which baked quads cannot express), so it keeps the cube mesher's
fluid branch and takes its layers from `[block.fluid.texture]` instead. The
now-unused painters in `art::tiles` are deliberately left in place: they stay
reachable through `paint_named`, so nothing warns, and deleting them is a single
clean sweep once the last block migrates.

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
- **State pattern** — `wyven_app::Screen` + `ScreenStack` (login → menus →
  loading → in-game → pause overlay). The runner only drives the stack; it never
  learns what a screen is. `wyven_app::Screen` is an alias for
  `Screen<Wyvencraft>` so nothing has to spell the parameter.
- **Dependency inversion at every engine seam** — the six traits in the table
  above. The renderer takes its art through `TileSource` instead of containing
  it, the mesher takes `&impl BlockCatalog` instead of a `BlockRegistry`, the
  transport takes `Protocol` + `JoinVerifier` instead of naming this game's
  messages and tickets, and the runner takes a `Game` instead of loading
  content itself.
- **Ports & adapters** (at I/O boundaries only — never in per-frame/per-voxel hot
  paths): `content::ContentSource`, `save::WorldRepository`, `state::session::Session`,
  `boot::Environment`. Each has a real impl, a null/embedded impl, and a test double,
  which is what lets content loading, saving, session logic and startup be tested
  without a filesystem, socket, or GPU. `ContentSource` reads *bytes*, with text
  derived from them, because model files carry PNGs and vertex buffers.
  `AssetSource` lives in `wyven-assets`, *below* both the model loaders and game
  content, which is what stopped `model` reaching up into `content` for its bytes.
  `chat::CommandContext` is a port for a different reason — not I/O, but to invert
  a dependency: commands are policy and live in `chat`, but they act on registries
  and inventories owned by `state`, which already depends on `chat`. Real impl
  `SessionContext` in `state::ingame_state::chat`, double `chat::FakeContext`.
- **Chat commands** — `chat::command::ChatCommand`, one impl per file, found
  through the `COMMANDS` registry, exactly like `ModelLoader`. There is no
  `Command` enum and no `match` over command kinds: both would need editing for
  every addition. Each command parses its own arguments and phrases its own
  messages; the dispatcher only resolves the name and checks `permission()`.
- **File-loaded models** — `wyven_model::ModelLoader` is one impl per format (`.gltf`,
  `.bbmodel`), all normalising to the same `ModelMesh` (Y-up, right-handed, one
  block = 1.0, top-left UVs), so callers cannot tell them apart. Both shipped
  exports of `assets/models/vine_sword` describe the same object, and a test
  asserts the two loaders agree vertex-for-vertex — that is what pins the
  bbmodel face-corner order, UV-rotation direction and 1/16 scale.
- **Registry** — `world::BlockRegistry`, `inventory::ItemRegistry`,
  `entity::EntityRegistry`, `wyven_render::TileRegistry` (texture name → atlas
  tile — allocation only; the *art* comes from a `TileSource`),
  `wyven_render::BlockTextureSet` (texture path → block-texture-array layer).
- **Strategy** — `wyven_voxel::WorldGenerator` (trait) with `world::NoiseGenerator`
  as this game's implementation.
- **Producer/consumer** — `wyven_voxel::ChunkLoader` (crossbeam worker pool).
- **Command/message** — `net::protocol` (`ClientMessage` / `ServerMessage`),
  carried by a `wyven_net::Host`/`Client` that never names either type.

### Where to make common changes
| Task                    | Location                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------ |
| Add a block type        | Model it in Blockbench (**Java Block/Item** format, per-face UV), export *Block/Item Model* to `assets/blocks/<name>.json` with its textures as separate PNGs in `assets/textures/`, then one `[[block]]` in `assets/blocks.toml` with `block_model = "assets/blocks/<name>.json"`. A full cube is `from [0,0,0]` → `to [16,16,16]`; set `cullface` on **each face's own direction** or it draws all six even when buried (and hides itself in the wrong one); set `tintindex` only on faces taking a biome colour. Textures may be 16px or 256px — anything square that divides 256 is scaled up to it. Parsing is `wyven_model::blockjson`, baking `wyven_voxel::blockmodel`, layers `wyven_render::block_textures`. The older `textures = "<name>"` atlas path still works for the blocks not yet re-authored |
| A non-cube block, the new way | Same as above — the model is already in cell coordinates, so it needs no placement. `random_yaw` on the `[[block]]` table turns each instance about its cell (and drops its `cullface`, which a turned face can no longer honour). The hitbox is derived from the geometry: a model that covers all six cell faces stays a plain `Target::Cell`, anything else gets a measured box. `assets/blocks/cornflower_block.json` is the worked example |
| Biome tint (grass, foliage, water) | `tint`, `foliage_tint` and `water_tint` per biome in `assets/worldgen.toml`, selected by a face's `tintindex` — **0 grass, 1 foliage, 2 water**, Minecraft's numbering — resolved through `WorldGenerator::biome_tint` at mesh time. Greyscale art (`grass_block_top`, `grass_block_side_overlay`, `oak_leaves`, `water_flow`) is what lets one texture serve every climate; a *coloured* texture must not be tinted. Grass and foliage default to white (the identity); `water_tint` defaults to `DEFAULT_WATER_TINT` instead, because nothing else in the water art supplies any colour |
| A non-cube block (plant, prop) | `[block.model]` in `assets/blocks.toml` — same `path`/`scale`/`offset`/`rotation` spelling as `[item.model]`, plus `random_yaw`. The block then emits **no** cube faces (`textures` becomes optional) and is baked into its cell by `wyven_voxel::meshing::culled`. Give it `solid = false` to walk through: `World::is_solid` is collision only, `is_targetable` is what the crosshair uses, and `is_replaceable` decides whether placing swallows it. Add a matching `[item.model]` on the same path so the drop, the hand and the icon agree — the registry memoises by path, so both share one `ModelId` |
| Block hitbox (crosshair, outline, cracks) | Not authored — `content::placed_bounds` measures the placed model and `wyven_voxel::model_hitbox` turns it into a square, centred, cell-clamped box on `BlockModel::hitbox`, so it can never drift from what is drawn. The raycast predicate returns `wyven_voxel::Target::{Cell,Box}`; a `Box` the ray misses does **not** stop the march. `InGameState::{target_at,hitbox_at}` are the single source for targeting *and* both overlays. Mob line-of-sight deliberately stays `is_solid` + `Target::Cell` — a flower must not hide you |
| Add an item / tool / food / armor | `assets/items.toml` (`[item.tool]` with `harvests`/`dig_speed`/`durability` and optional `damage`, `[item.food]`, `[item.armor]` with `slot`/`defense`/`durability`, `[item.model]` with `path`/`scale`/`offset`/`rotation`); starter kit in the same file |
| Tool tiers / melee damage | Tiers are data only — `dig_speed` + `durability` (+ `damage` on swords and axes) in `assets/items.toml`. There is deliberately **no** harvest-level gate: `harvests` decides *what* a tool is for, never *whether* a block drops. A tool without `damage` swings for `mobs::PLAYER_ATTACK_DAMAGE` (the fist); the local swing resolves in `InGameState::melee_damage`, a client's in `client_melee_damage`, which reads the inventory that client last reported |
| Load a 3D model from a file | drop a `.gltf` or `.bbmodel` in `assets/models/`, then point at it: `[entity.visual] kind = "model"` in `assets/entities.toml`, or `[item.model]` in `assets/items.toml`. Parsing is `wyven_model::{gltf,bbmodel}` behind the `ModelLoader` trait (a new format = a new impl + one line in `ModelRegistry::LOADERS`); placement math in `wyven_model::mesh`; GPU textures uploaded lazily in `state::ingame_state::view`. Exports disagree on which plane a flat object lies in (the tiered tools are flat in XY, `vine_sword` in YZ), so `ModelSpec::rotation` turns a model about its own axes — applied after `offset` re-centres it |
| Armor (slots, defense, wear, render) | data in `assets/items.toml` `[item.armor]`; slots 36..42 in `inventory::inventory`; defense math in `entity::player::damage`; equip gate + wear in `state::ingame_state`; worn-model shells + cape in `entity::model::build_mesh_armored`; procedural sheets in `art::armor`; net via `ServerMessage::PlayerEquipment` |
| Item icons              | `ItemIcon` is computed in `content`: `Cube` (from block faces), `Flat` (painters in `art::tiles::paint_named`, PNG-overridable), or `Model` for items with `[item.model]`. Drawn by `ui::icon::draw_item_icon`; the atlas and the 3D icon sheet are registered with egui in `state::shared`. A `block_model` block's cube faces are 16px stand-ins **downsampled from its own 256px art** (`content::derive_face_tiles`), so the icon and the dropped-item cube keep working unchanged — read them through `GameContent::face_textures`, never `Block::textures` |
| 3D item icons           | `wyven_render::icons` (cell layout, framing transform, ortho camera) + `Renderer::draw_icons`; the sheet is rendered **once** at startup by `state::shared::build_icon_sheet`, one cell per `ModelId`. Tune presentation with `ICON_YAW`/`ICON_PITCH`/`ICON_ROLL`/`FILL` in `wyven_render::icons` |
| Live player preview     | offscreen pass `wyven_render::Renderer::draw_model` + `PreviewFrame`; mesh/camera in `state::ingame_state::{update_preview_mesh,preview_frame}`; image + egui `TextureId` in `state::shared` (the runner draws it *before* the world pass, which is the ordering `wyven_app` owns) |
| Block drop rules        | `drops = ...` on the block in `assets/blocks.toml` (`"self"`, `"none"`, `{ requires_tool }`, `{ item, count }`) |
| Entity tuning / new kind | `assets/entities.toml` (physics/movement/vitals/item/mob components); a new *behavior* = one new component in `entity::kind` + its code hook |
| Add / tune a mob        | `assets/entities.toml` (`[entity.mob]`: health, speeds, `behavior`, `knockback_resistance`, `[entity.mob.ranged]`, `drops`; `[entity.visual]` humanoid `skin=`/`arms_forward` or quadruped) + a `[[spawn]]` entry in `assets/spawning.toml`; skin painter in `art::mobskin` (PNG override `assets/textures/mob_<name>.png`) |
| Mob AI behavior         | `entity::brain` (pure state machine: Idle/Wander/Chase/Flee, perception → intent); body/physics in `entity::mob`; state-layer tick/perception/combat in `state::ingame_state::mobs`. Disposition is the `entity::kind::Behavior` enum (`passive`/`hostile`/`inert`) — a new disposition is a variant plus its arm in `MobBrain::think`, never a new boolean |
| A static prop / statue  | an `[[entity]]` with `[entity.mob] behavior = "inert"` and no `spawning.toml` entry. `knockback_resistance` is the separate axis: `1.0` bolts it down, `0.0` lets a hit send it flying |
| Mob spawning rules      | `assets/spawning.toml` (caps, ring distances, weights, groups, night rules — strict: unknown entity rejects the file); planner in `entity::spawning` (pure, seeded); world sampling in `state::ingame_state::mobs::update_spawning` |
| Projectiles             | `entity::projectile` (ballistic `Arrow`); launch tuning in `[entity.mob.ranged]`; ticked in `state::ingame_state::mobs::update_arrows` |
| Change terrain          | `assets/worldgen.toml` (blocks, ores, sea level, biome surfaces — ⚠ alters existing worlds); noise/climate/mesas stay in `world::generation::{noise,biome,generator}` |
| Trees/boulders/features | shapes+chances in `assets/worldgen.toml`; canopy strategies in `world::generation::features` (jittered-grid anchors) |
| Ground cover / scatter  | per-biome `plants = [...]` + `plant_chance_per_mille` in `assets/worldgen.toml`; placement in `world::generation::features::try_plant`, which runs **after** trees and only into air so it can never punch a hole in a trunk |
| Meshing                 | `wyven_voxel::meshing::culled` (face culling; greedy is a TODO)                                  |
| Water / fluids          | `[block.fluid]` component in `assets/blocks.toml` (auto-registers flow blocks); sim in `world::fluid` (fluid-agnostic, ticked from `state::ingame_state` with the block registry passed in — the engine's `World` has no registry to ask) |
| Fluid art / animation   | `[block.fluid.texture]` on the same `[[block]]`: an animation strip of `frames` square frames stacked top to bottom, **column 0 flowing, column 1 still** (one column serves both). Loaded by `content::load_fluid_texture` into consecutive layers via `wyven_render::block_textures::resolve_strip`, meshed in the fluid branch of `wyven_voxel::meshing::culled` — source blocks are still on every face, flowing blocks still on top/bottom and flowing on the sides. `fps` defaults to 8 and the loader **rejects** an `fps * 3600` that is not a whole multiple of `frames` — the shader's animation clock wraps hourly and would otherwise jump mid-swell. `tint` is a `tintindex` (2 = water); `opacity` (0..1) rescales the art's alpha, and is the *only* thing controlling how much of the riverbed shows through, because a body of fluid is one blended sheet however deep it is (its interior faces are culled). A fluid still needs an atlas stand-in for its inventory icon, derived from its first still frame in `content` — read it through `GameContent::face_textures` |
| Player movement/physics | numbers in `assets/entities.toml`; formulas in `entity::player`, `entity::physics`         |
| Crafting recipes        | `assets/recipes.toml` (data); logic in `inventory::crafting`; panel in `ui::inventory`     |
| A new screen            | implement `wyven_app::Screen`, push/replace via `Transition`                                |
| HUD / inventory UI      | `ui::hud`, `ui::inventory`                                                                 |
| Add a chat command      | **one new file in `src/chat/command/` + one entry in `COMMANDS`** — nothing else changes (the `ModelLoader::LOADERS` pattern). Implement `ChatCommand` (`name`/`usage`/`permission`/`run`); the command parses its own arguments and phrases its own messages. It reaches the world only through the `CommandContext` port, so it never sees a `PlayerId` and works identically for the local player and a remote client. Test it against `chat::FakeContext` with no world, socket or GPU. **Caveat:** a command needing a capability the port doesn't expose yet also grows `CommandContext` + its two impls — and if that capability must reach a *client*, a `ServerMessage` too (`GrantItems`, `Teleport`) |
| Authorize a player      | `ops.toml` in the working directory (`ops = [{ id = "<account uuid>", name = "..." }]`), parsed by `chat::ops`. Keyed by the **account id from the verified join ticket**, never by anything a client asserts. The host/singleplayer player is always an op; only the authority loads the file |
| Chat log / input bar    | `chat::{log,composer}` (pure state), drawn by `ui::chat::draw_chat`; keys `chat`/`chat_command` in `config::Keybinds` (T and /) |
| Networking              | `wyven_net::{server,client}` + `net::protocol` transport; role behind `state::session::Session` (Singleplayer/Host/Client + `FakeSession`); message application in `state::ingame_state::net` |
| Accounts / login        | `wyven_auth::{client,session,account,keys,verifier}`. `AuthClient` is a port (`HttpAuthClient` via ureq / `FakeAuthClient`); `LoginState` is the first screen; the session persists in `profile.toml`. Server lives in the private repo [gustaavik/wcauthserver](https://github.com/gustaavik/wcauthserver) |
| Who may join            | `net::TicketJoin` (the game's `JoinVerifier`) checks the Ed25519 ticket a client puts in netcode `user_data` **before** a `PlayerId` exists — a failure is disconnected with no `Welcome`. Keys come from `authkeys.toml` via `wyven_auth::KeyCache`; **no keys means no joins**, never "everyone joins". Ticket format is `wcauth-ticket`, shared verbatim with the server — literally the same crate, pulled from the wcauthserver repo as a git dependency |
| Player nameplates       | `ui::nameplate` painted from `InGameState::draw_nameplates`; projection is `wyven_render::Camera::project`. egui composites after the world pass with no depth, so occlusion is an explicit `wyven_voxel::raycast` against `is_solid` |
| Saving / world files    | `save` module (formats, `saves/<slug>/`) behind `save::WorldRepository` (File/Null/InMemory); triggers in `state::ingame_state::save_world` |
| Pipelines / passes      | `wyven_render::pipeline`, `wyven_render::renderer`                                                     |
| GPU meshes / camera     | `state::ingame_state::view` (`SceneCache`) — the only holder of `RenderContext`             |
| Startup / dev env vars  | `boot::plan::BootPlan::from_env` (pure, tested); effects in `boot::start::initial_screen`. A dev-boot plan skips the login screen via `boot::start::boot_account`. The window/device/event-loop side is `wyven_app::run`, reached through the `Game` impl in `state::shared` |
| Loading `assets/*.toml` | `wyven_assets::AssetSource` (Fs/Embedded/Map) + one `load_or_builtin` helper           |
| Shaders                 | `crates/wyven-render/shaders/*.{vert,frag}`, declared in `wyven_render::shaders` (they moved out of `assets/` with the renderer — they are compiled into the binary, so nothing reads that path at runtime). `voxel.vert` is shared by both chunk pipelines, so a new vertex attribute means editing it plus `wyven_render::vertex` and every `ChunkVertex { .. }` site |

## Conventions & gotchas

- **Edition 2024.** Let-chains (`if x && let Some(y) = z`) are used; `gen` is a
  reserved keyword — don't use it as an identifier.
- The module is named `core` — always reference it as `crate::core`; never write a
  bare `core::` path (it would resolve to the std `core` crate).
- The crate-level `#![allow(dead_code)]` is **gone** — `cargo build --workspace`
  is warning-free and `cargo clippy --workspace --all-targets` is clean. Keep
  them that way rather than re-adding a blanket allow.
- **The engine must not learn what Wyvencraft is.** If a `wyven-*` crate needs a
  game fact, grow the trait it already takes (see the table under Architecture)
  rather than adding a dependency. Cargo will stop you from adding the wrong one,
  but it will not stop you from smuggling a hardcoded block name into
  `wyven-voxel`. `wyven-auth` is the only crate that may name `wcauth-ticket`.
- **Visual data must stay off `Block`.** `Block`'s `Debug` repr feeds
  `content::content_hash`, which gates multiplayer joins — so anything derived
  from a texture or a model (block models, baked geometry, *every* atlas tile
  index) lives on `GameContent` indexed by `BlockId`, never on `Block`. `Block`
  has no `textures` field at all: `blocks.toml` reports texture *names* out of
  band in `BlockVisuals`, and `content` resolves them to slots. Otherwise two peers whose grass is drawn slightly differently
  would be refused a shared world. If a `content_hash` test starts failing after
  a visual change, that is the invariant breaking, not the test being stale.
- **Block textures end up 256×256 whatever they were authored at.** An array
  image has one extent for every layer, so anything square that divides 256 is
  replicated up to it at load (`wyven_render::block_textures::upscale`) — nearest, at
  an integer factor, so a 16px texture is pixel-identical on screen under the
  array's nearest magnification. Anything else warns and renders magenta. The
  16px atlas is stricter — it hard-rejects anything that *isn't* 16×16.
- **A cutout block must cull against itself.** `BakedBlockModel::occludes` is
  measured from the texture's opacity, so a leaves cube correctly occludes
  nothing — which alone would leave every face of every block inside a canopy
  drawn. Both mesher paths therefore also drop the face a transparent or cutout
  block shares with a neighbour *of its own kind*.
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

1. `cargo build --workspace` / `cargo clippy --workspace --all-targets` clean.
2. `cargo test --workspace` green (488 tests: 8 core, 54 model, 43 render,
   14 voxel, 369 game).
3. Run it: `WYVEN_BOOT_INGAME=1 cargo run` (or host/join for net changes) and
   confirm no panic over several seconds. In a sandbox, launch in the background and
   poll the log rather than blocking on a foreground `sleep` — `timeout` is not
   installed on this machine; `perl -e 'alarm 25; exec @ARGV' cargo run` works.

For a change that touches the engine/game line, two extra checks:

4. The engine still builds with no GitHub credential:
   `cargo build -p wyven-core -p wyven-assets -p wyven-render -p wyven-model -p wyven-voxel -p wyven-net -p wyven-input -p wyven-app`
5. The logic crates still test with no Vulkan device:
   `cargo test -p wyven-core -p wyven-voxel -p wyven-model -p wyven-assets`
