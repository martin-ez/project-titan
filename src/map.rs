use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

const MAP_GRID_SIZE: i16 = 12;
const MAP_GRID_GAP: f32 = 0.2;
const MAP_TILE_SIZE: f32 = 10.;
const SQRT_3: f32 = 1.732_050_8;
const MAP_TILE_WIDTH: f32 = MAP_TILE_SIZE / 2. * SQRT_3;
const MAP_TILE_DEPTH: f32 = 0.25;

pub struct MapPlugin;

#[derive(Component)]
#[require(Transform, InheritedVisibility, NeedsInitialization)]
struct MapTile {
    coordinates: Vec2,
}

#[derive(SystemParam)]
struct MapTileInitializeParams<'w, 's> {
    query: Query<'w, 's, (&'static mut Transform, &'static mut Visibility), With<MapTile>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(
            PreUpdate,
            initialize_system::<MapTile, MapTileInitializeParams>,
        );
    }
}

fn setup(mut commands: Commands) {
    for x in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
        for y in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
            commands.spawn((
                MapTile {
                    coordinates: Vec2::new(x as f32, y as f32),
                },
                Visibility::Hidden,
            ));
        }
    }
}

impl MapTile {
    fn world_position(&self) -> Vec3 {
        let offset = if self.coordinates.y as i16 % 2 == 0 {
            0.
        } else {
            MAP_TILE_WIDTH / 2.
        };

        Vec3::new(
            (self.coordinates.x * MAP_TILE_WIDTH) + offset,
            0.,
            self.coordinates.y * MAP_TILE_SIZE * 0.75,
        )
    }
}

impl Initialize<MapTileInitializeParams<'_, '_>> for MapTile {
    fn initialize(&mut self, entity: &Entity, params: &mut MapTileInitializeParams) -> Result {
        let (mut transform, mut visibility) = params.query.get_mut(*entity)?;
        transform.translation = self.world_position();
        *visibility = Visibility::Visible;

        params.commands.entity(*entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(
                    params
                        .meshes
                        .add(Extrusion::new(RegularPolygon::default(), MAP_TILE_DEPTH)),
                ),
                MeshMaterial3d(params.materials.add(Color::srgb(0.98, 0.66, 0.46))),
                Transform::from_rotation(Quat::from_rotation_x(FRAC_PI_2))
                    .with_translation(Vec3::new(0., -MAP_TILE_DEPTH / 2., 0.))
                    .with_scale(Vec3::new(
                        MAP_TILE_SIZE - MAP_GRID_GAP,
                        MAP_TILE_SIZE - MAP_GRID_GAP,
                        1.,
                    )),
            ));
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::initialize::InitializationFailed;
    use crate::testing::{headless_app, tick};

    fn map_app() -> App {
        let mut app = headless_app();
        app.add_plugins(MapPlugin);
        app
    }

    fn spawn_tile(app: &mut App, x: f32, y: f32) -> Entity {
        app.world_mut()
            .spawn((
                MapTile {
                    coordinates: Vec2::new(x, y),
                },
                Visibility::Hidden,
            ))
            .id()
    }

    #[test]
    fn an_initialized_tile_stands_at_the_world_position_of_its_coordinates() {
        let mut app = map_app();
        let tile = spawn_tile(&mut app, 1., 0.);

        tick(&mut app);

        let world = app.world();
        assert_eq!(
            world.entity(tile).get::<Transform>().map(|t| t.translation),
            Some(Vec3::new(MAP_TILE_WIDTH, 0., 0.))
        );
        assert_eq!(
            world.entity(tile).get::<Visibility>(),
            Some(&Visibility::Visible)
        );
        assert!(!world.entity(tile).contains::<NeedsInitialization>());
        assert!(!world.entity(tile).contains::<InitializationFailed>());
    }

    #[test]
    fn a_tile_the_initializer_cannot_read_is_marked_rather_than_panicking() {
        let mut app = map_app();
        let tile = app
            .world_mut()
            .spawn(MapTile {
                coordinates: Vec2::ZERO,
            })
            .id();

        tick(&mut app);

        assert!(app.world().entity(tile).contains::<InitializationFailed>());
    }
}
