//! Queries: which entities have which components, and access to them.
//!
//! A query is a tuple of *terms*. Each term names one component type and says
//! how it is wanted:
//!
//! | term             | yields            | entity must have it? |
//! | ---------------- | ----------------- | -------------------- |
//! | `&T`             | `&T`              | yes                  |
//! | `&mut T`         | `&mut T`          | yes (mutable only)   |
//! | `Option<&T>`     | `Option<&T>`      | no                   |
//! | `Option<&mut T>` | `Option<&mut T>`  | no (mutable only)    |
//! | `With<T>`        | `()`              | yes                  |
//! | `Without<T>`     | `()`              | must *not* have it   |
//!
//! Read-only queries ([`crate::Ecs::query`]) return an ordinary iterator.
//! Mutable queries ([`crate::Ecs::for_each_mut`]) take a closure instead: a
//! closure's borrows end with each call, which is what lets several storages
//! be handed out `&mut` at once with no `unsafe` anywhere in this crate. Each
//! component type may appear only **once** in a mutable query; naming one
//! twice panics rather than aliasing.
//!
//! Iteration is driven by the smallest storage among the required terms, and
//! every other term is probed per entity. A query with no required term at
//! all (only `Option`/`Without`) walks every living entity.

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::entity::{Entity, EntityAllocator};
use crate::sparse_set::SparseSet;
use crate::storage::Storages;

/// Requires the entity to have a `T`, without borrowing it.
pub struct With<T>(PhantomData<T>);

/// Requires the entity *not* to have a `T`.
pub struct Without<T>(PhantomData<T>);

/// One term of a read-only query.
pub trait Term {
    type Component: 'static;
    type Item<'w>;
    /// Whether an entity lacking the component is excluded.
    const REQUIRED: bool;
    fn fetch<'w>(set: Option<&'w SparseSet<Self::Component>>, e: Entity) -> Option<Self::Item<'w>>;
}

/// One term of a mutable query.
pub trait TermMut {
    type Component: 'static;
    type Item<'s>;
    const REQUIRED: bool;
    fn fetch<'s>(
        set: Option<&'s mut SparseSet<Self::Component>>,
        e: Entity,
    ) -> Option<Self::Item<'s>>;
}

impl<T: 'static> Term for &T {
    type Component = T;
    type Item<'w> = &'w T;
    const REQUIRED: bool = true;
    fn fetch(set: Option<&SparseSet<T>>, e: Entity) -> Option<&T> {
        set?.get(e)
    }
}

impl<T: 'static> Term for Option<&T> {
    type Component = T;
    type Item<'w> = Option<&'w T>;
    const REQUIRED: bool = false;
    fn fetch(set: Option<&SparseSet<T>>, e: Entity) -> Option<Option<&T>> {
        Some(set.and_then(|s| s.get(e)))
    }
}

impl<T: 'static> Term for With<T> {
    type Component = T;
    type Item<'w> = ();
    const REQUIRED: bool = true;
    fn fetch(set: Option<&SparseSet<T>>, e: Entity) -> Option<()> {
        set?.contains(e).then_some(())
    }
}

impl<T: 'static> Term for Without<T> {
    type Component = T;
    type Item<'w> = ();
    const REQUIRED: bool = false;
    fn fetch(set: Option<&SparseSet<T>>, e: Entity) -> Option<()> {
        (!set.is_some_and(|s| s.contains(e))).then_some(())
    }
}

impl<T: 'static> TermMut for &T {
    type Component = T;
    type Item<'s> = &'s T;
    const REQUIRED: bool = true;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<&T> {
        set?.get(e)
    }
}

impl<T: 'static> TermMut for &mut T {
    type Component = T;
    type Item<'s> = &'s mut T;
    const REQUIRED: bool = true;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<&mut T> {
        set?.get_mut(e)
    }
}

impl<T: 'static> TermMut for Option<&T> {
    type Component = T;
    type Item<'s> = Option<&'s T>;
    const REQUIRED: bool = false;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<Option<&T>> {
        Some(set.and_then(|s| s.get(e)))
    }
}

impl<T: 'static> TermMut for Option<&mut T> {
    type Component = T;
    type Item<'s> = Option<&'s mut T>;
    const REQUIRED: bool = false;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<Option<&mut T>> {
        Some(set.and_then(|s| s.get_mut(e)))
    }
}

impl<T: 'static> TermMut for With<T> {
    type Component = T;
    type Item<'s> = ();
    const REQUIRED: bool = true;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<()> {
        set?.contains(e).then_some(())
    }
}

