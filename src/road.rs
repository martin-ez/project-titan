use crate::common::cleanup::DestroyOnStateChange;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{HexCoordinates, MapTile};
use bevy::ecs::system::SystemParam;
use bevy::math::cubic_splines::InsufficientDataError;
use bevy::prelude::*;
use std::collections::HashSet;

/// How many straight pieces a segment's spline is drawn as.
const SEGMENT_SUBDIVISIONS: u32 = 8;

/// How far into a segment the arrow onto the next one reaches, at either end of the handover.
const HANDOVER_REACH: f32 = 0.1;

/// How far the debug view lifts a lane off the ground, so it does not fight the tiles it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a lane's spline is drawn in
const LANE_COLOUR: Color = Color::srgb(0.35, 0.75, 0.95);

/// The colour the step from one segment onto the next is drawn in
const HANDOVER_COLOUR: Color = Color::srgb(0.95, 0.8, 0.3);

/// The colour the road the player is still dragging out is drawn in
const DRAWING_COLOUR: Color = Color::srgb(0.6, 0.95, 0.6);

/// The roads on the map, and the lanes a rover drives on them.
///
/// A road carries one lane in each direction, built together and removed together, and the two
/// join at each end so a dead-end spur is drivable. Nothing overtakes anywhere in the network:
/// there is no lane to move into, so a slow rover is everyone's problem and one badly placed
/// building is a queue you can watch form. One lane shared both ways was cheaper and made traffic
/// a decoration; several each way bought overtaking and spent it softening the jams the game is
/// for; making the player draw the return leg charged the saving to the first thing they build.
pub struct RoadPlugin;

/// A road the player drew: the tiles it runs through, in the order it crossed them.
///
/// The tiles are the road. Its spline, the lanes over it and every world position a rover ever
/// stands at are derived from them, so two roads drawn through the same tiles are the same shape.
#[derive(Component)]
#[require(NeedsInitialization)]
pub struct Road {
    /// The tiles the road was drawn through, from one end to the other.
    pub path: Vec<HexCoordinates>,
}

/// The road the player is part way through dragging out, as far as the cursor has taken it.
///
/// It is a record of tiles and nothing else until the button comes up: no lane, no segment and
/// nothing a rover could drive. Putting the tool down destroys it with the rest of the tool's
/// state, so a drag abandoned half way leaves the network as it was.
#[derive(Component)]
#[require(DestroyOnStateChange)]
struct DrawnRoad {
    path: Vec<HexCoordinates>,
}

/// One direction of travel along a road, owning the segments that make it up.
#[derive(Component)]
struct Lane;

/// A stretch of one lane: the piece of the road's spline a rover drives in one go.
#[derive(Component)]
pub struct RoadSegment {
    curve: CubicSegment<Vec3>,
}

/// The segment a rover leaving this one drives onto next.
#[derive(Component)]
#[relationship(relationship_target = PreviousSegments)]
pub struct NextSegment(pub Entity);

/// The segments that lead onto this one, which is more than one where lanes meet.
#[derive(Component)]
#[relationship_target(relationship = NextSegment)]
pub struct PreviousSegments(Vec<Entity>);

#[derive(SystemParam)]
struct RoadInitializeParams<'w, 's> {
    commands: Commands<'w, 's>,
}

impl Plugin for RoadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, initialize_system::<Road, RoadInitializeParams>)
            .add_systems(
                Update,
                (
                    (extend_the_drawn_road, lay_the_drawn_road).chain(),
                    draw_the_lanes,
                    draw_the_drawn_road,
                ),
            );
    }
}

impl RoadSegment {
    /// Where on the ground a rover `along` of the way down this segment stands.
    pub fn world_position(&self, along: f32) -> Vec3 {
        self.curve.position(along.clamp(0., 1.))
    }
}

impl Initialize<RoadInitializeParams<'_, '_>> for Road {
    fn initialize(&mut self, entity: &Entity, params: &mut RoadInitializeParams) -> Result {
        let along = spline(self.path.iter().copied())?;
        let back = spline(self.path.iter().copied().rev())?;

        let along = spawn_lane(&mut params.commands, *entity, &along)?;
        let back = spawn_lane(&mut params.commands, *entity, &back)?;

        params
            .commands
            .entity(along.last)
            .insert(NextSegment(back.first));
        params
            .commands
            .entity(back.last)
            .insert(NextSegment(along.first));
        Ok(())
    }
}

