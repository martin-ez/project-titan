use crate::common::{initialize_system, Initialize, NeedsInitialization};
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
    fn initialize(&mut self, entity: &Entity, params: &mut MapTileInitializeParams) {
        let (mut transform, mut visibility) = params.query.get_mut(*entity).unwrap();
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
    }
}
