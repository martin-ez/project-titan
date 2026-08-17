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

/// Marker component for entities whose initialization reported a failure.
///
/// `initialize_system` leaves it behind rather than retrying, so an entity that never got what its
/// initialization was going to give it says so, instead of looking like a rendering fault.
#[derive(Component)]
pub struct InitializationFailed;

/// Trait for initializing components marked with `NeedsInitialization`.
///
/// An implementation that cannot do its work returns the error rather than panicking: this runs
/// every frame over every marked entity, and one entity it cannot read must not take the game down
/// with it.
pub trait Initialize<P: SystemParam> {
    fn initialize(&mut self, entity: &Entity, params: &mut P::Item<'_, '_>) -> Result;
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
        if let Err(error) = component.initialize(&entity, &mut params) {
            error!("initializing {entity} failed: {error}");
            commands.entity(entity).insert(InitializationFailed);
        }
        commands.entity(entity).remove::<NeedsInitialization>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Probe {
        fails: bool,
        ran: bool,
    }

    impl Initialize<()> for Probe {
        fn initialize(&mut self, _entity: &Entity, _params: &mut ()) -> Result {
            self.ran = true;
            if self.fails {
                return Err("the probe was told to fail".into());
            }
            Ok(())
        }
    }

    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, initialize_system::<Probe, ()>);
        app
    }

    fn spawn_probe(app: &mut App, fails: bool) -> Entity {
        app.world_mut()
            .spawn((Probe { fails, ran: false }, NeedsInitialization))
            .id()
    }

    fn ran(app: &App, entity: Entity) -> bool {
        app.world()
            .entity(entity)
            .get::<Probe>()
            .is_some_and(|probe| probe.ran)
    }

    #[test]
    fn an_initialized_entity_loses_its_marker() {
        let mut app = headless_app();
        let entity = spawn_probe(&mut app, false);

        app.update();

        assert!(ran(&app, entity));
        assert!(!app.world().entity(entity).contains::<NeedsInitialization>());
        assert!(!app
            .world()
            .entity(entity)
            .contains::<InitializationFailed>());
    }

    #[test]
    fn an_entity_that_reports_a_failure_is_marked_as_failed() {
        let mut app = headless_app();
        let entity = spawn_probe(&mut app, true);

        app.update();

        assert!(app
            .world()
            .entity(entity)
            .contains::<InitializationFailed>());
    }

    #[test]
    fn an_entity_that_reports_a_failure_is_not_initialized_again() {
        let mut app = headless_app();
        let entity = spawn_probe(&mut app, true);

        app.update();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<Probe>()
            .unwrap()
            .ran = false;
        app.update();

        assert!(!ran(&app, entity));
    }

    #[test]
    fn a_failure_does_not_stop_the_entity_beside_it() {
        let mut app = headless_app();
        let failing = spawn_probe(&mut app, true);
        let succeeding = spawn_probe(&mut app, false);

        app.update();

        assert!(ran(&app, failing));
        assert!(ran(&app, succeeding));
        assert!(!app
            .world()
            .entity(succeeding)
            .contains::<InitializationFailed>());
    }
}