/// The curve running through the tiles of `path`, one cubic per step between two of them.
///
/// Catmull-Rom, so the road bends through a turn rather than cornering, and the tiles it was drawn
/// through are on it. Adjacent tiles all stand the same distance apart, which is what leaves the
/// cubics of one road roughly the same length without resampling them.
fn spline(
    path: impl Iterator<Item = HexCoordinates>,
) -> std::result::Result<CubicCurve<Vec3>, InsufficientDataError> {
    let points: Vec<Vec3> = path.map(|tile| tile.world_position()).collect();
    CubicCardinalSpline::new_catmull_rom(points).to_curve()
}

/// The two segments of a lane a road's other lane joins onto.
struct LaneEnds {
    first: Entity,
    last: Entity,
}

/// Put one direction of travel on `road`: a lane, and a segment of it per cubic of `curve`.
fn spawn_lane(commands: &mut Commands, road: Entity, curve: &CubicCurve<Vec3>) -> Result<LaneEnds> {
    let lane = commands.spawn((Lane, ChildOf(road))).id();
    let mut ends: Option<LaneEnds> = None;

    for &cubic in curve.segments() {
        let segment = commands
            .spawn((RoadSegment { curve: cubic }, ChildOf(lane)))
            .id();
        match ends {
            Some(ref mut ends) => {
                commands.entity(ends.last).insert(NextSegment(segment));
                ends.last = segment;
            }
            None => {
                ends = Some(LaneEnds {
                    first: segment,
                    last: segment,
                })
            }
        }
    }

    ends.ok_or_else(|| "a lane of no segments".into())
}

/// Take the road the player is dragging out as far as the tile under the cursor.
///
/// The path grows by the straight run from the tile it had reached, so the road a drag leaves is
/// the one the cursor crossed rather than the one it was sampled on: a flick that skipped three
/// tiles lays the same road as a slow drag over them, and a cursor resting on one adds nothing.
fn extend_the_drawn_road(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    tiles: Query<&MapTile>,
    mut drawn: Query<&mut DrawnRoad>,
) {
    if !player_input.dragging || *action.get() != PlayerAction::EditRoads {
        return;
    }
    let Some(reached) = player_input
        .cursor_tile
        .and_then(|tile| tiles.get(tile).ok())
        .map(|tile| tile.coordinates)
    else {
        return;
    };

    match drawn.iter_mut().next() {
        Some(mut drawn) => {
            if let Some(last) = drawn.path.last().copied() {
                drawn.path.extend(last.line_to(reached));
            }
        }
        None => {
            commands.spawn(DrawnRoad {
                path: vec![reached],
            });
        }
    }
}

/// Put the drawn road into the world when the player lets the button go.
///
/// A drag that never left its tile lays nothing, which is what a click with the road tool is.
fn lay_the_drawn_road(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    drawn: Query<(Entity, &DrawnRoad)>,
    roads: Query<(Entity, &Road)>,
) {
    if player_input.dragging {
        return;
    }

    for (entity, drawing) in &drawn {
        commands.entity(entity).despawn();

        let meetings = tiles_shared_with(&drawing.path, &roads);
        for path in split_at(&drawing.path, &meetings) {
            commands.spawn(Road { path });
        }
        for (crossed, road) in &roads {
            let pieces = split_at(&road.path, &meetings);
            if pieces.len() < 2 {
                continue;
            }
            commands.entity(crossed).despawn();
            for path in pieces {
                commands.spawn(Road { path });
            }
        }
    }
}

/// The tiles of `path` that a road already runs through.
fn tiles_shared_with(
    path: &[HexCoordinates],
    roads: &Query<(Entity, &Road)>,
) -> HashSet<HexCoordinates> {
    let drawn: HashSet<HexCoordinates> = path.iter().copied().collect();
    roads
        .iter()
        .flat_map(|(_, road)| road.path.iter().copied())
        .filter(|tile| drawn.contains(tile))
        .collect()
}

