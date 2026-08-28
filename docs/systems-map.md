# Systems map

The seams other than auth and netcode, at map depth. For those two see
[identity-and-auth.md](identity-and-auth.md) and [netcode.md](netcode.md); for *where to
change what*, see [`CLAUDE.md`](../CLAUDE.md).

---

## 1. The crate graph

A cargo workspace. Nine `wyven-*` members under `crates/` are the **engine**; the root
package `wyvencraft` (`src/`) is the **game**.

```
wyven-core     ← nothing            coordinate/voxel types, AABB/Ray/Frustum, RNG, timing
wyven-assets   ← nothing            AssetSource port (Fs/Embedded/Map), Rgba8, decode_png
wyven-render   ← core, assets       Vulkan: context, pipelines, mesh upload, camera, atlas
wyven-model    ← core, assets, render   .gltf/.bbmodel/block JSON → ModelMesh + texture
wyven-voxel    ← core, render, model    chunk store, loader pool, culled mesher, raycast
wyven-net      ← core               renet transport, generic over <Protocol, JoinVerifier>
wyven-input    ← core               winit events → frame-coherent InputState
wyven-auth     ← nothing            accounts, key cache, ticket verify
wyven-app      ← core, render, input    window, egui, event loop, screen stack
```

**The direction is enforced by cargo.** No engine crate lists the game, so a violation stops
compiling rather than being caught in review. Note that `wyven-voxel` depends on
`wyven-render` and not the reverse — that is the crate-level encoding of *render never
depends on world*.

What cargo **cannot** catch is smuggling a game *fact* into an engine crate: a hardcoded
block name, a survival-mode rule, a mob's behaviour. When engine code needs one, grow the
trait it already takes.

---

## 2. The six traits

Meaning crosses the engine/game line on exactly six traits. Each is declared by the engine
and implemented by the game, so every arrow below points **engine → game** at call time even
though the dependency points the other way.

| Trait                            | Declared                                                              | Implemented                                                | Dispatch                                        |
| -------------------------------- | --------------------------------------------------------------------- | ---------------------------------------------------------- | ----------------------------------------------- |
| `TileSource` (+ `ReservedTiles`) | `crates/wyven-render/src/tile_registry.rs:54`                         | `src/art/mod.rs:32` (`WyvencraftArt`)                      | `Box<dyn>`, load-time only                      |
| `BlockCatalog`                   | `crates/wyven-voxel/src/catalog.rs:28`                                | `src/content/catalog.rs:28` (`BlockAppearance`)            | **`&impl`, monomorphised** — per-voxel hot path |
| `BlockProperties`                | `crates/wyven-voxel/src/catalog.rs:88`                                | `src/world/block.rs:181` (`BlockRegistry`)                 | `Arc<dyn>` — crosses threads                    |
| `WorldGenerator`                 | `crates/wyven-voxel/src/generate.rs:9`                                | `src/world/generation/generator.rs:110` (`NoiseGenerator`) | `Arc<dyn>` — cloned into workers                |
| `Protocol` / `JoinVerifier`      | `crates/wyven-net/src/session.rs:15`, `:32`                           | `src/net/join.rs:20`, `:63`                                | monomorphised into `Host<P, V>`                 |
| `Game` (+ `Screen`)              | `crates/wyven-app/src/lib.rs:38`, `crates/wyven-app/src/screen.rs:56` | `src/state/shared.rs:92` + every screen                    | `Box<dyn Screen>`                               |

The dispatch column is the design rule, not an accident: **ports belong at I/O boundaries,
never in per-frame or per-voxel hot paths.** `mesh_chunk` takes `&impl BlockCatalog` so
monomorphising puts the loads back; `BlockProperties` is `dyn` because it is stored in
`World` and read from multiple threads.

`BlockCatalog` has six methods because the mesher asks six questions — it does not expose a
`Block`. That is the shape to copy when growing a trait: name the fact the engine is
missing, as narrowly as possible.

Each trait has a test double in the engine, which is what proves the engine needs no game:
`NoTiles` / `Swatch`, `TestCatalog`, `TestProperties`, `FlatGenerator`, `OpenJoin`.

---

## 3. The frame loop

`src/app.rs` is the entire game→engine handoff:

```rust
pub fn run() -> Result<(), AppError> {
    wyven_app::run(Wyvencraft::new())
}
```

The engine then owns the loop and calls back. One-time startup, in order:

```
game.window()      → WindowConfig, before the window opens
game.textures()    → RendererTextures { atlas, blocks } → Renderer::new
game.start(Boot{}) → (Shared, first Screen)
```

`Wyvencraft::new()` runs **before any window exists** — content loading, `AccountState`, and
`BootPlan::from_env` are all GPU-free. `Game::start` is where the one-shot GPU work happens:
the 3D item-icon sheet, the player-preview image, three egui texture registrations, and then
`boot::initial_screen`.

Per frame:

