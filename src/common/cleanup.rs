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
        commands.entity(entity).despawn_recursive();
    }
}

fn destroy_on_state_change_system(
    mut commands: Commands,
    query: Query<Entity, With<DestroyOnStateChange>>,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn_recursive();
    }
}
