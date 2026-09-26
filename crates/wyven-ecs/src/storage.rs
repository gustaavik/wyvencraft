//! The per-type storage map an [`crate::Ecs`] keeps, and the typed access to it.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::sparse_set::{ErasedStorage, SparseSet};

/// One [`SparseSet`] per component type that has ever been inserted.
///
/// Public only so it can appear in the (sealed) query traits; not exported.
#[derive(Default)]
pub struct Storages {
    map: HashMap<TypeId, Box<dyn ErasedStorage>>,
}

impl Storages {
    pub(crate) fn get<T: 'static>(&self) -> Option<&SparseSet<T>> {
        let erased: &dyn Any = self.map.get(&TypeId::of::<T>())?.as_ref();
        erased.downcast_ref()
    }

    pub(crate) fn get_mut<T: 'static>(&mut self) -> Option<&mut SparseSet<T>> {
        let erased: &mut dyn Any = self.map.get_mut(&TypeId::of::<T>())?.as_mut();
        erased.downcast_mut()
    }

    pub(crate) fn get_or_create<T: 'static>(&mut self) -> &mut SparseSet<T> {
        let erased: &mut dyn Any = self
            .map
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(SparseSet::<T>::default()))
            .as_mut();
        erased
            .downcast_mut()
            .expect("a storage is always keyed by its own component type")
    }

    /// Lift `T`'s storage out of the map, so it can be borrowed mutably
    /// alongside others. Must be handed back with [`Storages::restore`].
    pub(crate) fn take<T: 'static>(&mut self) -> Option<Box<SparseSet<T>>> {
        let erased: Box<dyn Any> = self.map.remove(&TypeId::of::<T>())?;
        Some(
            erased
                .downcast()
                .expect("a storage is always keyed by its own component type"),
        )
    }

    pub(crate) fn restore<T: 'static>(&mut self, storage: Option<Box<SparseSet<T>>>) {
        if let Some(storage) = storage {
            self.map.insert(TypeId::of::<T>(), storage);
        }
    }

    pub(crate) fn remove_entity(&mut self, entity: crate::Entity) {
        for storage in self.map.values_mut() {
            storage.remove_entity(entity);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }
}