```
1. dt = clock.tick()
2. stack.update(Frame)        → Screen::update      (screens write `grab_cursor` here)
3. gui.begin_frame()
   stack.ui(&egui_ctx, Frame) → Screen::ui
4. apply_cursor_grab, input.end_frame()
5. renderer.acquire()
   ├─ preview pass:  G::preview_target + stack.preview_frame() → draw_model  (offscreen)
   ├─ world pass:    stack.scene_frame(aspect)                 → draw
   ├─ egui:          gui.draw_on_image                         (composites, no depth)
   └─ present
```

**The preview pass must go first.** The world pass clears, so it has to be the swapchain
image's first writer before the egui overlay; inserting an offscreen pass between world and
egui breaks vulkano's swapchain layout tracking.

Two `Frame` values get built per frame rather than one, because `gui` is borrowed mutably
between steps 2 and 3.

### Events and the screen stack

`window_event` gives **egui every event first** and forwards only what egui did not consume:

```rust
let consumed = self.gui.as_mut().map(|g| g.update(&event)).unwrap_or(false);
```

That single line is why typing in chat is safe by construction — a focused `TextEdit`
starves `InputState`, so gameplay keys never fire. The `!typing` guards in the in-game frame
only cover the frame between opening the bar and the widget taking focus.

`ScreenStack` drives only the top screen, and applies the returned `Transition`
(`crates/wyven-app/src/screen.rs:39`): `Push` / `Replace` / `Pop` / `ReplaceAll` / `Quit`,
running `on_exit` and `on_enter` hooks around each. But `scene_frame` and `preview_frame`
search the stack **top-down**, which is how the pause overlay keeps the world drawing behind
it — `PauseMenuState::is_overlay()` returns true, its `scene_frame` returns `None`, and the
search falls through to `InGameState`.

Because `Push` runs `on_exit` on the covered screen, **pausing autosaves**.
`shutdown` — from a window close or `Transition::Quit` — pops every screen running `on_exit`,
which is the other save trigger.

Screen chain: `MainMenuState` → `SingleplayerMenuState` /
`MultiplayerMenuState` → `LoadingState` / `ConnectingState` → `InGameState` →
`PauseMenuState`. There is no login screen: `boot::start::boot_account` settles
the account before the first screen is built.

`SingleplayerMenuState` reaches `InGameState` two ways — Play (offline) and Host
(the same world, with a socket bound on it). `MultiplayerMenuState` is the server
browser and only ever leads to `ConnectingState`.

---

## 4. The render seam

Two rules, both structural rather than conventional.

**`render` never depends on `world`.** `wyven-render` lists only `wyven-core` and
`wyven-assets`. The active screen builds plain `CpuMesh` data and hands the renderer a
`SceneFrame` (`crates/wyven-render/src/renderer.rs:84`) — a camera, sky and light
parameters, and *borrowed* mesh references, split into lists by which texture binds at set 0:

- `opaque` / `transparent` — the shared 16px atlas. One bind per pass.
- `array_opaque` / `array_transparent` — the block texture array, one 256×256 layer per
  named texture, chosen per vertex. Also one bind per pass, however many block types are
  on screen.
- `textured` — meshes that bring their own texture and rebind set 0 **per draw**
  (file-loaded `.gltf` / `.bbmodel` models).
- `lines` — the block selection outline.

There is still **no model matrix**; every transform is baked on the CPU.

**Only `state::ingame_state::view` touches `RenderContext`.** `SceneCache`
(`src/state/ingame_state/view.rs:84`) is the sole holder, and `refresh_view` is the single
per-frame seam where simulation becomes GPU data. Chunk streaming, mob AI, fluids and
interaction are plain logic — which is exactly why they are testable without a Vulkan
device. The one exception is startup: `Game::start` uploads temporary meshes to render the
icon sheet, waits on the fence, and drops them.

Because `SceneFrame` holds borrows into `SceneCache`, the runner drops it explicitly after
the draw.

---

## 5. Ports and adapters

At I/O boundaries only. Each has a real impl, a null or embedded impl, and a test double —
which is what lets content loading, saving, session logic, startup and auth be tested with
no filesystem, socket or GPU.

| Port                                           | Declared                               | Real                           | Null / embedded       | Double                    |
| ---------------------------------------------- | -------------------------------------- | ------------------------------ | --------------------- | ------------------------- |
| `AssetSource` (re-exported as `ContentSource`) | `crates/wyven-assets/src/source.rs:25` | `FsSource`                     | `EmbeddedSource`      | `MapSource`               |
| `WorldRepository`                              | `src/save/repository.rs:32`            | `FileWorldRepository`          | `NullWorldRepository` | `InMemoryWorldRepository` |
| `Environment`                                  | `src/boot/plan.rs:20`                  | `SystemEnv`                    | —                     | `MapEnv`                  |
| `CommandContext`                               | `src/chat/command/context.rs:35`       | `SessionContext`               | —                     | `FakeContext`             |
| `Session`                                      | `src/state/session/mod.rs:78`          | `HostSession`, `ClientSession` | `SingleplayerSession` | `FakeSession`             |
| `AuthClient`                                   | `crates/wyven-auth/src/client.rs:65`   | `HttpAuthClient`               | —                     | `FakeAuthClient`          |

The doubles are **hand-written real implementations, not mocks** — `FakeAuthClient` actually
signs tickets that verify against the key set it publishes, which is what makes the auth
tests meaningful without a server.

