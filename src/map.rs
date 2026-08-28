//! The hex grid the game is played on, and what a tile of it holds.
//!
//! A tile is located by integer coordinates and its world position is derived from them
//! (invariant 3), and a road's nodes stand on the same integers at twice the resolution.
//!
//! A tile may also carry a `Deposit`: a raw material in the ground, which never runs out. Titan
//! is a terraforming builder rather than a survival game, and its production tree is a standing
//! chain rather than a race, so a deposit is ground the player builds around and keeps building
//! around rather than ground they exhaust and walk away from. What a deposit says instead of a
//! reserve is how rich it is — how much it yields, which is a standing property of the ground.

use crate::common::cursor::{CursorSurface, TileSurface};
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

const MAP_GRID_SIZE: i32 = 12;
const MAP_GRID_GAP: f32 = 0.2;
/// The across-corners size of a tile: the diameter of the circle its hexagon fits in.
pub const MAP_TILE_SIZE: f32 = 10.;
const SQRT_3: f32 = 1.732_050_8;
const MAP_TILE_WIDTH: f32 = MAP_TILE_SIZE / 2. * SQRT_3;
const MAP_TILE_ROW_SPACING: f32 = MAP_TILE_SIZE * 0.75;
const MAP_TILE_DEPTH: f32 = 0.25;
const NODE_SPACING: f32 = MAP_TILE_SIZE / 2.;
const NODE_BASIS_X: f32 = MAP_TILE_SIZE * SQRT_3 / 4.;

/// How far it is from the middle of a tile to the middle of any of its six edges.
pub const MAP_TILE_INRADIUS: f32 = MAP_TILE_WIDTH / 2.;

/// How far the debug view lifts the mark on a deposit, so it does not fight the tile it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// How far the mark on a deposit of richness one reaches, as a share of the tile's inradius.
const DEPOSIT_MARK: f32 = 0.25;

const ICE_COLOUR: Color = Color::srgb(0.75, 0.95, 1.);
const CARBON_MONOXIDE_COLOUR: Color = Color::srgb(0.3, 0.3, 0.34);
const NITROGEN_COLOUR: Color = Color::srgb(0.55, 0.45, 0.95);
const SILICON_COLOUR: Color = Color::srgb(0.3, 0.75, 0.4);
const COBALT_ORE_COLOUR: Color = Color::srgb(0.15, 0.3, 0.9);

/// The deposits the starting map holds: a tile's offset column and row, its material, and how
/// rich it is.
///
/// Laid out rather than generated, because a map that differs between two runs of the same save
/// takes back the determinism the simulation is built on (invariant 2). Ice is commonest and
/// cobalt ore rarest, which is the order the production tree reaches them in.
const STARTING_DEPOSITS: [(i32, i32, RawMaterial, u32); 11] = [
    (-4, -4, RawMaterial::Ice, 3),
    (2, -5, RawMaterial::Ice, 3),
    (-1, 3, RawMaterial::Ice, 3),
    (4, 1, RawMaterial::Ice, 2),
    (-5, 1, RawMaterial::CarbonMonoxide, 2),
    (3, -2, RawMaterial::CarbonMonoxide, 2),
    (0, -3, RawMaterial::Nitrogen, 2),
    (-3, 4, RawMaterial::Nitrogen, 2),
    (1, 1, RawMaterial::Silicon, 2),
    (-2, -1, RawMaterial::Silicon, 1),
    (5, 4, RawMaterial::CobaltOre, 1),
];

pub struct MapPlugin;

/// A raw material lying in the ground, which every chain of the production tree starts at.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RawMaterial {
    Ice,
    CarbonMonoxide,
    Nitrogen,
    Silicon,
    CobaltOre,
}

impl RawMaterial {
    /// What the player is told this material is called.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ice => "Ice",
            Self::CarbonMonoxide => "Carbon Monoxide",
            Self::Nitrogen => "Nitrogen",
            Self::Silicon => "Silicon",
            Self::CobaltOre => "Cobalt Ore",
        }
    }
}

/// The raw material a tile holds, and how much of it the ground gives up.
///
/// Carried by the `MapTile` it lies under, so a tile with none answers for itself rather than for
/// a neighbour. A deposit never runs out; `richness` is what it yields, not what is left of it.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deposit {
    /// Which raw material is in the ground here.
    pub material: RawMaterial,
    /// How much this ground gives up, richer being more.
    pub richness: u32,
}

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

