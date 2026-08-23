use crate::common::cursor::{CursorSurface, TileSurface};
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

const MAP_GRID_SIZE: i32 = 12;
const MAP_GRID_GAP: f32 = 0.2;
/// The across-corners size of a tile: the diameter of the circle its hexagon fits in.
pub const MAP_TILE_SIZE: f32 = 10.;
const SQRT_3: f32 = 1.732_050_8;
/// The across-flats size of a tile: how far it is from one tile's middle to a neighbour's.
pub const MAP_TILE_WIDTH: f32 = MAP_TILE_SIZE / 2. * SQRT_3;
const MAP_TILE_ROW_SPACING: f32 = MAP_TILE_SIZE * 0.75;
const MAP_TILE_DEPTH: f32 = 0.25;
const NODE_SPACING: f32 = MAP_TILE_SIZE / 2.;
const NODE_BASIS_X: f32 = MAP_TILE_SIZE * SQRT_3 / 4.;

pub struct MapPlugin;

/// Where a tile is: axial hex coordinates, from which a world position is derived.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HexCoordinates {
    q: i32,
    r: i32,
}

/// Where a road's nodes stand: the tile centres and their corners, which together are one lattice.
///
/// The two sets form a single triangular lattice at half the tile size, in which every point has
/// six neighbours at that spacing whether it is a centre or a corner. So a road is placed on the
/// same integers a tile is, at twice the resolution, and invariant 3 holds for a road without a
/// second coordinate system to keep in step with the first.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LatticeNode {
    i: i32,
    j: i32,
}

/// One tile of the grid, standing where its coordinates put it.
#[derive(Component)]
#[require(
    Transform,
    InheritedVisibility,
    NeedsInitialization,
    TileSurface,
    CursorSurface = tile_surface()
)]
pub struct MapTile {
    /// The tile's place on the grid.
    pub coordinates: HexCoordinates,
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

impl LatticeNode {
    /// The node a tile's middle stands on.
    pub fn from_tile(tile: HexCoordinates) -> Self {
        Self {
            i: 2 * tile.q + tile.r,
            j: tile.r - tile.q,
        }
    }

    /// Where on the ground plane this node stands.
    pub fn world_position(&self) -> Vec3 {
        Vec3::new(
            NODE_BASIS_X * self.i as f32,
            0.,
            NODE_SPACING * (self.i as f32 / 2. + self.j as f32),
        )
    }

    /// The node of `tile` nearest to `position`: its middle, or whichever corner is closer.
    ///
    /// Only the seven nodes of one tile are offered, so a cursor over a tile settles on a node of
    /// that tile and never on one belonging to the tile beyond it.
    pub fn nearest_on(tile: HexCoordinates, position: Vec3) -> Self {
        let centre = Self::from_tile(tile);
        centre
            .corners()
            .into_iter()
            .chain(std::iter::once(centre))
            .min_by(|node, other| {
                let strayed = node.world_position().distance_squared(position);
                strayed.total_cmp(&other.world_position().distance_squared(position))
            })
            .unwrap_or(centre)
    }

    fn corners(&self) -> [Self; 6] {
        [(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)].map(|(di, dj)| Self {
            i: self.i + di,
            j: self.j + dj,
        })
    }
}

impl HexCoordinates {
    /// The coordinates of the cell at `col` and `row` of an offset-row layout.
    pub fn from_offset_row(col: i32, row: i32) -> Self {
        Self {
            q: col - (row - row.rem_euclid(2)) / 2,
            r: row,
        }
    }

