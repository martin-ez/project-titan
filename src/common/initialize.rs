use bevy::ecs::component::Mutable;
use bevy::ecs::system::{StaticSystemParam, SystemParam};
use bevy::prelude::*;

/// Marker component for entities that require initialization.
///
/// This should be added to entities alongside a component that implements the `Initialize` trait.
/// When the `initialize_system` for the specific component is registered, the initialization code
/// will execute once for each entity with this marker.
#[derive(Component, Default)]
pub struct NeedsInitialization;

/// Trait for initializing components marked with `NeedsInitialization`.
pub trait Initialize<P: SystemParam> {
    fn initialize(&mut self, entity: &Entity, params: &mut P::Item<'_, '_>);
}

/// Generic system that runs initialization code for components that implement the `Initialize`
/// trait, removing the `NeedsInitialization` marker.
pub fn initialize_system<T: Component<Mutability = Mutable> + Initialize<P>, P: SystemParam>(
    mut query: Query<(&mut T, Entity), With<NeedsInitialization>>,
    params: StaticSystemParam<P>,
    mut commands: Commands,
) {
    let mut params = params.into_inner();
    for (mut component, entity) in query.iter_mut() {
        component.initialize(&entity, &mut params);
        commands.entity(entity).remove::<NeedsInitialization>();
    }
}
