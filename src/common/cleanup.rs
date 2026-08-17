use crate::input::PlayerAction;
use bevy::prelude::*;

pub struct CleanupPlugin;

/// Marker component for entities that should be destroyed as soon as possible.
#[derive(Component)]
pub struct Destroy;

/// Marker component for entities that should be destroyed when the game state changes.
#[derive(Component)]
pub struct DestroyOnStateChange;

impl Plugin for CleanupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, destroy_entities_system)
            .add_systems(OnExit(PlayerAction::Select), destroy_on_state_change_system)
            .add_systems(
                OnExit(PlayerAction::EditRoads),
                destroy_on_state_change_system,
            )
            .add_systems(
                OnExit(PlayerAction::EditBuildings),
                destroy_on_state_change_system,
            );
    }
}

fn destroy_entities_system(mut commands: Commands, query: Query<Entity, With<Destroy>>) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}

fn destroy_on_state_change_system(
    mut commands: Commands,
    query: Query<Entity, With<DestroyOnStateChange>>,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{headless_app, tick};

    fn cleanup_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::Select)
            .add_plugins(CleanupPlugin);
        app
    }

    fn change_the_player_action(app: &mut App) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::EditRoads);
    }

    fn still_there(app: &App, entity: Entity) -> bool {
        app.world().entities().contains(entity)
    }

    #[test]
    fn an_entity_marked_for_destruction_does_not_survive_the_frame() {
        let mut app = cleanup_app();
        let entity = app.world_mut().spawn(Destroy).id();

        tick(&mut app);

        assert!(!still_there(&app, entity));
    }

    #[test]
    fn an_entity_bound_to_the_player_action_survives_until_it_changes() {
        let mut app = cleanup_app();
        let entity = app.world_mut().spawn(DestroyOnStateChange).id();

        tick(&mut app);
        tick(&mut app);

        assert!(still_there(&app, entity));
    }

    #[test]
    fn changing_the_player_action_destroys_what_was_bound_to_it() {
        let mut app = cleanup_app();
        let entity = app.world_mut().spawn(DestroyOnStateChange).id();
        tick(&mut app);

        change_the_player_action(&mut app);
        tick(&mut app);

        assert!(!still_there(&app, entity));
    }

    #[test]
    fn changing_the_player_action_leaves_an_unmarked_entity_alone() {
        let mut app = cleanup_app();
        let entity = app.world_mut().spawn_empty().id();
        tick(&mut app);

        change_the_player_action(&mut app);
        tick(&mut app);

        assert!(still_there(&app, entity));
    }
}
