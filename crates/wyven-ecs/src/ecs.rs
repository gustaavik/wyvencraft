//! The entity-component store itself.

use std::fmt;

use crate::Bundle;
use crate::commands::CommandBuffer;
use crate::entity::{Entity, EntityAllocator};
use crate::query::{Query, QueryMut};
use crate::sparse_set::SparseSet;
use crate::storage::Storages;

/// Entities, and one sparse set of components per component type.
///
/// There is no scheduler and no global resources: systems are plain functions
/// that take the `Ecs` alongside whatever else they need, so every dependency
/// a system has is visible in its signature.
#[derive(Default)]
pub struct Ecs {
    entities: EntityAllocator,
    storages: Storages,
}

impl Ecs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make a new entity holding every component in `bundle`.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.entities.allocate();
        bundle.insert_into(self, entity);
        entity
    }

    /// Remove `entity` and every component it holds. False if it was already
    /// gone, which makes despawning idempotent.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.free(entity) {
            return false;
        }
        self.storages.remove_entity(entity);
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// How many entities are alive.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.len() == 0
    }

    /// Every living entity, in slot order.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter()
    }

    /// Give `entity` a `T`, replacing any it had. False (and `component` is
    /// dropped) if the entity is dead.
    pub fn insert<T: 'static>(&mut self, entity: Entity, component: T) -> bool {
        if !self.entities.is_alive(entity) {
            return false;
        }
        self.storages.get_or_create::<T>().insert(entity, component);
        true
    }

    /// Take `entity`'s `T` away from it.
    pub fn remove<T: 'static>(&mut self, entity: Entity) -> Option<T> {
        self.storages.get_mut::<T>()?.remove(entity)
    }

    pub fn get<T: 'static>(&self, entity: Entity) -> Option<&T> {
        self.storages.get::<T>()?.get(entity)
    }

    pub fn get_mut<T: 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        self.storages.get_mut::<T>()?.get_mut(entity)
    }

    pub fn has<T: 'static>(&self, entity: Entity) -> bool {
        self.storages.get::<T>().is_some_and(|s| s.contains(entity))
    }

    /// How many entities hold a `T`.
    pub fn count<T: 'static>(&self) -> usize {
        self.storages.get::<T>().map_or(0, SparseSet::len)
    }

    /// Direct access to every `T`, for when one component type is all a
    /// system needs.
    pub fn storage<T: 'static>(&self) -> Option<&SparseSet<T>> {
        self.storages.get::<T>()
    }

    pub fn storage_mut<T: 'static>(&mut self) -> Option<&mut SparseSet<T>> {
        self.storages.get_mut::<T>()
    }

    /// Iterate the entities matching `Q`, with read access to their
    /// components. See [`crate::query`] for the term syntax.
    pub fn query<Q: Query>(&self) -> impl Iterator<Item = (Entity, Q::Item<'_>)> + '_ {
        Q::iter(&self.entities, &self.storages)
    }

    /// Call `f` for every entity matching `Q`, with mutable access where the
    /// query asks for it. Each component type may appear in `Q` only once.
    pub fn for_each_mut<Q, F>(&mut self, f: F)
    where
        Q: QueryMut,
        F: for<'s> FnMut(Entity, Q::Item<'s>),
    {
        Q::for_each(&self.entities, &mut self.storages, f);
    }

    /// Run every command in `commands`, in the order they were recorded,
    /// leaving the buffer empty and reusable.
    pub fn apply(&mut self, commands: &mut CommandBuffer) {
        for command in commands.drain() {
            command(self);
        }
    }

    /// Despawn everything.
    pub fn clear(&mut self) {
        self.entities = EntityAllocator::default();
        self.storages.clear();
    }
}

impl fmt::Debug for Ecs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ecs")
            .field("entities", &self.entities.len())
            .finish_non_exhaustive()
    }
}
