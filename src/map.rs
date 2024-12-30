use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

const MAP_GRID_SIZE: i16 = 12;
const MAP_GRID_GAP: f32 = 0.2;
const MAP_TILE_SIZE: f32 = 10.;
const MAP_TILE_WIDTH: f32 = MAP_TILE_SIZE / 2. * 1.73205080757; // sqrt(3)
const MAP_TILE_DEPTH: f32 = 0.25;

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(PostStartup, translate_tiles);
    }
}

#[derive(Component)]
#[require(Transform, InheritedVisibility)]
struct MapTile {
    coordinates: Vec2,
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

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for x in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
        for y in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
            commands
                .spawn(MapTile {
                    coordinates: Vec2::new(x as f32, y as f32),
                })
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(
                            meshes.add(Extrusion::new(RegularPolygon::default(), MAP_TILE_DEPTH)),
                        ),
                        MeshMaterial3d(materials.add(Color::srgb(0.98, 0.66, 0.46))),
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
}

fn translate_tiles(mut tiles_q: Query<(&mut Transform, &MapTile)>) {
    for (mut transform, tile) in tiles_q.iter_mut() {
        transform.translation = tile.world_position();
    }
}
