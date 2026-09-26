# Wyvencraft

A voxel survival game written in **Rust** with **Vulkan** (via [`vulkano`](https://crates.io/crates/vulkano)):
it looks like Minecraft, plays a Valheim-style progression loop, and digs
Terraria-style into a layered underground.

The world is a set of concentric **biome rings** around spawn, each harder than
the last. Every biome hides **shrines**; reading one reveals where that biome's
**boss altar** stands. Offer an effigy carved from the biome's materials at the altar, beat
the boss, and its drops make the tool that can mine the next ring's ore —
solo or with friends over the network.

> Status: all core milestones complete (M0–M8). Builds and runs on macOS via
> MoltenVK. See [Roadmap](#roadmap) for optional polish that remains.

Developer documentation on how the systems communicate — accounts and join tickets,
the multiplayer wire protocol, and the engine/game seams — is in [`docs/`](docs/README.md).

## Account

Singleplayer needs no account. **Multiplayer does** — a host verifies a signed join
ticket before admitting a player, and the game has no login screen of its own: the
[launcher](https://github.com/gustaavik/wc-launcher) signs you in and hands the
session to the game.

Register at **[game.wyvencraft.com](https://game.wyvencraft.com)**.

## Features

- **Biome rings** — Meadows → Darkwood → Mire → Frostpeaks → Ashlands, as
  noise-warped rings around spawn, each with its own terrain shape, surface,
  vegetation and colours, blended at the borders. Deterministic from a seed, so
  every multiplayer peer generates identical terrain.
- **A layered underground** — a dirt-flecked surface layer, stone caverns, then
  deepstone down to flooded depths, each with its own cave density; every
  biome's metal lies only under that biome.
- **Shrines, altars and bosses** — shrines and boss altars are stamped into the
  terrain from the seed. Reading a wayrune reveals the altar on everyone's
  compass; an offering there summons the boss — a phased fight with telegraphed
  attacks, shared loot for everyone in the arena, and a drop that crafts the next
  tier of pickaxe. The Meadows' **Elder Stag** is the first.
- **Threaded chunk streaming** — terrain is generated on a worker pool and meshed
  on a per-frame budget, so the world streams in smoothly out to the render
  distance. Frustum culling skips off-screen chunks.
- **Player & physics** — swept-AABB collision against the voxel grid, gravity,
  jumping, and block break/place via voxel raycasting.
- **First & third person** — toggle with `F5` (first → third-behind → third-front);
  a box-part humanoid model is drawn in third person and for remote players.
- **Peer-to-peer multiplayer** — host-authoritative, direct/LAN connect. One player
  hosts from their world list; others keep a list of saved servers showing each
  one's name, players online and ping. The world seed is shared on join and only block edits +
  player positions travel the wire.
- **Menus, HUD & inventory** — main menu, world list (play / host), server browser, pause
  overlay, crosshair, hotbar, `F3` debug overlay, and a click-to-move inventory
  screen.
- **Transparent rendering** — water and glass are drawn in a separate alpha-blended
  pass.

## Requirements

- **Rust** ≥ 1.85 (the crate uses edition 2024). Built/tested on 1.96.
- **A Vulkan-capable GPU.** On macOS this means **MoltenVK** (Vulkan-over-Metal).
  On Windows and Linux, the GPU vendor's driver provides Vulkan. Virtual
  machines generally have no Vulkan driver and cannot run the game; it exits
  with code 3 and says so, rather than crashing.

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

### Windows toolchain

- **Visual Studio Build Tools** with the *Desktop development with C++* workload
  (MSVC, CMake, Ninja). Rust's `x86_64-pc-windows-msvc` target links with it.
- **Python 3** on `PATH`.
- The Vulkan runtime (`vulkan-1.dll`) ships with every current GPU driver, so
  there is nothing to install to *run* the game.
- Release builds link the C runtime statically
  (`-C target-feature=+crt-static`, set in `release.yml`), so players need no
  Visual C++ Redistributable. A plain `cargo build` does not, and its binary
  will not start on a PC without the redistributable.

With nothing else set, `shaderc` builds from its vendored sources on the first
build, which takes several minutes. Two things make that painless:

- Install the [LunarG Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows)
  and set `SHADERC_LIB_DIR` to its `Lib` directory. The build then links the
  SDK's prebuilt `shaderc_combined.lib` instead (see
  [`.cargo/config.toml.example`](.cargo/config.toml.example)).
- If you do build from source, keep the checkout (or `CARGO_TARGET_DIR`) at a
  short path such as `C:\src\wyvencraft`. shaderc's CMake tree nests deep
  enough to cross the 260-character `MAX_PATH` limit otherwise.

Do **not** copy the macOS `[env]` entries into `.cargo/config.toml` on Windows:
`VK_ICD_FILENAMES` pointing at a MoltenVK manifest that does not exist leaves
the Vulkan loader with no driver at all.

> On Linux the same code builds with CMake, Ninja, Python 3 and
> `libasound2-dev`; leave `SHADERC_LIB_DIR` unset so shaderc builds its vendored
> sources (Ubuntu's `libshaderc-dev` is not usable, see `ci.yml`).

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

| Variable               | Effect                                   |
| ---------------------- | ---------------------------------------- |
| `WYVEN_BOOT_INGAME=1`  | Boot straight into a singleplayer world  |
| `WYVEN_HOST=1`         | Host a session immediately (port `6091`) |
| `WYVEN_JOIN=addr:port` | Connect to a host immediately            |
| `WYVEN_WORLD=test`     | Select the world too connect too         |

## Controls

| Input           | Action                                        |
| --------------- | --------------------------------------------- |
| `W` `A` `S` `D` | Move                                          |
| Mouse           | Look                                          |
| `Space`         | Jump                                          |
| Left click      | Break block                                   |
| Right click     | Place selected block                          |
| Scroll wheel    | Select hotbar slot                            |
| `Q`             | Drop one item from the selected slot          |
| `E`             | Open/close inventory                          |
| `F5`            | Toggle perspective (1st / 3rd person)         |
| `F3`            | Toggle debug overlay                          |
| `F2`            | Save a screenshot (its name in chat opens it) |
| `Esc`           | Pause (or close inventory)                    |

## Multiplayer

1. One player chooses **Multiplayer → Host Game** (listens on UDP `6091`).
2. Others choose **Multiplayer → Join** and enter the host's `address:port`
   (e.g. `192.168.1.20:6091`, or `127.0.0.1:6091` on the same machine).

Everyone taking part needs an account — see [Account](#account).

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

| Concern        | Crate                                        |
| -------------- | -------------------------------------------- |
| Vulkan         | `vulkano`, `vulkano-shaders`, `vulkano-util` |
| Window / input | `winit`                                      |
| UI             | `egui`, `egui_winit_vulkano`                 |
| Math           | `glam`                                       |
| Noise          | `noise`                                      |
| Networking     | `renet`, `renet_netcode`                     |
| Concurrency    | `rayon`, `crossbeam-channel`, `parking_lot`  |
| Serialization  | `serde`, `bincode`                           |

## Roadmap

The Minecraft × Terraria × Valheim loop is built in milestones. **Milestone 1 is
done**: biome rings, the layered underground, tiered ores, shrines and altars,
world progression, and the Meadows boss. Next:

- **RPG gear** — item rarity, stat blocks (damage, crit, defense, speed),
  random prefixes rolled at craft time, accessory slots
- **Skills** — mining, woodcutting, weapons, running and jumping that level by use
- **Boss powers** — trophies placed at a spawn shrine grant a timed,
  re-activatable buff
- **Bosses 2–5** — Darkwood, Mire, Frostpeaks and Ashlands, each with its shrines,
  altar, offering, key drop and the next pickaxe tier (bronze, iron, silver);
  cavern dungeons; lava; biome hazards

Polish not yet implemented:

- Real per-vertex ambient occlusion and greedy meshing
- In-game settings screen (render distance / FOV / sensitivity)
- NAT traversal / relay for internet play
- Textures for items

## License

The **code** is dual licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. This covers `src/` and the ten `wyven-*` engine crates under
`crates/`, which are usable on their own — the engine knows nothing about
Wyvencraft and depends on none of it.

The **assets** are not. Everything under `assets/` is proprietary; see
[assets/LICENSE](assets/LICENSE). You may use them to build, run and modify this
software, but not redistribute them separately or ship them in another product.
Fork the engine freely and bring your own art.

The **name** is not either — see [TRADEMARK.md](TRADEMARK.md). Building on this
and saying so is fine; calling your own thing Wyvencraft is not.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
