# Inside the game crate: layers, the ECS, and the simulation

The engine/game line is enforced by cargo: no `wyven-*` crate lists the game, so crossing it
the wrong way stops compiling. This document is about the lines **inside** the game crate,
which one crate cannot have cargo enforce — and about the entity store that most of the
game's moving things now live in.

## 1. Four layers, pointing inward

```
presentation ─┐
              ├─→ application ─→ domain
infrastructure┘
```

| Layer | What lives there | May depend on |
| --- | --- | --- |
| `domain` | The rules: blocks, items, entity kinds, brains, physics, worldgen, structures, progression, chat commands, the hashed `Registries`. | engine primitives only |
| `application` | Use cases and the ports they need: `Simulation` and its methods, the ECS components and systems, the `Session` port, the wire protocol, loading the rules through the asset port, the boot plan. | `domain` |
| `infrastructure` | Adapters to the outside world: saves, sockets and the real sessions, audio devices, the data directory, `ops.toml`, the recipe file, the editor's file access. | `application`, `domain` |
| `presentation` | Everything that draws or reads input: the screens, egui views, entity meshes, art, keybinds, the item editor, and `GameContent` (rules + visuals + sounds). | everything below |

`boot` and `app.rs` are the **composition root** — the only code that names concrete adapters
and wires them into the first screen. Nothing imports them back: `app.rs` hands
`boot::initial_screen` to the game as a factory.

### The test that holds it

`tests/architecture.rs` reads `src/` and fails on:

- a `domain` file naming `crate::{application, infrastructure, presentation, boot}`, `egui`,
  `wyven_render`, `vulkano`, `wyven_app`, `wyven_net`, `renet` or `std::fs`;
- an `application` file naming `crate::{infrastructure, presentation, boot}`, `egui`,
  `wyven_render`, `vulkano`, `wyven_app` or `std::fs`.

Comments are skipped: a doc link to an outer layer explains a boundary, it does not cross one.
**If it fails, move the code — do not widen the rule.** `application` may use the network's
*vocabulary* (`wyven_net::PlayerId`, `Channel`) because the protocol is its contract; it may not
use a transport.

### Visual data cannot reach the content hash

`domain::content::Registries` is every definition two peers must agree on, and computes its
fingerprint in its constructor — so a registry set can never carry a stale hash.
`presentation::content::Visuals` is every texture, model, icon and label, a separate value.
"A visual difference must never refuse a join" is therefore a fact about the types, not a
convention each loader has to remember.

## 2. The ECS

`crates/wyven-ecs` is a small engine crate: generational `Entity` handles, one `SparseSet` per
component type, tuple queries, and a `CommandBuffer`. It has no scheduler, no resources map,
no change detection and no `unsafe`.

### What is an entity

| Entity | Components |
| --- | --- |
| Dropped item | `Transform`, `Velocity`, `Body` (centred), `ItemDrop` |
| Arrow | `Transform`, `Velocity`, `Projectile` |
| Simulated mob | `MobId`, `Kind`, `Transform`, `Velocity`, `Body` (feet), `Mob`, `Health`, `Animation`, `Sensed`, `Decision`, and `Boss` on a boss |
| A client's copy of a host mob | `MobId`, `Kind`, `Transform`, `Body`, `Health`, `Animation`, `Replica`, `LastSeen` |
| Another player | `RemotePlayer`, `Animation`, `LastSeen` |

A simulated mob and its replica are the **same shape** with a different marker, so targeting,
rendering and the boss bar are each one query.

**The local player is not an entity.** It is the one thing every frame reads well over a
hundred times — camera, input, inventory and HUD all hang off it — so it is a singleton on
`Simulation` (a "resource", in ECS terms), with its `AnimationState` beside it. Making it an
entity would turn each of those reads into a lookup by a handle that can never fail.

### Conventions

- **Components are data.** Rules live on the domain types (`ItemDrop::expired`,
  `Mob::think`, `Projectile::step`); a component that is only bookkeeping lives in
  `application::ecs::components`.