/// One of the six corners of a tile, named by the way it lies from the tile's middle.
///
/// A tile is pointy-topped, so two of its corners face due north and south and the other four sit
/// off its sides. Nothing on the map is turned before it is placed, so a corner named here is the
/// same corner of every tile there is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TileCorner {
    North,
    NorthEast,
    SouthEast,
    South,
    SouthWest,
    NorthWest,
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
        app.add_systems(Startup, setup)
            .add_systems(
                PreUpdate,
                initialize_system::<MapTile, MapTileInitializeParams>,
            )
            .add_systems(Update, draw_the_deposits);
    }
}

fn setup(mut commands: Commands) {
    for col in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
        for row in -MAP_GRID_SIZE / 2..MAP_GRID_SIZE / 2 {
            let coordinates = HexCoordinates::from_offset_row(col, row);
            let tile = commands
                .spawn((MapTile { coordinates }, Visibility::Hidden))
                .id();

            if let Some(deposit) = starting_deposit_on(coordinates) {
                commands.entity(tile).insert(deposit);
            }
        }
    }
}

fn starting_deposit_on(tile: HexCoordinates) -> Option<Deposit> {
    STARTING_DEPOSITS
        .iter()
        .find(|(col, row, _, _)| HexCoordinates::from_offset_row(*col, *row) == tile)
        .map(|(_, _, material, richness)| Deposit {
            material: *material,
            richness: *richness,
        })
}

/// Mark every deposit on the map, in the colour of its material and at the size of its richness.
///
/// A deposit is a fact of the ground with nothing standing on it to show it, so without this the
/// grid reads as bare tiles and where the player builds looks arbitrary. What one looks like past
/// a ring is presentation work, and not this (invariant 5).
fn draw_the_deposits(mut gizmos: Gizmos<DebugGizmos>, deposits: Query<(&MapTile, &Deposit)>) {
    for (tile, deposit) in &deposits {
        gizmos.circle(
            Isometry3d::new(
                tile.coordinates.world_position() + GIZMO_LIFT,
                Quat::from_rotation_x(FRAC_PI_2),
            ),
            MAP_TILE_INRADIUS * DEPOSIT_MARK * deposit.richness as f32,
            colour_of(deposit.material),
        );
    }
}

fn colour_of(material: RawMaterial) -> Color {
    match material {
        RawMaterial::Ice => ICE_COLOUR,
        RawMaterial::CarbonMonoxide => CARBON_MONOXIDE_COLOUR,
        RawMaterial::Nitrogen => NITROGEN_COLOUR,
        RawMaterial::Silicon => SILICON_COLOUR,
        RawMaterial::CobaltOre => COBALT_ORE_COLOUR,
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
            .around()
            .into_iter()
            .chain(std::iter::once(centre))
            .min_by(|node, other| {
                let strayed = node.world_position().distance_squared(position);
                strayed.total_cmp(&other.world_position().distance_squared(position))
            })
            .unwrap_or(centre)
    }

    /// The three tiles this node is a corner of, or nothing when it is a tile's own middle.
    ///
    /// The three are found by arithmetic rather than by measuring what is near: a node is a
    /// middle when `i - j` divides by three, so the tiles sharing a corner are whichever of the
    /// six nodes around it are middles, and there are always three of them (invariant 3).
    pub fn tiles_sharing(&self) -> Option<[HexCoordinates; 3]> {
        let mut sharing = self
            .around()
            .into_iter()
            .filter_map(|node| node.middle_of());
        Some([sharing.next()?, sharing.next()?, sharing.next()?])
    }

    fn middle_of(&self) -> Option<HexCoordinates> {
        ((self.i - self.j).rem_euclid(3) == 0).then(|| HexCoordinates {
            q: (self.i - self.j) / 3,
            r: (self.i + 2 * self.j) / 3,
        })
    }

    fn around(&self) -> [Self; 6] {
        TileCorner::ALL.map(|corner| corner.step_from(*self))
    }
}

impl TileCorner {
    /// The six corners of a tile, in the order they come round it from due north.
    pub const ALL: [Self; 6] = [
        Self::North,
        Self::NorthEast,
        Self::SouthEast,
        Self::South,
        Self::SouthWest,
        Self::NorthWest,
    ];

    /// The lattice node this corner of `tile` stands on.
    pub fn node_of(self, tile: HexCoordinates) -> LatticeNode {
        self.step_from(LatticeNode::from_tile(tile))
    }

