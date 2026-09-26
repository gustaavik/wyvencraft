//! One component type's storage: a sparse set keyed by entity index.
//!
//! `sparse[index]` points into `dense`, and `owners[i]` says which entity
//! `dense[i]` belongs to. Insert, remove and lookup are O(1); iteration walks
//! `dense`, which is contiguous. Removal swaps the last element into the hole,
//! so iteration order is *not* insertion order once anything is removed.

use std::any::Any;

use crate::entity::Entity;

const EMPTY: u32 = u32::MAX;

/// Dense storage for every `T` in an [`crate::Ecs`].
#[derive(Debug)]
pub struct SparseSet<T> {
    sparse: Vec<u32>,
    dense: Vec<T>,
    owners: Vec<Entity>,
}

impl<T> Default for SparseSet<T> {
    fn default() -> Self {
        Self {
            sparse: Vec::new(),
            dense: Vec::new(),
            owners: Vec::new(),
        }
    }
}

impl<T> SparseSet<T> {
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// The entities holding a `T`, parallel to the component values.
    pub fn owners(&self) -> &[Entity] {
        &self.owners
    }

    fn slot(&self, entity: Entity) -> Option<usize> {
        let dense = *self.sparse.get(entity.index() as usize)?;
        if dense == EMPTY {
            return None;
        }
        let dense = dense as usize;
        (self.owners[dense] == entity).then_some(dense)
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.slot(entity).is_some()
    }

    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.slot(entity).map(|i| &self.dense[i])
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        self.slot(entity).map(|i| &mut self.dense[i])
    }

    /// Store `value` for `entity`, returning whatever it replaced.
    pub fn insert(&mut self, entity: Entity, value: T) -> Option<T> {
        if let Some(i) = self.slot(entity) {
            return Some(std::mem::replace(&mut self.dense[i], value));
        }
        let index = entity.index() as usize;
        if index >= self.sparse.len() {
            self.sparse.resize(index + 1, EMPTY);
        }
        self.sparse[index] = self.dense.len() as u32;
        self.dense.push(value);
        self.owners.push(entity);
        None
    }

    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let i = self.slot(entity)?;
        self.sparse[entity.index() as usize] = EMPTY;
        let value = self.dense.swap_remove(i);
        self.owners.swap_remove(i);
        if let Some(&moved) = self.owners.get(i) {
            self.sparse[moved.index() as usize] = i as u32;
        }
        Some(value)
    }

    /// `(entity, &value)` pairs in storage order.
    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.owners.iter().copied().zip(self.dense.iter())
    }

    /// `(entity, &mut value)` pairs in storage order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.owners.iter().copied().zip(self.dense.iter_mut())
    }
}

/// A [`SparseSet`] with its component type erased, so an [`crate::Ecs`] can
/// hold one per type in a single map and still despawn across all of them.
pub(crate) trait ErasedStorage: Any {
    fn remove_entity(&mut self, entity: Entity);
}

impl<T: 'static> ErasedStorage for SparseSet<T> {
    fn remove_entity(&mut self, entity: Entity) {
        self.remove(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ecs;

    fn entities(n: usize) -> Vec<Entity> {
        let mut ecs = Ecs::new();
        (0..n).map(|_| ecs.spawn(())).collect()
    }

    #[test]
    fn insert_get_and_replace() {
        let [a, b] = entities(2)[..] else {
            unreachable!()
        };
        let mut set = SparseSet::default();
        assert_eq!(set.insert(b, "b"), None);
        assert_eq!(set.insert(a, "a"), None);
        assert_eq!(set.insert(a, "A"), Some("a"));
        assert_eq!(set.get(a), Some(&"A"));
        assert_eq!(set.get(b), Some(&"b"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn removing_the_middle_keeps_the_moved_element_reachable() {
        let es = entities(3);
        let mut set = SparseSet::default();
        for (i, &e) in es.iter().enumerate() {
            set.insert(e, i);
        }
        assert_eq!(set.remove(es[0]), Some(0));
        assert_eq!(
            set.get(es[2]),
            Some(&2),
            "the last element was swapped into the hole"
        );
        assert_eq!(set.get(es[1]), Some(&1));
        assert!(!set.contains(es[0]));
        assert_eq!(set.remove(es[0]), None);
    }

    #[test]
    fn a_stale_handle_does_not_read_its_successors_component() {
        let mut ecs = Ecs::new();
        let old = ecs.spawn(());
        ecs.despawn(old);
        let new = ecs.spawn(());
        assert_eq!(old.index(), new.index());

        let mut set = SparseSet::default();
        set.insert(new, 7);
        assert_eq!(set.get(old), None);
        assert_eq!(set.remove(old), None);
        assert_eq!(set.get(new), Some(&7));
    }
}