impl<T: 'static> TermMut for Without<T> {
    type Component = T;
    type Item<'s> = ();
    const REQUIRED: bool = false;
    fn fetch(set: Option<&mut SparseSet<T>>, e: Entity) -> Option<()> {
        (!set.is_some_and(|s| s.contains(e))).then_some(())
    }
}

/// A read-only query: a tuple of [`Term`]s. Implemented for tuples of up to
/// eight terms; the method is the store's business, reached through
/// [`crate::Ecs::query`].
pub trait Query {
    type Item<'w>;
    #[doc(hidden)]
    fn iter<'w>(
        entities: &'w EntityAllocator,
        storages: &'w Storages,
    ) -> impl Iterator<Item = (Entity, Self::Item<'w>)> + 'w;
}

/// A mutable query: a tuple of [`TermMut`]s over distinct component types,
/// reached through [`crate::Ecs::for_each_mut`].
pub trait QueryMut {
    type Item<'s>;
    #[doc(hidden)]
    fn for_each<F>(entities: &EntityAllocator, storages: &mut Storages, f: F)
    where
        F: for<'s> FnMut(Entity, Self::Item<'s>);
}

/// The entities to walk: the smallest required storage's owners, or every
/// living entity when nothing is required.
fn driver<'w>(smallest: Option<&'w [Entity]>, entities: &'w EntityAllocator) -> Cow<'w, [Entity]> {
    match smallest {
        Some(owners) => Cow::Borrowed(owners),
        None => Cow::Owned(entities.iter().collect()),
    }
}

/// Keep the shorter of two candidate driver lists.
fn shorter<'a>(best: Option<&'a [Entity]>, candidate: &'a [Entity]) -> Option<&'a [Entity]> {
    match best {
        Some(best) if best.len() <= candidate.len() => Some(best),
        _ => Some(candidate),
    }
}

/// Panic if a mutable query names any component type twice.
fn assert_distinct(ids: &[TypeId], names: &[&str]) {
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            panic!(
                "a mutable query names `{}` twice; each component type may appear once",
                names[i]
            );
        }
    }
}

macro_rules! impl_query {
    ($($term:ident $set:ident),+) => {
        impl<$($term: Term),+> Query for ($($term,)+) {
            type Item<'w> = ($($term::Item<'w>,)+);

            fn iter<'w>(
                entities: &'w EntityAllocator,
                storages: &'w Storages,
            ) -> impl Iterator<Item = (Entity, Self::Item<'w>)> + 'w {
                $( let $set = storages.get::<$term::Component>(); )+
                let mut smallest: Option<&'w [Entity]> = None;
                $(
                    if $term::REQUIRED {
                        smallest = shorter(smallest, $set.map_or(&[][..], |s| s.owners()));
                    }
                )+
                let driver = driver(smallest, entities);
                (0..driver.len()).filter_map(move |i| {
                    let e = driver[i];
                    Some((e, ($($term::fetch($set, e)?,)+)))
                })
            }
        }

        impl<$($term: TermMut),+> QueryMut for ($($term,)+) {
            type Item<'s> = ($($term::Item<'s>,)+);

            fn for_each<Func>(entities: &EntityAllocator, storages: &mut Storages, mut f: Func)
            where
                Func: for<'s> FnMut(Entity, Self::Item<'s>),
            {
                assert_distinct(
                    &[$(TypeId::of::<$term::Component>()),+],
                    &[$(type_name::<$term::Component>()),+],
                );
                $( let mut $set = storages.take::<$term::Component>(); )+
                let driver: Vec<Entity> = {
                    let mut smallest: Option<&[Entity]> = None;
                    $(
                        if $term::REQUIRED {
                            smallest = shorter(
                                smallest,
                                $set.as_deref().map_or(&[][..], |s| s.owners()),
                            );
                        }
                    )+
                    driver(smallest, entities).into_owned()
                };
                // Storages are out of the map for the duration; put them back
                // even if the closure panics, so a caught panic cannot leave
                // the ECS missing whole component types.
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    for &e in &driver {
                        let item = ($(
                            match $term::fetch($set.as_deref_mut(), e) {
                                Some(item) => item,
                                None => continue,
                            },
                        )+);
                        f(e, item);
                    }
                }));
                $( storages.restore($set); )+
                if let Err(panic) = outcome {
                    resume_unwind(panic);
                }
            }
        }
    };
}

impl_query!(A a);
impl_query!(A a, B b);
impl_query!(A a, B b, C c);
impl_query!(A a, B b, C c, D d);
impl_query!(A a, B b, C c, D d, E e2);
impl_query!(A a, B b, C c, D d, E e2, F f2);
impl_query!(A a, B b, C c, D d, E e2, F f2, G g);
impl_query!(A a, B b, C c, D d, E e2, F f2, G g, H h);
