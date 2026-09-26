//! A bundle is a tuple of components inserted together at spawn.

use crate::{Ecs, Entity};

/// A group of components an entity can be spawned with. Implemented for `()`
/// and for tuples of up to sixteen components, each of a distinct type.
pub trait Bundle: 'static {
    fn insert_into(self, ecs: &mut Ecs, entity: Entity);
}

macro_rules! impl_bundle {
    ($($name:ident),*) => {
        impl<$($name: 'static),*> Bundle for ($($name,)*) {
            #[allow(non_snake_case, unused_variables)]
            fn insert_into(self, ecs: &mut Ecs, entity: Entity) {
                let ($($name,)*) = self;
                $( ecs.insert(entity, $name); )*
            }
        }
    };
}

impl_bundle!();
impl_bundle!(A);
impl_bundle!(A, B);
impl_bundle!(A, B, C);
impl_bundle!(A, B, C, D);
impl_bundle!(A, B, C, D, E);
impl_bundle!(A, B, C, D, E, F);
impl_bundle!(A, B, C, D, E, F, G);
impl_bundle!(A, B, C, D, E, F, G, H);
impl_bundle!(A, B, C, D, E, F, G, H, I);
impl_bundle!(A, B, C, D, E, F, G, H, I, J);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
