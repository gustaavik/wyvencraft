//! Generational entity handles and the allocator that hands them out.
//!
//! An [`Entity`] is an index plus a generation. Despawning bumps the slot's
//! generation before the index is reused, so a handle kept past its entity's
//! death can never be mistaken for whatever lives in that slot next — the
//! failure a plain `Vec` index has the moment anything is `swap_remove`d.

use std::fmt;

/// A handle to one entity. Cheap to copy, meaningless on its own: every
/// question about it goes through the [`crate::Ecs`] that made it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// The slot this entity occupies. Unique among *living* entities only.
    pub fn index(self) -> u32 {
        self.index
    }

    /// How many times this slot had been freed when the entity was made.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    generation: u32,
    alive: bool,
}

/// Hands out entity handles and recycles freed slots.
/// Public only so it can appear in the (sealed) query traits; not exported.
#[derive(Debug, Default)]
pub struct EntityAllocator {
    slots: Vec<Slot>,
    free: Vec<u32>,
    alive: usize,
}

impl EntityAllocator {
    pub(crate) fn allocate(&mut self) -> Entity {
        self.alive += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            return Entity {
                index,
                generation: slot.generation,
            };
        }
        let index = u32::try_from(self.slots.len()).expect("more than u32::MAX entities");
        self.slots.push(Slot {
            generation: 0,
            alive: true,
        });
        Entity {
            index,
            generation: 0,
        }
    }

    /// Free `entity`'s slot. False if it was already dead (or never ours).
    pub(crate) fn free(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let slot = &mut self.slots[entity.index as usize];
        slot.alive = false;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(entity.index);
        self.alive -= 1;
        true
    }

    pub(crate) fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation)
    }

    pub(crate) fn len(&self) -> usize {
        self.alive
    }

    /// Every living entity, in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.alive)
            .map(|(index, slot)| Entity {
                index: index as u32,
                generation: slot.generation,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freed_slot_is_reused_under_a_new_generation() {
        let mut alloc = EntityAllocator::default();
        let first = alloc.allocate();
        assert!(alloc.free(first));
        let second = alloc.allocate();

        assert_eq!(first.index(), second.index());
        assert_ne!(first, second);
        assert!(!alloc.is_alive(first), "the stale handle must stay dead");
        assert!(alloc.is_alive(second));
    }

    #[test]
    fn freeing_twice_is_refused() {
        let mut alloc = EntityAllocator::default();
        let e = alloc.allocate();
        assert!(alloc.free(e));
        assert!(!alloc.free(e));
        assert_eq!(alloc.len(), 0);
    }

    #[test]
    fn iter_lists_only_the_living() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.allocate();
        let b = alloc.allocate();
        let c = alloc.allocate();
        alloc.free(b);
        assert_eq!(alloc.iter().collect::<Vec<_>>(), vec![a, c]);
        assert_eq!(alloc.len(), 2);
    }

    #[test]
    fn debug_shows_index_and_generation() {
        let mut alloc = EntityAllocator::default();
        let e = alloc.allocate();
        alloc.free(e);
        let e = alloc.allocate();
        assert_eq!(format!("{e:?}"), "Entity(0v1)");
    }
}
