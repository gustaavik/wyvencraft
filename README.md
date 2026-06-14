# Wyvencraft

A Minecraft-style voxel sandbox written in **Rust** with **Vulkan** (via [`vulkano`](https://crates.io/crates/vulkano)).

Procedurally generated, infinite-ish worlds you can walk around, mine, and build
in — solo or with friends over the network.

> Status: all core milestones complete (M0–M8). Builds and runs on macOS via
> MoltenVK. See [Roadmap](#roadmap) for optional polish that remains.

## Features

- **Procedural world generation** — seeded, multi-octave Perlin/Simplex noise for
  terrain height, 3D cave carving, and temperature-based biomes (plains / desert /
  snowy). Deterministic from a seed, so every multiplayer peer generates identical
  terrain.
- **Threaded chunk streaming** — terrain is generated on a worker pool and meshed
  on a per-frame budget, so the world streams in smoothly out to the render
  distance. Frustum culling skips off-screen chunks.
- **Player & physics** — swept-AABB collision against the voxel grid, gravity,
  jumping, and block break/place via voxel raycasting.
- **First & third person** — toggle with `F5` (first → third-behind → third-front);
  a box-part humanoid model is drawn in third person and for remote players.
- **Peer-to-peer multiplayer** — host-authoritative, direct/LAN connect. One player
  hosts; others join by IP. The world seed is shared on join and only block edits +
  player positions travel the wire.
- **Menus, HUD & inventory** — main menu, multiplayer (host / join) menu, pause
  overlay, crosshair, hotbar, `F3` debug overlay, and a click-to-move inventory
  screen.
- **Transparent rendering** — water and glass are drawn in a separate alpha-blended
  pass.

## Requirements

- **Rust** ≥ 1.85 (the crate uses edition 2024). Built/tested on 1.96.
- **A Vulkan-capable GPU.** On macOS this means **MoltenVK** (Vulkan-over-Metal).

### macOS toolchain (Homebrew)

```sh
brew install molten-vk vulkan-loader vulkan-tools shaderc glslang cmake
```

- `shaderc` is needed at build time (the shaders are compiled GLSL → SPIR-V by
  `vulkano-shaders`).
- `molten-vk` + `vulkan-loader` provide the Vulkan runtime.

The repo's [`.cargo/config.toml`](.cargo/config.toml) wires the build/run
environment automatically (points `vulkano-shaders` at the prebuilt shaderc
library and the Vulkan loader at the MoltenVK ICD), so `cargo run` works out of the
box once the packages above are installed.

> On Linux/Windows the same code should build with the native Vulkan SDK +
> `shaderc`; the `[env]` paths in `.cargo/config.toml` are macOS/Homebrew specific
> and can be removed or adjusted.

## Build & Run

```sh
cargo run            # launch the game (Main Menu → Singleplayer / Multiplayer)
cargo build          # build only
cargo test           # run unit tests
cargo clippy         # lint
```

Logging honours `RUST_LOG` (e.g. `RUST_LOG=info,wyvencraft=debug cargo run`).

### Developer shortcuts

Skip the menus with environment variables:

| Variable | Effect |
|---|---|
| `WYVEN_BOOT_INGAME=1` | Boot straight into a singleplayer world |
| `WYVEN_HOST=1` | Host a session immediately (port `25565`) |
| `WYVEN_JOIN=addr:port` | Connect to a host immediately |

## Controls

| Input | Action |
|---|---|
| `W` `A` `S` `D` | Move |
| Mouse | Look |
| `Space` | Jump |
| Left click | Break block |
| Right click | Place selected block |
| Scroll wheel | Select hotbar slot |
| `E` | Open/close inventory |
| `F5` | Toggle perspective (1st / 3rd person) |
| `F3` | Toggle debug overlay |
| `Esc` | Pause (or close inventory) |

## Multiplayer

1. One player chooses **Multiplayer → Host Game** (listens on UDP `25565`).
2. Others choose **Multiplayer → Join** and enter the host's `address:port`
   (e.g. `192.168.1.20:25565`, or `127.0.0.1:25565` on the same machine).

The connection is host-authoritative over direct/LAN UDP (no NAT traversal — use a
LAN address or port-forward). The host shares its world seed so all peers generate
the same terrain; thereafter only block edits and player movement are synced.

## Project structure

```
src/
├── main.rs        # entry point (logging + run)
├── app.rs         # winit event loop, Vulkan window, drives the state stack
├── config.rs      # settings + keybinds
├── core/          # shared value types: voxel coords, AABB/Ray/Frustum, timing
├── render/        # Vulkan layer: context, pipelines, mesh upload, camera, atlas
├── world/         # voxel data: blocks, chunks, generation, meshing, raycast, loader
├── entity/        # player, swept-AABB physics, humanoid model
├── input/         # winit events → frame-coherent input state
├── inventory/     # item/stack/inventory data model (UI-independent)
├── ui/            # egui views: HUD + inventory screen
├── net/           # renet host/client, wire protocol, remote-player interpolation
└── state/         # game-state machine: menu, multiplayer menu, loading,
                   #   connecting, in-game, pause
assets/shaders/    # GLSL shaders (compiled at build time)
```

Dependency direction is one-way and enforced by module boundaries: `render` never
depends on `world`; the game state hands the renderer plain mesh data + a camera.

## Tech stack

| Concern | Crate |
|---|---|
| Vulkan | `vulkano`, `vulkano-shaders`, `vulkano-util` |
| Window / input | `winit` |
| UI | `egui`, `egui_winit_vulkano` |
| Math | `glam` |
| Noise | `noise` |
| Networking | `renet`, `renet_netcode` |
| Concurrency | `rayon`, `crossbeam-channel`, `parking_lot` |
| Serialization | `serde`, `bincode` |

## Roadmap

Optional polish not yet implemented:

- Mobs with AI
- World save/load to disk (chunk types already derive `serde`)
- Real per-vertex ambient occlusion and greedy meshing
- In-game settings screen (render distance / FOV / sensitivity)
- NAT traversal / relay for internet play
- Textures for items
