use crate::common::cursor::{CursorSurface, TileSurface};
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

const MAP_GRID_SIZE: i32 = 12;
const MAP_GRID_GAP: f32 = 0.2;
const MAP_TILE_SIZE: f32 = 10.;
const SQRT_3: f32 = 1.732_050_8;
const MAP_TILE_WIDTH: f32 = MAP_TILE_SIZE / 2. * SQRT_3;
const MAP_TILE_ROW_SPACING: f32 = MAP_TILE_SIZE * 0.75;
const MAP_TILE_DEPTH: f32 = 0.25;

pub struct MapPlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct HexCoordinates {
    q: i32,
    r: i32,
}

#[derive(Component)]
#[require(
    Transform,
    InheritedVisibility,
    NeedsInitialization,
    TileSurface,
    CursorSurface = tile_surface()
)]
struct MapTile {
    coordinates: HexCoordinates,
}

/// The ground a tile offers the cursor: the whole hex, gap included, lying flat on the tile.
fn tile_surface() -> CursorSurface {
    CursorSurface {
        radius: MAP_TILE_SIZE / 2.,
        height: 0.,
    }
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
    for col in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
        for row in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
            commands.spawn((
                MapTile {
                    coordinates: HexCoordinates::from_offset_row(col, row),
                },
                Visibility::Hidden,
            ));
        }
    }
}

impl HexCoordinates {
    fn from_offset_row(col: i32, row: i32) -> Self {
        Self {
            q: col - (row - row.rem_euclid(2)) / 2,
            r: row,
        }
    }

    fn world_position(&self) -> Vec3 {
        Vec3::new(
            MAP_TILE_WIDTH * (self.q as f32 + self.r as f32 / 2.),
            0.,
            MAP_TILE_ROW_SPACING * self.r as f32,
        )
    }
}

impl Initialize<MapTileInitializeParams<'_, '_>> for MapTile {
    fn initialize(&mut self, entity: &Entity, params: &mut MapTileInitializeParams) -> Result {
        let (mut transform, mut visibility) = params.query.get_mut(*entity)?;
        transform.translation = self.coordinates.world_position();
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
    use std::collections::HashSet;

    const NEIGHBOUR_STEPS: [(i32, i32); 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

    fn map_app() -> App {
        let mut app = headless_app();
        app.add_plugins(MapPlugin);
        app
    }

    fn spawn_tile(app: &mut App, coordinates: HexCoordinates) -> Entity {
        app.world_mut()
            .spawn((MapTile { coordinates }, Visibility::Hidden))
            .id()
    }

    fn translation_of(app: &App, tile: Entity) -> Option<Vec3> {
        app.world()
            .entity(tile)
            .get::<Transform>()
            .map(|t| t.translation)
    }

    #[test]
    fn an_initialized_tile_stands_at_the_world_position_of_its_coordinates() {
        let mut app = map_app();
        let tile = spawn_tile(&mut app, HexCoordinates { q: 1, r: 0 });

        tick(&mut app);

        assert_eq!(
            translation_of(&app, tile),
            Some(Vec3::new(MAP_TILE_WIDTH, 0., 0.))
        );
        let world = app.world();
        assert_eq!(
            world.entity(tile).get::<Visibility>(),
            Some(&Visibility::Visible)
        );
        assert!(!world.entity(tile).contains::<NeedsInitialization>());
        assert!(!world.entity(tile).contains::<InitializationFailed>());
    }

    #[test]
    fn a_tile_offers_the_cursor_a_hexagon_the_size_of_the_grid() {
        let mut app = map_app();
        let tile = spawn_tile(&mut app, HexCoordinates { q: 0, r: 0 });

        tick(&mut app);

        let world = app.world();
        assert!(world.entity(tile).contains::<TileSurface>());
        assert_eq!(
            world.entity(tile).get::<CursorSurface>().map(|s| s.radius),
            Some(MAP_TILE_SIZE / 2.)
        );
    }

    #[test]
    fn a_tile_on_an_odd_row_is_offset_by_half_a_tile_width() {
        let mut app = map_app();
        let tile = spawn_tile(&mut app, HexCoordinates::from_offset_row(0, 1));

        tick(&mut app);

        assert_eq!(
            translation_of(&app, tile),
            Some(Vec3::new(MAP_TILE_WIDTH / 2., 0., MAP_TILE_ROW_SPACING))
        );
    }

    #[test]
    fn an_odd_row_below_the_origin_is_offset_the_same_way() {
        let mut app = map_app();
        let tile = spawn_tile(&mut app, HexCoordinates::from_offset_row(0, -1));

        tick(&mut app);

        assert_eq!(
            translation_of(&app, tile),
            Some(Vec3::new(MAP_TILE_WIDTH / 2., 0., -MAP_TILE_ROW_SPACING))
        );
    }

    #[test]
    fn the_map_spawns_a_tile_for_every_cell_of_the_grid() {
        let mut app = map_app();

        tick(&mut app);

        let corner = HexCoordinates::from_offset_row(-MAP_GRID_SIZE / 2, -MAP_GRID_SIZE / 2);
        let mut query = app.world_mut().query::<(&MapTile, &Transform)>();
        let tiles: Vec<_> = query.iter(app.world()).collect();

        assert_eq!(tiles.len() as i32, MAP_GRID_SIZE * MAP_GRID_SIZE);
        assert_eq!(
            tiles
                .iter()
                .find(|(tile, _)| tile.coordinates == corner)
                .map(|(_, transform)| transform.translation),
            Some(Vec3::new(
                -MAP_GRID_SIZE as f32 / 2. * MAP_TILE_WIDTH,
                0.,
                -MAP_GRID_SIZE as f32 / 2. * MAP_TILE_ROW_SPACING
            ))
        );
    }

    #[test]
    fn the_six_neighbours_of_a_tile_are_one_step_away_in_integers() {
        const TOLERANCE: f32 = 1e-3;

        let centre = HexCoordinates::from_offset_row(2, 3);
        let mut neighbours = HashSet::new();

        for (dq, dr) in NEIGHBOUR_STEPS {
            let neighbour = HexCoordinates {
                q: centre.q + dq,
                r: centre.r + dr,
            };
            let distance = neighbour.world_position().distance(centre.world_position());

            assert!(
                (distance - MAP_TILE_WIDTH).abs() < TOLERANCE,
                "{neighbour:?} stands {distance} from {centre:?}, not {MAP_TILE_WIDTH}"
            );
            neighbours.insert(neighbour);
        }

        assert_eq!(neighbours.len(), NEIGHBOUR_STEPS.len());
    }

    #[test]
    fn a_tile_the_initializer_cannot_read_is_marked_rather_than_panicking() {
        let mut app = map_app();
        let tile = app
            .world_mut()
            .spawn(MapTile {
                coordinates: HexCoordinates { q: 0, r: 0 },
            })
            .id();

        tick(&mut app);

        assert!(app.world().entity(tile).contains::<InitializationFailed>());
    }
}
