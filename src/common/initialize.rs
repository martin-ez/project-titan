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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{headless_app, tick};

    #[derive(Component, Default)]
    struct Counted {
        initializations: u32,
    }

    impl Initialize<()> for Counted {
        fn initialize(&mut self, _entity: &Entity, _params: &mut ()) {
            self.initializations += 1;
        }
    }

    fn app_with_a_counted_entity() -> (App, Entity) {
        let mut app = headless_app();
        app.add_systems(Update, initialize_system::<Counted, ()>);
        let entity = app
            .world_mut()
            .spawn((Counted::default(), NeedsInitialization))
            .id();
        (app, entity)
    }

    fn initializations(app: &App, entity: Entity) -> u32 {
        app.world()
            .get::<Counted>(entity)
            .expect("the entity is never despawned")
            .initializations
    }

    #[test]
    fn a_component_that_needs_initialization_is_initialized() {
        let (mut app, entity) = app_with_a_counted_entity();

        tick(&mut app);

        assert_eq!(initializations(&app, entity), 1);
    }

    #[test]
    fn a_component_is_not_initialized_a_second_time() {
        let (mut app, entity) = app_with_a_counted_entity();

        tick(&mut app);
        tick(&mut app);
        tick(&mut app);

        assert_eq!(initializations(&app, entity), 1);
    }
}