/// Break `path` into the roads it becomes once cut at every tile in `at`.
///
/// A cut tile ends the piece before it and starts the piece after, so the roads either side meet
/// there rather than running through: that shared end is what makes the tile a place a rover has
/// to be handed over at. A cut at one of `path`'s own ends leaves it whole, being where it already
/// ended, and a piece of a single tile is no road at all and is dropped.
fn split_at(path: &[HexCoordinates], at: &HashSet<HexCoordinates>) -> Vec<Vec<HexCoordinates>> {
    let mut pieces = Vec::new();
    let mut piece: Vec<HexCoordinates> = Vec::new();

    for &tile in path {
        piece.push(tile);
        if at.contains(&tile) && piece.len() > 1 {
            pieces.push(std::mem::replace(&mut piece, vec![tile]));
        }
    }
    if piece.len() > 1 {
        pieces.push(piece);
    }

    pieces
}

/// Draw the road the player is dragging out, which has no lane to be seen by until it is laid.
fn draw_the_drawn_road(mut gizmos: Gizmos<DebugGizmos>, drawn: Query<&DrawnRoad>) {
    for drawing in &drawn {
        gizmos.linestrip(
            drawing
                .path
                .iter()
                .map(|tile| tile.world_position() + GIZMO_LIFT),
            DRAWING_COLOUR,
        );
    }
}

