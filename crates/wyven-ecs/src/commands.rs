//! Structural changes deferred until a query has finished.
//!
//! A system iterating with [`crate::Ecs::for_each_mut`] holds the ECS
//! borrowed, so it cannot spawn or despawn mid-loop. It records the change
//! here instead, and the caller applies the buffer once the loop is done —
//! in the order the commands were recorded.

use std::fmt;

use crate::{Bundle, Ecs, Entity};

type Command = Box<dyn FnOnce(&mut Ecs)>;

/// An ordered list of spawns, despawns, inserts and removes to apply later.
#[derive(Default)]
pub struct CommandBuffer {
    commands: Vec<Command>,
}

impl CommandBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn<B: Bundle>(&mut self, bundle: B) {
        self.push(move |ecs| {
            ecs.spawn(bundle);
        });
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.push(move |ecs| {
            ecs.despawn(entity);
        });
    }

    pub fn insert<T: 'static>(&mut self, entity: Entity, component: T) {
        self.push(move |ecs| {
            ecs.insert(entity, component);
        });
    }

    pub fn remove<T: 'static>(&mut self, entity: Entity) {
        self.push(move |ecs| {
            ecs.remove::<T>(entity);
        });
    }

    /// Any other deferred change.
    pub fn push(&mut self, command: impl FnOnce(&mut Ecs) + 'static) {
        self.commands.push(Box::new(command));
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = Command> + '_ {
        self.commands.drain(..)
    }
}

impl fmt::Debug for CommandBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandBuffer")
            .field("pending", &self.commands.len())
            .finish()
    }
}
