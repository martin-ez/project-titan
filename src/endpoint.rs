use crate::diagnostics::DebugGizmos;
use crate::map::{HexCoordinates, LatticeNode, MAP_TILE_INRADIUS};
use crate::road::{Road, RoadSegment, RoadTiles};
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

/// How far along a segment its end is, which is where the road node a rover stops at stands.
const SEGMENT_END: f32 = 1.;

/// How far the debug view lifts an endpoint's marks off the ground, so they do not fight the tiles.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.15, 0.);

/// The colour the link from a tile to the road serving it is drawn in
const SERVED_COLOUR: Color = Color::srgb(0.4, 0.95, 0.7);

/// The colour a tile no road reaches is marked in
const UNSERVED_COLOUR: Color = Color::srgb(0.95, 0.4, 0.6);

/// How wide the mark on a tile no road reaches is drawn, as a share of the tile's inradius.
const UNSERVED_MARK: f32 = 0.45;

/// Where the buildings on the map meet the roads on it.
///
/// A road serves a tile when one of its nodes stands on one of that tile's six corners: integer
/// equality on the lattice rather than a distance, so what serves a building is a fact of the grid
/// and answers the same however the arcs curve over it. Everything a building receives arrives on
/// a rover that drove there (invariant 1), and this is where that sentence is joined up: without
/// it a building is scenery and a rover has nowhere to be sent. A building no road reaches is
/// useless rather than illegal — it is placed, and it reports that nothing serves it.
pub struct EndpointPlugin;

/// Where whatever stands on a tile meets the road network.
///
/// It is a segment and a place along it and nothing else: a rover has arrived when it reaches that
/// place, so nothing docks and no approach path is drawn. An endpoint no road reaches carries
/// none, which is what something offering it a delivery has to ask before it makes one.
#[derive(Component)]
pub struct RoadEndpoint {
    tile: HexCoordinates,
    served: Option<ServedBy>,
}

/// The segment serving an endpoint, and how far along it a rover stops.
#[derive(Clone, Copy, Debug)]
pub struct ServedBy {
    /// The segment a rover arrives on.
    pub segment: Entity,
    /// How far along it the endpoint stands, from `0` at its start to `1` at its end.
    pub along: f32,
}

impl Plugin for EndpointPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (connect_the_endpoints, draw_the_endpoints).chain());
    }
}

impl RoadEndpoint {
    /// An endpoint for whatever stands on `tile`, which nothing serves until a road reaches it.
    pub fn on(tile: HexCoordinates) -> Self {
        Self { tile, served: None }
    }

    /// The segment serving it and where along it, or nothing while no road reaches its tile.
    pub fn served_by(&self) -> Option<ServedBy> {
        self.served
    }
}

/// Give every endpoint nothing serves the segment of a road that does.
///
/// One that is already served keeps what it has, so a road laid later does not take a building
/// that is already on the network. One whose segment has gone looks again, which is what a road
/// cut in two leaves behind it.
fn connect_the_endpoints(
    mut endpoints: Query<&mut RoadEndpoint>,
    occupied: Res<RoadTiles>,
    roads: Query<&Road>,
    children: Query<&Children>,
    segments: Query<&RoadSegment>,
) {
    for mut endpoint in &mut endpoints {
        if endpoint
            .served
            .is_some_and(|served| segments.contains(served.segment))
        {
            continue;
        }

        let tile = endpoint.tile;
        endpoint.served = the_road_serving(tile, &occupied, &roads, &children, &segments);
    }
}

/// The place on the network serving `tile`: the first of its corners a road stands on.
///
/// Only the roads over the tile and the six around it are read. A road standing on a corner runs
/// over one of the three tiles sharing that corner, all of which are in that set, so nothing
/// outside it can serve the tile and no road beyond the neighbours is measured.
fn the_road_serving(
    tile: HexCoordinates,
    occupied: &RoadTiles,
    roads: &Query<&Road>,
    children: &Query<&Children>,
    segments: &Query<&RoadSegment>,
) -> Option<ServedBy> {
    LatticeNode::corners_of(tile)
        .into_iter()
        .find_map(|corner| {
            std::iter::once(tile)
                .chain(tile.neighbours())
                .flat_map(|near| occupied.roads_over(near))
                .copied()
                .find(|&road| {
                    roads
                        .get(road)
                        .is_ok_and(|standing| standing.nodes.contains(&corner))
                })
                .and_then(|road| segment_ending_at(corner, road, children, segments))
        })
}