- **Systems are plain functions** in `application::ecs::systems`, taking `&mut Ecs` plus
  exactly what they read, and returning what they could not apply themselves. Every
  dependency a system has is in its signature, and each is testable with a bare `Ecs`.
- **Mutable queries are closures**:
  `ecs.for_each_mut::<(&mut A, &B, Option<&mut C>, With<D>, Without<E>), _>(|e, (a, b, c, (), ())| …)`.
  Each component type may appear once; naming one twice panics rather than aliasing.
- **Structural changes mid-loop** go through a `CommandBuffer`, applied before the system
  returns.
- **Ids stay at the boundary.** The wire and the saves still spell `MobId` and `PlayerId`;
  `Entity` handles never leave the process. Lookups by id are linear scans
  (`systems::mobs::find`, `systems::players::find`) — tens of entities, and no second index to
  keep in step with every spawn and despawn.
- **The view is handed values, not entities.** `refresh_view` turns queries into
  `DropSprite`, `ArrowSprite`, `MobSprite` and `PeerSprite`, so no render code learns where
  things are stored — and nothing in the view advances a clock any more.

### A mob's frame

`Mob::update` used to do everything in one call. It is now five steps on the domain type, which
`systems::mobs::simulate` runs as passes over every mob:

1. **perceive** — `Sensed(Some(sight))`, or `None` for a mob in an unloaded chunk, which then
   sits the frame out entirely, cooldowns and all;
2. **think** — tick the cooldown, ask the brain, turn to face;
3. **steer** — the gait (a boss plants itself mid wind-up) plus any knockback still bleeding off;
4. **integrate** — collide, land, hop a ledge;
5. **animate**, then **act** — the attack it commits to, or a boss's beat.

No step reads another mob's state, which is what makes a pass per step the same as the
per-mob call it replaced.

## 3. The simulation and the screen

`application::simulation::Simulation` is the world as one value: the voxel world, the ECS,
the local player and what they carry, fluids, the chunk loader, progression, the day cycle, the
mob director (spawner, boss fight, telegraph), a play clock, and an **outbox**.

- `Simulation::new(rules, seed, start)` generates a world with no screen, socket or GPU. Its
  use cases — `step_player`, `tick_mobs`, `resolve_mob_attacks`, `settle_kill`, `pop_loot`,
  `fly_arrows`, `update_spawning`, `throw`, `update_drops`, `loose_volley`,
  `summon_minions`, `tick_entities` — are tested that way.
- **The outbox** is how the simulation tells other peers what it decided (spawns, hurts,
  deaths, arrows, boss beats) without knowing about a session. The network pump drains it every
  frame and broadcasts it only when the session serves peers — exactly where the old
  `emit_mob_event` used to drop events at the moment of emission.
- `authoritative` is fixed per session: whether this peer decides or mirrors.

`InGameState` is `{ sim, net, content, save, ..presentation state }`, and is glue. What stays on
it is whatever chats, draws, or asks the session mid-use-case: landing a boss beat announces
it, a client's attack is a request to the host.

`frame.rs::update` spells the frame's order once, each step a method:

```
screen keys → take controls → player keys → step_local_player → interact
  → tick_session (clocks, chat, discovery, autosave, network pump,
                  the authority's fluids / mobs / boss fight / spawning, streaming)
  → sim.tick_entities (drops, arrows, replica and peer animation, the local body)
  → refresh_view
```

The order is part of the contract. It includes one known quirk kept on purpose: the pump runs
before the mob tick, so a frame's mob events go out on the next frame.

## 4. What was deliberately not done

- **No ECS scheduler or resources.** Systems are called from named places in a stated order;
  a scheduler would hide that order behind registration.
- **No `NetId → Entity` index.** Linear lookups are simpler and cannot drift.
- **The local player stays a singleton** — see above.
- **Engine hot paths were left alone.** `mesh_chunk`, `renderer::draw` and `runner::frame` are
  long, but they are hot paths with their own structure, and outside this refactor's scope.
- **No wire, save or content-hash change** anywhere in the restructure: the content hash was
  checked byte-identical before and after (`f93c7ffdd44deb43` for the builtins and the shipped
  assets).