/// Draw every lane, and the order a rover drives its segments in.
///
/// A chain of segments is otherwise only visible in a test: two lanes lying on the same road look
/// like one road, and the join at a dead end looks like a rover turning round of its own accord.
fn draw_the_lanes(
    mut gizmos: Gizmos<DebugGizmos>,
    segments: Query<(&RoadSegment, Option<&NextSegment>)>,
    onward: Query<&RoadSegment>,
) {
    for (segment, next) in &segments {
        gizmos.linestrip(
            (0..=SEGMENT_SUBDIVISIONS).map(|step| {
                segment.world_position(step as f32 / SEGMENT_SUBDIVISIONS as f32) + GIZMO_LIFT
            }),
            LANE_COLOUR,
        );

        let Some(next) = next.and_then(|next| onward.get(next.0).ok()) else {
            continue;
        };
        gizmos.arrow(
            segment.world_position(1. - HANDOVER_REACH) + GIZMO_LIFT,
            next.world_position(HANDOVER_REACH) + GIZMO_LIFT,
            HANDOVER_COLOUR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::cleanup::CleanupPlugin;
    use crate::common::initialize::InitializationFailed;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::testing::{headless_app, tick};

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// How many straight pieces a segment is measured in.
    const LENGTH_SAMPLES: usize = 128;

    /// A straight run of tiles, in offset-row coordinates.
    const STRAIGHT: [(i32, i32); 4] = [(0, 0), (1, 0), (2, 0), (3, 0)];

    /// A run of tiles that turns a corner, in offset-row coordinates.
    const TURNING: [(i32, i32); 3] = [(0, 0), (1, 0), (1, 1)];

    /// A run of tiles that runs straight and then turns twice, in offset-row coordinates.
    const WINDING: [(i32, i32); 5] = [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)];

    /// A run of tiles crossing `STRAIGHT` at its third tile, in offset-row coordinates.
    const CROSSING: [(i32, i32); 3] = [(2, -1), (2, 0), (2, 1)];

    /// A run of tiles setting off from the last tile of `STRAIGHT`, in offset-row coordinates.
    const ONWARD: [(i32, i32); 2] = [(3, 0), (3, 1)];

    fn app_holding(tool: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(tool)
            .insert_resource(PlayerInput::default())
            .add_plugins((DebugGizmosPlugin, CleanupPlugin, RoadPlugin));
        app
    }

    fn road_app() -> App {
        app_holding(PlayerAction::Select)
    }

    fn tiles(offsets: &[(i32, i32)]) -> Vec<HexCoordinates> {
        offsets
            .iter()
            .map(|&(col, row)| HexCoordinates::from_offset_row(col, row))
            .collect()
    }

    fn spawn_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        app.world_mut()
            .spawn(Road {
                path: tiles(offsets),
            })
            .id()
    }

    fn built_road(offsets: &[(i32, i32)]) -> (App, Entity) {
        let mut app = road_app();
        let road = spawn_road(&mut app, offsets);
        tick(&mut app);
        (app, road)
    }

    /// The `T` on `entity`, or nothing where the entity is gone as well as where the component is.
    fn component_of<T: Component>(app: &App, entity: Entity) -> Option<&T> {
        app.world().get_entity(entity).ok()?.get::<T>()
    }

    fn children_of(app: &App, entity: Entity) -> Vec<Entity> {
        component_of::<Children>(app, entity)
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    }

    fn lanes(app: &App, road: Entity) -> Vec<Entity> {
        children_of(app, road)
            .into_iter()
            .filter(|&lane| component_of::<Lane>(app, lane).is_some())
            .collect()
    }

    /// The lane of `road` that sets off from `tile`, of which there is exactly one.
    fn lane_from(app: &App, road: Entity, tile: HexCoordinates) -> Vec<Entity> {
        lanes(app, road)
            .into_iter()
            .map(|lane| children_of(app, lane))
            .find(|segments| {
                segments.first().is_some_and(|&first| {
                    position(app, first, 0.).distance(tile.world_position()) < TOLERANCE
                })
            })
            .unwrap_or_default()
    }

    fn position(app: &App, segment: Entity, along: f32) -> Vec3 {
        component_of::<RoadSegment>(app, segment)
            .map(|segment| segment.world_position(along))
            .unwrap_or(Vec3::NAN)
    }

    /// How far it is along `segment`, measured by walking the spline in small steps.
    fn length_of(app: &App, segment: Entity) -> f32 {
        (1..=LENGTH_SAMPLES)
            .map(|step| {
                let before = position(app, segment, (step - 1) as f32 / LENGTH_SAMPLES as f32);
                position(app, segment, step as f32 / LENGTH_SAMPLES as f32).distance(before)
            })
            .sum()
    }

    fn next_of(app: &App, segment: Entity) -> Option<Entity> {
        component_of::<NextSegment>(app, segment).map(|next| next.0)
    }

    fn segments_in_the_world(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<RoadSegment>>()
            .iter(app.world())
            .count()
    }

    #[test]
    fn a_road_gets_a_lane_in_each_direction() {
        let (app, road) = built_road(&STRAIGHT);

        assert_eq!(lanes(&app, road).len(), 2);
    }

    #[test]
    fn a_lane_is_split_into_a_segment_for_every_step_of_the_road() {
        let (app, road) = built_road(&STRAIGHT);

        for lane in lanes(&app, road) {
            assert_eq!(children_of(&app, lane).len(), STRAIGHT.len() - 1);
        }
    }

    #[test]
    fn a_lane_runs_from_the_first_tile_of_the_road_to_the_last() {
        let path = tiles(&STRAIGHT);
        let (app, road) = built_road(&STRAIGHT);

        let lane = lane_from(&app, road, path[0]);

        let end = position(&app, *lane.last().expect("the lane has segments"), 1.);
        assert!(end.distance(path[STRAIGHT.len() - 1].world_position()) < TOLERANCE);
    }

    #[test]
    fn the_opposing_lane_runs_the_same_road_the_other_way() {
        let path = tiles(&STRAIGHT);
        let (app, road) = built_road(&STRAIGHT);

        let lane = lane_from(&app, road, path[STRAIGHT.len() - 1]);

        let end = position(&app, *lane.last().expect("the lane has segments"), 1.);
        assert!(end.distance(path[0].world_position()) < TOLERANCE);
    }

    #[test]
    fn a_segment_starts_where_the_one_before_it_ends() {
        let (app, road) = built_road(&TURNING);

        let mut joins = 0;
        for lane in lanes(&app, road) {
            for pair in children_of(&app, lane).windows(2) {
                let ends = position(&app, pair[0], 1.);
                let starts = position(&app, pair[1], 0.);
                assert!(ends.distance(starts) < TOLERANCE, "{ends} then {starts}");
                joins += 1;
            }
        }

        assert_eq!(joins, 2 * (TURNING.len() - 2));
    }

    #[test]
    fn the_segments_of_a_lane_are_roughly_the_same_length() {
        /// How much longer than the shortest segment of a lane the longest may be.
        ///
        /// A road that runs straight and then turns twice measures 3.4% across, the shortest
        /// being a straight run between two tiles and the longest the outside of a bend.
        const SPREAD: f32 = 1.05;

        let (app, road) = built_road(&WINDING);

        let lengths: Vec<f32> = lanes(&app, road)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
            .map(|segment| length_of(&app, segment))
            .collect();
        let longest = lengths.iter().copied().fold(f32::MIN, f32::max);
        let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);

        assert_eq!(lengths.len(), 2 * (WINDING.len() - 1));
        assert!(longest <= shortest * SPREAD, "{lengths:?}");
    }

    #[test]
    fn a_position_along_a_segment_follows_the_spline_rather_than_the_line_between_its_ends() {
        /// How far off the straight line the middle of a curved segment has to be, as a share of
        /// the distance between its ends.
        const BEND: f32 = 0.01;

        let path = tiles(&TURNING);
        let (app, road) = built_road(&TURNING);

        let lane = lane_from(&app, road, path[0]);
        let segment = *lane.first().expect("the lane has segments");

        let (start, end) = (position(&app, segment, 0.), position(&app, segment, 1.));
        let strayed = position(&app, segment, 0.5).distance(start.midpoint(end));
        assert!(strayed > start.distance(end) * BEND, "strayed {strayed}");
    }

    #[test]
    fn following_the_segments_returns_to_where_it_started_having_driven_both_lanes() {
        let (mut app, road) = built_road(&TURNING);
        let start = children_of(&app, lanes(&app, road)[0])[0];

        let mut driven = vec![start];
        while let Some(next) = next_of(&app, *driven.last().expect("the drive has a segment")) {
            if next == start {
                break;
            }
            driven.push(next);
        }

        assert_eq!(driven.len(), segments_in_the_world(&mut app));
        assert_eq!(
            next_of(&app, *driven.last().expect("the drive ends")),
            Some(start)
        );
    }

    #[test]
    fn despawning_a_road_takes_both_lanes_and_every_segment_with_it() {
        let (mut app, road) = built_road(&STRAIGHT);
        assert!(segments_in_the_world(&mut app) > 0);

        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert_eq!(segments_in_the_world(&mut app), 0);
        assert!(lanes(&app, road).is_empty());
    }

    #[test]
    fn a_road_of_a_single_tile_is_marked_as_failed_rather_than_taking_the_game_down() {
        let (mut app, road) = built_road(&[(0, 0)]);

        assert!(app.world().entity(road).contains::<InitializationFailed>());
        assert_eq!(segments_in_the_world(&mut app), 0);
    }

    fn spawn_tiles(app: &mut App, offsets: &[(i32, i32)]) -> Vec<Entity> {
        tiles(offsets)
            .into_iter()
            .map(|coordinates| app.world_mut().spawn(MapTile { coordinates }).id())
            .collect()
    }

    /// Put the cursor over `tile` with the primary button as `dragging` says, and take a frame.
    fn move_cursor(app: &mut App, tile: Option<Entity>, dragging: bool) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.cursor_tile = tile;
            input.dragging = dragging;
        }
        tick(app);
    }

    /// Drag over `path` a tile at a frame, then let the button go over the last of them.
    fn drag_over(app: &mut App, path: &[Entity]) {
        for &tile in path {
            move_cursor(app, Some(tile), true);
        }
        move_cursor(app, path.last().copied(), false);
    }

    fn roads_in_the_world(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .count()
    }

    /// Whether a road runs through exactly `offsets`, drawn from either of its two ends.
    fn a_road_runs_through(app: &mut App, offsets: &[(i32, i32)]) -> bool {
        let wanted = tiles(offsets);
        let backwards: Vec<HexCoordinates> = wanted.iter().copied().rev().collect();
        app.world_mut()
            .query::<&Road>()
            .iter(app.world())
            .any(|road| road.path == wanted || road.path == backwards)
    }

    #[test]
    fn a_drag_across_tiles_lays_a_road_through_them() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &STRAIGHT);

        drag_over(&mut app, &path);

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert_eq!(roads_in_the_world(&mut app), 1);
    }

    #[test]
    fn nothing_is_laid_until_the_drag_ends() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &STRAIGHT);

        for &tile in &path {
            move_cursor(&mut app, Some(tile), true);
        }

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn a_drag_that_never_left_its_tile_lays_no_road() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &[(0, 0)]);

        drag_over(&mut app, &path);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn a_drag_while_selecting_lays_nothing() {
        let mut app = app_holding(PlayerAction::Select);
        let path = spawn_tiles(&mut app, &STRAIGHT);

        drag_over(&mut app, &path);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn a_drag_while_editing_buildings_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditBuildings);
        let path = spawn_tiles(&mut app, &STRAIGHT);

        drag_over(&mut app, &path);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn a_drag_that_skipped_a_tile_still_runs_through_it() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_tiles(&mut app, &STRAIGHT);
        let flicked = spawn_tiles(&mut app, &[STRAIGHT[0], STRAIGHT[STRAIGHT.len() - 1]]);

        drag_over(&mut app, &flicked);

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
    }

    #[test]
    fn resting_on_a_tile_does_not_repeat_it() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &[(0, 0), (1, 0)]);

        move_cursor(&mut app, Some(path[0]), true);
        move_cursor(&mut app, Some(path[0]), true);
        move_cursor(&mut app, Some(path[1]), true);
        move_cursor(&mut app, Some(path[1]), true);
        move_cursor(&mut app, Some(path[1]), false);

        assert!(a_road_runs_through(&mut app, &[(0, 0), (1, 0)]));
    }

    #[test]
    fn a_drag_passing_over_no_tile_carries_on_from_where_it_left_off() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &[(0, 0), (1, 0)]);

        move_cursor(&mut app, Some(path[0]), true);
        move_cursor(&mut app, None, true);
        move_cursor(&mut app, Some(path[1]), true);
        move_cursor(&mut app, Some(path[1]), false);

        assert!(a_road_runs_through(&mut app, &[(0, 0), (1, 0)]));
    }

    #[test]
    fn putting_the_tool_down_mid_drag_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = spawn_tiles(&mut app, &STRAIGHT);
        for &tile in &path {
            move_cursor(&mut app, Some(tile), true);
        }

        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::Select);
        move_cursor(&mut app, path.last().copied(), true);
        move_cursor(&mut app, path.last().copied(), false);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    /// Lay `STRAIGHT`, then drag `CROSSING` over the middle of it.
    fn a_road_drawn_across_another() -> App {
        let mut app = app_holding(PlayerAction::EditRoads);
        let along = spawn_tiles(&mut app, &STRAIGHT);
        drag_over(&mut app, &along);
        let across = spawn_tiles(&mut app, &CROSSING);
        drag_over(&mut app, &across);
        tick(&mut app);
        app
    }

    #[test]
    fn a_road_drawn_across_another_ends_where_they_meet() {
        let mut app = a_road_drawn_across_another();

        assert!(a_road_runs_through(&mut app, &CROSSING[..2]));
        assert!(a_road_runs_through(&mut app, &CROSSING[1..]));
    }

    #[test]
    fn the_road_it_crossed_is_split_at_the_tile_they_share() {
        let mut app = a_road_drawn_across_another();

        assert!(a_road_runs_through(&mut app, &STRAIGHT[..3]));
        assert!(a_road_runs_through(&mut app, &STRAIGHT[2..]));
        assert_eq!(roads_in_the_world(&mut app), 4);
    }

    #[test]
    fn every_road_a_crossing_leaves_behind_gets_its_lanes() {
        let mut app = a_road_drawn_across_another();

        let laid: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .collect();

        assert_eq!(laid.len(), 4);
        for road in laid {
            assert!(!app.world().entity(road).contains::<InitializationFailed>());
            assert_eq!(lanes(&app, road).len(), 2);
        }
    }

    #[test]
    fn a_road_drawn_onto_the_end_of_another_leaves_it_whole() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let along = spawn_tiles(&mut app, &STRAIGHT);
        drag_over(&mut app, &along);

        let onward = spawn_tiles(&mut app, &ONWARD);
        drag_over(&mut app, &onward);

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert!(a_road_runs_through(&mut app, &ONWARD));
        assert_eq!(roads_in_the_world(&mut app), 2);
    }
}