    /// Where on the ground plane these coordinates put the middle of a tile.
    pub fn world_position(&self) -> Vec3 {
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

    /// The steps to the six nodes around a node of the lattice.
    const NODE_STEPS: [(i32, i32); 6] = [(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)];

    #[test]
    fn a_tile_stands_on_a_node_of_the_lattice() {
        const TOLERANCE: f32 = 1e-3;

        for (col, row) in [(0, 0), (2, 3), (-4, 1), (3, -2)] {
            let tile = HexCoordinates::from_offset_row(col, row);
            let node = LatticeNode::from_tile(tile);

            let strayed = node.world_position().distance(tile.world_position());
            assert!(
                strayed < TOLERANCE,
                "{node:?} stands {strayed} from {tile:?}"
            );
        }
    }

    /// The seven nodes a cursor over `tile` may settle on: its middle and its six corners.
    fn nodes_of(tile: HexCoordinates) -> HashSet<LatticeNode> {
        let centre = LatticeNode::from_tile(tile);
        let mut nodes: HashSet<LatticeNode> = NODE_STEPS
            .iter()
            .map(|&(di, dj)| LatticeNode {
                i: centre.i + di,
                j: centre.j + dj,
            })
            .collect();
        nodes.insert(centre);
        nodes
    }

    #[test]
    fn a_cursor_at_the_middle_of_a_tile_settles_on_the_tile_s_own_node() {
        let tile = HexCoordinates::from_offset_row(2, 3);

        let node = LatticeNode::nearest_on(tile, tile.world_position());

        assert_eq!(node, LatticeNode::from_tile(tile));
    }

    #[test]
    fn a_cursor_by_a_corner_of_a_tile_settles_on_that_corner() {
        let tile = HexCoordinates::from_offset_row(2, 3);
        let centre = LatticeNode::from_tile(tile);
        let corner = LatticeNode {
            i: centre.i + 1,
            j: centre.j,
        };
        let just_inside = corner.world_position()
            + (centre.world_position() - corner.world_position()).normalize() * 0.5;

        assert_eq!(LatticeNode::nearest_on(tile, just_inside), corner);
    }

    #[test]
    fn a_cursor_anywhere_inside_a_tile_settles_on_a_node_of_that_tile() {
        const SAMPLES: i32 = 40;

        let tile = HexCoordinates::from_offset_row(-1, 2);
        let nodes = nodes_of(tile);

        for across in -SAMPLES..=SAMPLES {
            for along in -SAMPLES..=SAMPLES {
                let offset = Vec3::new(
                    across as f32 / SAMPLES as f32 * MAP_TILE_SIZE / 2.,
                    0.,
                    along as f32 / SAMPLES as f32 * MAP_TILE_SIZE / 2.,
                );
                let position = tile.world_position() + offset;

                let settled = LatticeNode::nearest_on(tile, position);

                assert!(
                    nodes.contains(&settled),
                    "{settled:?} is not a node of {tile:?}, from {position:?}"
                );
            }
        }
    }

    #[test]
    fn a_cursor_settles_on_the_nearest_of_a_tile_s_nodes() {
        const SAMPLES: i32 = 20;

        let tile = HexCoordinates::from_offset_row(0, 0);
        let nodes = nodes_of(tile);

        for across in -SAMPLES..=SAMPLES {
            for along in -SAMPLES..=SAMPLES {
                let position = tile.world_position()
                    + Vec3::new(
                        across as f32 / SAMPLES as f32 * MAP_TILE_SIZE / 2.,
                        0.,
                        along as f32 / SAMPLES as f32 * MAP_TILE_SIZE / 2.,
                    );

                let settled = LatticeNode::nearest_on(tile, position);
                let strayed = settled.world_position().distance(position);

                for node in &nodes {
                    assert!(
                        node.world_position().distance(position) >= strayed - 1e-3,
                        "{node:?} is nearer {position:?} than {settled:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_six_nodes_around_a_tile_are_its_corners() {
        const TOLERANCE: f32 = 1e-3;

        let centre = LatticeNode::from_tile(HexCoordinates::from_offset_row(2, 3));
        let mut corners = HashSet::new();

        for (di, dj) in NODE_STEPS {
            let corner = LatticeNode {
                i: centre.i + di,
                j: centre.j + dj,
            };
            let distance = corner.world_position().distance(centre.world_position());

            assert!(
                (distance - MAP_TILE_SIZE / 2.).abs() < TOLERANCE,
                "{corner:?} stands {distance} from {centre:?}, not {}",
                MAP_TILE_SIZE / 2.
            );
            corners.insert(corner);
        }

        assert_eq!(corners.len(), NODE_STEPS.len());
    }
}