/// The segment of `road` whose end stands on `node`, which is where a rover arriving there stops.
fn segment_ending_at(
    node: LatticeNode,
    road: Entity,
    children: &Query<&Children>,
    segments: &Query<&RoadSegment>,
) -> Option<ServedBy> {
    let standing = node.world_position();
    children
        .get(road)
        .ok()?
        .iter()
        .filter_map(|lane| children.get(lane).ok())
        .flat_map(|lane| lane.iter())
        .filter_map(|segment| {
            segments
                .get(segment)
                .ok()
                .map(|piece| (segment, piece.world_position(SEGMENT_END)))
        })
        .min_by(|(_, ends), (_, other)| {
            ends.distance_squared(standing)
                .total_cmp(&other.distance_squared(standing))
        })
        .map(|(segment, _)| ServedBy {
            segment,
            along: SEGMENT_END,
        })
}

/// Draw what serves each endpoint, and mark the tiles nothing does.
///
/// Whether a building is on the network is otherwise invisible: it stands on its tile looking the
/// same either way, and a road running past it looks like a road serving it (invariant 5).
fn draw_the_endpoints(
    mut gizmos: Gizmos<DebugGizmos>,
    endpoints: Query<&RoadEndpoint>,
    segments: Query<&RoadSegment>,
) {
    for endpoint in &endpoints {
        let standing = endpoint.tile.world_position() + GIZMO_LIFT;
        let served = endpoint.served_by().and_then(|served| {
            segments
                .get(served.segment)
                .ok()
                .map(|segment| segment.world_position(served.along))
        });

        let Some(place) = served else {
            gizmos.circle(
                Isometry3d::new(standing, Quat::from_rotation_x(FRAC_PI_2)),
                MAP_TILE_INRADIUS * UNSERVED_MARK,
                UNSERVED_COLOUR,
            );
            continue;
        };
        gizmos.line(standing, place + GIZMO_LIFT, SERVED_COLOUR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::MAP_TILE_SIZE;
    use crate::road::RoadPlugin;
    use crate::rover::{Rover, RoverPlugin};
    use crate::testing::{headless_app, tick};

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// The tile the endpoint under test stands on, in offset-row coordinates.
    const BUILT_ON: (i32, i32) = (0, 0);

    /// The tile sharing the northern corner of `BUILT_ON`, in offset-row coordinates.
    const BESIDE: (i32, i32) = (0, 1);

    /// The tile east of `BUILT_ON`, which shares two of its corners, in offset-row coordinates.
    const NEXT_DOOR: (i32, i32) = (1, 0);

    /// A tile sharing no corner with `BUILT_ON`, in offset-row coordinates.
    const AWAY: (i32, i32) = (2, 0);

    /// The tile the road that gets cut sets off from, in offset-row coordinates. It stands in line
    /// with `BESIDE` and the corner they serve, so the road it makes runs straight.
    const ALONG: (i32, i32) = (2, 2);

    /// The tile the road doing the cutting sets off from, in offset-row coordinates.
    const ACROSS: (i32, i32) = (1, 1);

    fn app_holding(tool: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(tool)
            .insert_resource(PlayerInput::default())
            .add_plugins((DebugGizmosPlugin, CleanupPlugin, RoadPlugin, EndpointPlugin));
        app
    }

    fn endpoint_app() -> App {
        app_holding(PlayerAction::Select)
    }

    fn tile(offset: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offset.0, offset.1)
    }

    fn centre_of(offset: (i32, i32)) -> LatticeNode {
        LatticeNode::from_tile(tile(offset))
    }

    /// The corner of the tile at `offset` nearest to `towards` of its middle.
    fn corner_of(offset: (i32, i32), towards: Vec3) -> LatticeNode {
        let target = tile(offset).world_position() + towards;
        LatticeNode::corners_of(tile(offset))
            .into_iter()
            .min_by(|node, other| {
                node.world_position()
                    .distance_squared(target)
                    .total_cmp(&other.world_position().distance_squared(target))
            })
            .expect("a tile has six corners")
    }

    /// The corner of `BUILT_ON` the roads in these tests reach it on.
    fn served_corner() -> LatticeNode {
        corner_of(BUILT_ON, Vec3::Z * MAP_TILE_SIZE)
    }

    fn spawn_road(app: &mut App, nodes: Vec<LatticeNode>) -> Entity {
        app.world_mut()
            .spawn(Road {
                nodes,
                leaving: None,
            })
            .id()
    }

    fn spawn_endpoint(app: &mut App, offset: (i32, i32)) -> Entity {
        app.world_mut().spawn(RoadEndpoint::on(tile(offset))).id()
    }

    fn served_by(app: &App, endpoint: Entity) -> Option<ServedBy> {
        app.world()
            .get::<RoadEndpoint>(endpoint)
            .and_then(|endpoint| endpoint.served_by())
    }

    /// Where on the ground the road serving `endpoint` stops for it.
    fn served_at(app: &App, endpoint: Entity) -> Option<Vec3> {
        let served = served_by(app, endpoint)?;
        app.world()
            .get::<RoadSegment>(served.segment)
            .map(|segment| segment.world_position(served.along))
    }

    /// Click on `node`, and take the frame that reads the click.
    fn click_at(app: &mut App, node: LatticeNode) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.cursor_node = Some(node);
            input.tap = true;
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    /// An app holding an endpoint on `BUILT_ON` and a road ending on the corner that serves it.
    fn served_app() -> (App, Entity) {
        let mut app = endpoint_app();
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);
        spawn_road(&mut app, vec![centre_of(BESIDE), served_corner()]);
        tick(&mut app);
        (app, endpoint)
    }

    #[test]
    fn an_endpoint_is_served_by_a_road_standing_on_a_corner_of_its_tile() {
        let (app, endpoint) = served_app();

        assert!(served_by(&app, endpoint).is_some());
    }

    #[test]
    fn an_endpoint_is_served_where_the_road_stands_on_its_tile_s_corner() {
        let (app, endpoint) = served_app();

        let standing = served_at(&app, endpoint).expect("the endpoint is served");
        let corner = served_corner().world_position();

        assert!(
            standing.distance(corner) < TOLERANCE,
            "served at {standing}, not at the corner {corner}"
        );
    }

    #[test]
    fn an_endpoint_no_road_reaches_is_served_by_nothing() {
        let mut app = endpoint_app();
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);
        spawn_road(&mut app, vec![centre_of(NEXT_DOOR), centre_of(AWAY)]);

        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn a_road_through_the_middle_of_a_tile_does_not_serve_what_stands_on_it() {
        let mut app = endpoint_app();
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);
        spawn_road(&mut app, vec![centre_of(BUILT_ON), centre_of(NEXT_DOOR)]);

        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn an_endpoint_is_served_by_a_road_laid_after_it() {
        let mut app = endpoint_app();
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);

        tick(&mut app);
        assert!(
            served_by(&app, endpoint).is_none(),
            "served before any road was laid"
        );

        spawn_road(&mut app, vec![centre_of(BESIDE), served_corner()]);
        tick(&mut app);

        assert!(served_by(&app, endpoint).is_some());
    }

    #[test]
    fn an_endpoint_whose_road_is_removed_reports_that_nothing_serves_it() {
        let mut app = endpoint_app();
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);
        let road = spawn_road(&mut app, vec![centre_of(BESIDE), served_corner()]);
        tick(&mut app);

        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn cutting_the_road_leaves_the_endpoint_served_in_the_same_place() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let endpoint = spawn_endpoint(&mut app, BUILT_ON);
        spawn_road(
            &mut app,
            vec![centre_of(ALONG), centre_of(BESIDE), served_corner()],
        );
        tick(&mut app);
        let before = served_at(&app, endpoint).expect("the endpoint is served");

        click_at(&mut app, centre_of(ACROSS));
        click_at(&mut app, centre_of(BESIDE));
        tick(&mut app);

        let after = served_at(&app, endpoint).expect("the endpoint is served after the cut");
        assert!(
            after.distance(before) < TOLERANCE,
            "served at {after} after the cut, having been served at {before}"
        );
    }

    #[test]
    fn a_rover_standing_where_an_endpoint_is_served_stands_on_the_road_node_serving_it() {
        let (mut app, endpoint) = served_app();
        app.add_plugins(RoverPlugin);
        let served = served_by(&app, endpoint).expect("the endpoint is served");

        let rover = app
            .world_mut()
            .spawn(Rover {
                segment: served.segment,
                along: served.along,
            })
            .id();
        tick(&mut app);

        let standing = app
            .world()
            .get::<Transform>(rover)
            .map(|transform| transform.translation)
            .expect("the rover stands somewhere");
        let corner = served_corner().world_position();

        assert!(
            standing.distance(corner) < TOLERANCE,
            "the rover stands at {standing}, not at the corner {corner}"
        );
    }
}