Two of these deserve a note on *why*:

**`AssetSource` reads bytes, with text derived from them**, because model files carry PNGs
and vertex buffers. It lives in `wyven-assets`, *below* both the model loaders and game
content, which is what stopped `wyven-model` reaching up into the game for its bytes.

**`CommandContext` is a port for a different reason — not I/O, but to invert a dependency.**
Commands are policy and live in `chat`, but they act on registries and inventories owned by
`state`, which already depends on `chat`. Binding the actor into the impl also keeps
`PlayerId` out of the commands' vocabulary entirely.

`NullWorldRepository` earns its place: the state layer used to hold an `Option<WorldSave>`
and guard every write with a `None` check, where the `None` stood in for both "client" and
"ephemeral dev-boot world". The null object removed the branch.

---

## 6. Content loading

Game content is TOML under `assets/`, loaded once into `content::GameContent` and shared via
`Arc`. Behaviour is *components plus code hooks*: data declares typed components, each
implemented once in Rust and dispatched on — **never on block or item identity**.

Load order inside `from_source` is a dependency chain: the tile registry first (everything
allocates from it), then blocks, then items (which need blocks), then entities, worldgen and
spawning, then models, then Blockbench block JSON, then fluid strips, then plain atlas
blocks last so a block with its own model keeps its model-derived tiles.

Every file has an embedded `include_str!` fallback and degrades fail-soft with a logged
warning. Worldgen and spawning are the exceptions — strict, because any bad name there
rejects the file.

**Visual data must stay off `Block`.** `Block`'s `Debug` repr feeds `content_hash`, which
gates multiplayer joins, so anything derived from a texture or model — block models, baked
geometry, every atlas tile index — lives on `GameContent` indexed by `BlockId`. `Block` has
no `textures` field at all; `blocks.toml` reports texture *names* out of band and `content`
resolves them to slots. Otherwise two peers whose grass is drawn slightly differently would
be refused a shared world.

If a `content_hash` test starts failing after a visual change, that is the invariant
breaking, not the test being stale.

---

## 7. Chunks and threading

The **only** worker pool in the game is chunk generation. Everything else is main-thread.

```
worker threads                     main thread
──────────────                     ───────────
WorldGenerator::generate(pos)
   │  crossbeam unbounded (MPMC — one queue, all workers)
   └──────────────────────────────► drain_ready()
                                    World::insert_chunk  (+ replay edit overlay, mark dirty)
                                    take_dirty() → mesh_queue
                                    mesh_chunk(&Chunk, &BlockAppearance, neighbor, tint)
                                    GpuMesh::upload × 5
                                    SceneCache → scene_frame → Renderer::draw
```

`ChunkLoader` (`crates/wyven-voxel/src/loader.rs:23`) dedupes in-flight requests, and its
`Drop` replaces the sender so every worker's `recv()` errors and the threads join.

**Meshing deliberately stays on the main thread** and is budgeted instead — it needs
neighbour block data that lives in the `World`. Per frame: at most 8 chunks meshed, at most
64 chunk requests issued nearest-first, and anything beyond the render distance plus a
2-chunk margin unloaded. (`rayon` appears in `wyven-voxel`'s manifest but has no uses.)

Networking is not threaded either — `Session::poll` and `flush` are pumped synchronously
from the frame.

The other threads are all short-lived one-shot HTTP workers: one per login/register/refresh
attempt, one detached key fetch after sign-in, one per join attempt for the ticket. Each
communicates through an `std::sync::mpsc` channel polled non-blocking each frame. The
exception is `boot_account`, which blocks the main thread deliberately — it runs before the
window exists.

What actually crosses a thread boundary: `Arc<dyn WorldGenerator>` (which is why that trait
is `Send + Sync`), `Arc<dyn BlockProperties>` inside `World`, `Arc<dyn AuthClient>`, and
`AccountState` (an `Arc<RwLock<…>>` cloned into every screen and into the ticket worker).

**Terrain never crosses the network.** Every peer regenerates identical terrain from the
shared seed; only the edit overlay travels. `Channel::Chunk` refers to renet message
chunking, not voxel chunks.

---

## 8. Where a change usually goes wrong

Signs you are about to put something on the wrong side of a seam:

- Adding `wyvencraft` to a `wyven-*` `Cargo.toml` — cargo will refuse.
- Hardcoding a block or item name inside an engine crate — cargo will **not** refuse.
- Reaching for `RenderContext` outside `state::ingame_state::view`.
- Putting a visual field on `Block` — it will silently change `content_hash` and split
  multiplayer.
- Moving game policy into the engine because it was convenient. `GameMode`'s
  survival-versus-creative rules lived in `core` for exactly that reason and had to be moved
  back out.

The counter-example is worth knowing too: **not everything should be extracted.**
`world::fluid` stayed in the game because all of it is `[block.fluid]` policy with no
substrate underneath worth extracting, and the mob methods stayed on `InGameState` because
mob AI genuinely needs the world *and* the player — a `MobSystem::update` taking five
borrows relocates coupling rather than removing it.