    fn step_from(self, node: LatticeNode) -> LatticeNode {
        let (di, dj) = match self {
            Self::North => (0, 1),
            Self::NorthEast => (1, 0),
            Self::SouthEast => (1, -1),
            Self::South => (0, -1),
            Self::SouthWest => (-1, 0),
            Self::NorthWest => (-1, 1),
        };
        LatticeNode {
            i: node.i + di,
            j: node.j + dj,
        }
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

    /// The tile the point `position` stands on, whatever height it stands at.
    ///
    /// The inverse of `world_position`. A point of the plane always stands on one, and one on an
    /// edge or a corner stands on whichever of the tiles sharing it the rounding settles for, so
    /// what this answers is which tile to hold a road against rather than which it is inside.
    pub fn from_world_position(position: Vec3) -> Self {
        let r = position.z / MAP_TILE_ROW_SPACING;
        Self::nearest(position.x / MAP_TILE_WIDTH - r / 2., r)
    }

    fn nearest(q: f32, r: f32) -> Self {
        let s = -q - r;
        let (mut rounded_q, mut rounded_r, rounded_s) = (q.round(), r.round(), s.round());
        let (strayed_q, strayed_r, strayed_s) = (
            (rounded_q - q).abs(),
            (rounded_r - r).abs(),
            (rounded_s - s).abs(),
        );

        if strayed_q > strayed_r && strayed_q > strayed_s {
            rounded_q = -rounded_r - rounded_s;
        } else if strayed_r > strayed_s {
            rounded_r = -rounded_q - rounded_s;
        }

        Self {
            q: rounded_q as i32,
            r: rounded_r as i32,
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
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::testing::{headless_app, tick};
    use std::collections::{HashMap, HashSet};

    fn map_app() -> App {
        let mut app = headless_app();
        app.add_plugins((DebugGizmosPlugin, MapPlugin));
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
    fn the_six_corners_of_a_tile_stand_at_the_corners_of_its_hexagon() {
        const TOLERANCE: f32 = 1e-3;

        let tile = HexCoordinates::from_offset_row(2, 3);
        let corners: HashSet<LatticeNode> = TileCorner::ALL
            .map(|corner| corner.node_of(tile))
            .into_iter()
            .collect();

        for corner in TileCorner::ALL.map(|corner| corner.node_of(tile)) {
            let distance = corner.world_position().distance(tile.world_position());
            let across_corners = MAP_TILE_SIZE / 2.;

            assert!(
                (distance - across_corners).abs() < TOLERANCE,
                "{corner:?} stands {distance} from {tile:?}, not {across_corners}"
            );
        }

        assert_eq!(corners.len(), 6);
    }

    /// Which side of the middle a corner lies on, along one axis, with `+x` east and `+z` north.
    fn side_of(reach: f32) -> i32 {
        const TOLERANCE: f32 = 1e-3;

        if reach.abs() < TOLERANCE {
            0
        } else {
            reach.signum() as i32
        }
    }

    #[test]
    fn a_corner_named_for_a_direction_stands_that_way_from_the_middle_of_its_tile() {
        let tile = HexCoordinates::from_offset_row(2, 3);
        let middle = tile.world_position();

        for (corner, east, north) in [
            (TileCorner::North, 0, 1),
            (TileCorner::NorthEast, 1, 1),
            (TileCorner::SouthEast, 1, -1),
            (TileCorner::South, 0, -1),
            (TileCorner::SouthWest, -1, -1),
            (TileCorner::NorthWest, -1, 1),
        ] {
            let strayed = corner.node_of(tile).world_position() - middle;

            assert_eq!(
                (side_of(strayed.x), side_of(strayed.z)),
                (east, north),
                "{corner:?} lies {strayed} from the middle of {tile:?}"
            );
        }
    }

    #[test]
    fn the_three_tiles_sharing_a_corner_each_name_it_as_a_corner_of_their_own() {
        let tile = HexCoordinates::from_offset_row(2, 3);

        for corner in TileCorner::ALL {
            let node = corner.node_of(tile);
            let sharing = node
                .tiles_sharing()
                .expect("a corner is shared by three tiles");

            assert!(sharing.contains(&tile), "{corner:?} left out its own tile");
            for shared in sharing {
                assert!(
                    TileCorner::ALL
                        .iter()
                        .any(|named| named.node_of(shared) == node),
                    "{shared:?} shares {corner:?} of {tile:?} without naming it"
                );
            }
        }
    }

    #[test]
    fn the_middle_of_a_tile_is_shared_by_no_tiles_at_all() {
        let tile = HexCoordinates::from_offset_row(2, 3);

        assert!(LatticeNode::from_tile(tile).tiles_sharing().is_none());
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
    #[test]
    fn a_world_position_reports_the_tile_it_stands_in() {
        for (col, row) in [(0, 0), (2, 3), (-4, 1), (3, -2)] {
            let tile = HexCoordinates::from_offset_row(col, row);

            assert_eq!(
                HexCoordinates::from_world_position(tile.world_position()),
                tile
            );
        }
    }

    #[test]
    fn a_world_position_past_a_tiles_edge_stands_in_the_next_tile() {
        /// How far either side of the edge the two positions are taken, as a share of the reach
        /// to it.
        const STRIDE: f32 = 0.01;

        let tile = HexCoordinates::from_offset_row(2, 3);
        let beyond = HexCoordinates {
            q: tile.q + 1,
            r: tile.r,
        };
        let towards = Vec3::new(MAP_TILE_INRADIUS, 0., 0.);

        assert_eq!(
            HexCoordinates::from_world_position(tile.world_position() + towards * (1. - STRIDE)),
            tile
        );
        assert_eq!(
            HexCoordinates::from_world_position(tile.world_position() + towards * (1. + STRIDE)),
            beyond
        );
    }

    /// A tile the starting map puts ice on, in offset column and row.
    const ICE_TILE: (i32, i32) = (-4, -4);

    /// A tile beside `ICE_TILE` that the starting map leaves bare, so neither answers for the
    /// other.
    const BARE_TILE: (i32, i32) = (-3, -4);

    fn deposits(app: &mut App) -> HashMap<HexCoordinates, Deposit> {
        let mut query = app.world_mut().query::<(&MapTile, &Deposit)>();
        query
            .iter(app.world())
            .map(|(tile, deposit)| (tile.coordinates, *deposit))
            .collect()
    }

    fn started_map() -> App {
        let mut app = map_app();
        tick(&mut app);
        app
    }

    fn offset(tile: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(tile.0, tile.1)
    }

    #[test]
    fn a_tile_carrying_a_deposit_says_which_material_it_holds() {
        let mut app = started_map();

        let held = deposits(&mut app);

        assert_eq!(
            held.get(&offset(ICE_TILE)).map(|deposit| deposit.material),
            Some(RawMaterial::Ice)
        );
    }

    #[test]
    fn a_tile_carrying_a_deposit_says_how_rich_it_is() {
        let mut app = started_map();

        let held = deposits(&mut app);

        assert_eq!(
            held.get(&offset(ICE_TILE)).map(|deposit| deposit.richness),
            Some(3)
        );
    }

    #[test]
    fn a_tile_beside_a_deposit_carries_none() {
        let mut app = started_map();
        let bare = offset(BARE_TILE);

        let held = deposits(&mut app);

        assert!(
            !held.contains_key(&bare),
            "{bare:?} answered {:?} for its neighbour",
            held.get(&bare)
        );
    }

    #[test]
    fn the_starting_map_offers_every_raw_material() {
        let mut app = started_map();

        let held: HashSet<RawMaterial> = deposits(&mut app)
            .values()
            .map(|deposit| deposit.material)
            .collect();

        for material in [
            RawMaterial::Ice,
            RawMaterial::CarbonMonoxide,
            RawMaterial::Nitrogen,
            RawMaterial::Silicon,
            RawMaterial::CobaltOre,
        ] {
            assert!(
                held.contains(&material),
                "no chain can start at {material:?}"
            );
        }
    }

    #[test]
    fn the_same_starting_map_comes_up_on_every_run() {
        let mut one = started_map();
        let mut again = started_map();

        assert_eq!(deposits(&mut one), deposits(&mut again));
    }

    #[test]
    fn a_deposit_stands_where_its_tile_stands() {
        let mut app = started_map();
        let ice = offset(ICE_TILE);

        let mut query = app.world_mut().query::<(&MapTile, &Transform, &Deposit)>();
        let standing = query
            .iter(app.world())
            .find(|(tile, _, _)| tile.coordinates == ice)
            .map(|(_, transform, _)| transform.translation);

        assert_eq!(standing, Some(ice.world_position()));
    }
}
