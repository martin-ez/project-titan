use crate::common::cleanup::DestroyOnStateChange;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{HexCoordinates, LatticeNode, MapTile};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashSet;

/// How many straight pieces a segment's arc is drawn as.
const SEGMENT_SUBDIVISIONS: u32 = 8;

/// How far into a segment the arrow onto the next one reaches, at either end of the handover.
const HANDOVER_REACH: f32 = 0.1;

/// How long a stretch of an arc a rover should drive in one go.
///
/// Arcs come out of a fit at lengths of their own, so each is cut into whichever number of equal
/// stretches lands nearest this. Keeping segments close to one length is what lets #8 read a
/// segment's capacity off its geometry rather than store one beside it.
const SEGMENT_LENGTH: f32 = 5.;

/// How near two tangents have to be to count as the same direction when a biarc is fitted.
const JOIN_TOLERANCE: f32 = 1e-4;

/// How far the debug view lifts a lane off the ground, so it does not fight the tiles it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a lane's arcs are drawn in
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

/// A road the player drew: the nodes it runs through, in the order it crossed them.
///
/// The nodes are the road. Its arcs, the lanes over them and every world position a rover ever
/// stands at are derived from them when it is laid, so two roads drawn through the same nodes are
/// the same shape. Invariant 3: the integers are the truth and the curve comes out of them.
#[derive(Component)]
#[require(NeedsInitialization)]
pub struct Road {
    /// The nodes the road was drawn through, from one end to the other.
    pub nodes: Vec<LatticeNode>,
}

/// The road the player is part way through dragging out, as far as the cursor has taken it.
///
/// It is a record of tiles and nothing else until the button comes up: no arc, no lane and
/// nothing a rover could drive. Putting the tool down destroys it with the rest of the tool's
/// state, so a drag abandoned half way leaves the network as it was.
#[derive(Component)]
#[require(DestroyOnStateChange)]
struct DrawnRoad {
    path: Vec<HexCoordinates>,
}

/// A circular arc on the ground, and the whole of a road's geometry.
///
/// A straight is an arc of zero curvature rather than a second case, so nothing reading one has to
/// ask which it holds. It is built when the road is laid and never rewritten: cutting a road moves
/// which stretch of an arc a segment covers and never the arc itself, which is what makes a
/// junction cut into a road move none of it, however many times it is cut (invariant 6).
#[derive(Clone, Copy, Debug)]
struct Arc {
    start: Vec3,
    tangent: Vec3,
    curvature: f32,
    length: f32,
}

/// One direction of travel along a road, owning the segments that make it up.
#[derive(Component)]
struct Lane;

/// A stretch of one lane: the piece of an arc a rover drives in one go.
///
/// It holds the arc rather than a curve of its own, and says where along it the stretch begins and
/// ends. Two segments cut from one arc therefore share it exactly, and neither has moved.
#[derive(Component)]
pub struct RoadSegment {
    arc: Arc,
    from: f32,
    to: f32,
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

impl Arc {
    /// The one arc that leaves `start` along `tangent` and passes through `target`.
    ///
    /// There is exactly one, so nothing about the curve is chosen: aiming along the tangent gives
    /// a straight and aiming off it gives the arc that reaches the target.
    fn through(start: Vec3, tangent: Vec3, target: Vec3) -> Self {
        let reach = target - start;
        let span = reach.length_squared();
        if span == 0. {
            return Self {
                start,
                tangent,
                curvature: 0.,
                length: 0.,
            };
        }

        let sideways = turn_of(tangent, reach);
        let curvature = 2. * sideways / span;
        let turn = 2. * sideways.atan2(tangent.dot(reach));

        Self {
            start,
            tangent,
            curvature,
            length: if curvature == 0. {
                reach.length()
            } else {
                turn / curvature
            },
        }
    }

    /// Where on the ground a point `at` along this arc stands.
    fn position(&self, at: f32) -> Vec3 {
        let at = at.clamp(0., self.length);
        if self.curvature == 0. {
            return self.start + self.tangent * at;
        }

        let centre = self.start + left_of(self.tangent) / self.curvature;
        centre + turned(self.start - centre, self.curvature * at)
    }

    /// Which way a rover `at` along this arc is pointing.
    fn tangent_at(&self, at: f32) -> Vec3 {
        turned(self.tangent, self.curvature * at.clamp(0., self.length))
    }

    /// The same arc driven the other way, for the lane that runs back down the road.
    fn reversed(&self) -> Self {
        Self {
            start: self.position(self.length),
            tangent: -self.tangent_at(self.length),
            curvature: -self.curvature,
            length: self.length,
        }
    }
}

fn turn_of(tangent: Vec3, reach: Vec3) -> f32 {
    tangent.x * reach.z - tangent.z * reach.x
}

fn left_of(tangent: Vec3) -> Vec3 {
    Vec3::new(-tangent.z, 0., tangent.x)
}

fn turned(vector: Vec3, by: f32) -> Vec3 {
    let (sin, cos) = by.sin_cos();
    Vec3::new(
        vector.x * cos - vector.z * sin,
        vector.y,
        vector.x * sin + vector.z * cos,
    )
}

impl RoadSegment {
    /// Where on the ground a rover `along` of the way down this segment stands.
    ///
    /// Either end of the stretch is read off the arc exactly rather than interpolated to, so a
    /// rover leaving a segment stands where the next one starts rather than a rounding away.
    pub fn world_position(&self, along: f32) -> Vec3 {
        self.arc.position(match along {
            along if along <= 0. => self.from,
            along if along >= 1. => self.to,
            along => self.from + (self.to - self.from) * along,
        })
    }
}

impl Initialize<RoadInitializeParams<'_, '_>> for Road {
    fn initialize(&mut self, entity: &Entity, params: &mut RoadInitializeParams) -> Result {
        let along = arcs_through(&self.nodes);
        if along.is_empty() {
            return Err("a road of no arcs".into());
        }
        let back: Vec<Arc> = along.iter().rev().map(Arc::reversed).collect();

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

/// The arcs running through `nodes`, each leaving the one before it at the same tangent.
///
/// A node's direction is the bisector of the two runs meeting there, and a pair of nodes is joined
/// by the one arc that honours both directions where it can and by two where it cannot. Fitting a
/// single arc per pair from the previous tangent alone would not do: aiming at a node sixty
/// degrees off the current heading turns the road a hundred and twenty, and a chain of those winds
/// further off course at every step rather than following the nodes it was drawn through.
fn arcs_through(nodes: &[LatticeNode]) -> Vec<Arc> {
    let points: Vec<Vec3> = nodes.iter().map(LatticeNode::world_position).collect();
    let tangents = tangents_along(&points);

    (0..points.len().saturating_sub(1))
        .flat_map(|step| {
            biarc(
                points[step],
                tangents[step],
                points[step + 1],
                tangents[step + 1],
            )
        })
        .collect()
}

fn tangents_along(points: &[Vec3]) -> Vec<Vec3> {
    (0..points.len())
        .map(|at| {
            let before = (at > 0).then(|| (points[at] - points[at - 1]).normalize_or_zero());
            let after =
                (at + 1 < points.len()).then(|| (points[at + 1] - points[at]).normalize_or_zero());

            match (before, after) {
                (Some(before), Some(after)) => (before + after).normalize_or(after),
                (Some(before), None) => before,
                (None, Some(after)) => after,
                (None, None) => Vec3::X,
            }
        })
        .collect()
}

fn biarc(from: Vec3, leaving: Vec3, to: Vec3, arriving: Vec3) -> Vec<Arc> {
    let whole = Arc::through(from, leaving, to);
    if whole
        .tangent_at(whole.length)
        .abs_diff_eq(arriving, JOIN_TOLERANCE)
    {
        return vec![whole];
    }

    let joint = joint_between(from, leaving, to, arriving);
    let first = Arc::through(from, leaving, joint);
    let second = Arc::through(joint, first.tangent_at(first.length), to);
    vec![first, second]
}

fn joint_between(from: Vec3, leaving: Vec3, to: Vec3, arriving: Vec3) -> Vec3 {
    let reach = to - from;
    let closing = 2. * (1. - leaving.dot(arriving));
    let along = reach.dot(leaving + arriving);
    let reached = if closing.abs() < JOIN_TOLERANCE {
        reach.length() / 2.
    } else {
        (-along + (along * along + closing * reach.length_squared()).sqrt()) / closing
    };

    (from + leaving * reached).midpoint(to - arriving * reached)
}

/// The two segments of a lane a road's other lane joins onto.
struct LaneEnds {
    first: Entity,
    last: Entity,
}

/// Put one direction of travel on `road`: a lane, and a segment of it per stretch of every arc.
fn spawn_lane(commands: &mut Commands, road: Entity, arcs: &[Arc]) -> Result<LaneEnds> {
    let lane = commands.spawn((Lane, ChildOf(road))).id();
    let mut ends: Option<LaneEnds> = None;

    for &arc in arcs {
        for (from, to) in stretches_of(&arc) {
            let segment = commands
                .spawn((RoadSegment { arc, from, to }, ChildOf(lane)))
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
    }

    ends.ok_or_else(|| "a lane of no segments".into())
}

/// Where along `arc` each of its segments begins and ends, all of them the same length.
///
/// The count is whichever leaves them nearest `SEGMENT_LENGTH`, so an arc half again as long as
/// one segment is one segment rather than two short ones. The last ends on the arc's own length
/// rather than on a multiple of the step, so a rover leaves the arc exactly where it ends.
fn stretches_of(arc: &Arc) -> Vec<(f32, f32)> {
    let pieces = (arc.length / SEGMENT_LENGTH).round().max(1.) as usize;
    let step = arc.length / pieces as f32;

    (0..pieces)
        .map(|piece| {
            let ends = if piece + 1 == pieces {
                arc.length
            } else {
                (piece + 1) as f32 * step
            };
            (piece as f32 * step, ends)
        })
        .collect()
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

        let drawn: Vec<LatticeNode> = drawing
            .path
            .iter()
            .copied()
            .map(LatticeNode::from_tile)
            .collect();
        let meetings = nodes_shared_with(&drawn, &roads);
        for nodes in split_at(&drawn, &meetings) {
            commands.spawn(Road { nodes });
        }
        for (crossed, road) in &roads {
            let pieces = split_at(&road.nodes, &meetings);
            if pieces.len() < 2 {
                continue;
            }
            commands.entity(crossed).despawn();
            for nodes in pieces {
                commands.spawn(Road { nodes });
            }
        }
    }
}

/// The nodes of `drawn` that a road already runs through.
fn nodes_shared_with(
    drawn: &[LatticeNode],
    roads: &Query<(Entity, &Road)>,
) -> HashSet<LatticeNode> {
    let drawn: HashSet<LatticeNode> = drawn.iter().copied().collect();
    roads
        .iter()
        .flat_map(|(_, road)| road.nodes.iter().copied())
        .filter(|node| drawn.contains(node))
        .collect()
}

/// Break `nodes` into the roads they become once cut at every node in `at`.
///
/// A cut node ends the piece before it and starts the piece after, so the roads either side meet
/// there rather than running through: that shared end is what makes the node a place a rover has
/// to be handed over at. A cut at one of `nodes`' own ends leaves it whole, being where it already
/// ended, and a piece of a single node is no road at all and is dropped.
fn split_at(nodes: &[LatticeNode], at: &HashSet<LatticeNode>) -> Vec<Vec<LatticeNode>> {
    let mut pieces = Vec::new();
    let mut piece: Vec<LatticeNode> = Vec::new();

    for &node in nodes {
        piece.push(node);
        if at.contains(&node) && piece.len() > 1 {
            pieces.push(std::mem::replace(&mut piece, vec![node]));
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

    fn nodes(offsets: &[(i32, i32)]) -> Vec<LatticeNode> {
        tiles(offsets)
            .into_iter()
            .map(LatticeNode::from_tile)
            .collect()
    }

    /// How far it is along the straight runs between the nodes of `offsets`.
    fn run_through(offsets: &[(i32, i32)]) -> f32 {
        nodes(offsets)
            .windows(2)
            .map(|pair| pair[0].world_position().distance(pair[1].world_position()))
            .sum()
    }

    fn spawn_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        app.world_mut()
            .spawn(Road {
                nodes: nodes(offsets),
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

    /// How far it is along `segment`, measured by walking it in small steps.
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
    fn the_segments_of_a_lane_cover_the_whole_road() {
        let (app, road) = built_road(&STRAIGHT);
        let drawn = run_through(&STRAIGHT);

        for lane in lanes(&app, road) {
            let segments = children_of(&app, lane);
            let driven: f32 = segments
                .iter()
                .map(|&segment| length_of(&app, segment))
                .sum();

            assert!(segments.len() > 1, "one segment for the whole road");
            assert!(
                (driven - drawn).abs() < drawn * TOLERANCE,
                "{driven} driven against {drawn} drawn"
            );
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
        let (mut app, road) = built_road(&TURNING);

        let mut joins = 0;
        for lane in lanes(&app, road) {
            for pair in children_of(&app, lane).windows(2) {
                let ends = position(&app, pair[0], 1.);
                let starts = position(&app, pair[1], 0.);
                assert!(ends.distance(starts) < TOLERANCE, "{ends} then {starts}");
                joins += 1;
            }
        }

        assert_eq!(joins, segments_in_the_world(&mut app) - 2);
    }

    #[test]
    fn the_segments_of_a_lane_are_roughly_the_same_length() {
        /// How much longer than the shortest segment of a lane the longest may be.
        ///
        /// A road that runs straight and then turns twice measures 4.8% across: the arcs a fit
        /// leaves are of lengths of their own, and cutting each to the nearest whole number of
        /// target lengths is what brings them back together.
        const SPREAD: f32 = 1.06;

        let (mut app, road) = built_road(&WINDING);

        let lengths: Vec<f32> = lanes(&app, road)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
            .map(|segment| length_of(&app, segment))
            .collect();
        let longest = lengths.iter().copied().fold(f32::MIN, f32::max);
        let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);

        assert_eq!(lengths.len(), segments_in_the_world(&mut app));

        assert!(longest <= shortest * SPREAD, "{lengths:?}");
    }

    #[test]
    fn a_position_along_a_segment_follows_its_arc_rather_than_the_line_between_its_ends() {
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
        let wanted = nodes(offsets);
        let backwards: Vec<LatticeNode> = wanted.iter().copied().rev().collect();
        app.world_mut()
            .query::<&Road>()
            .iter(app.world())
            .any(|road| road.nodes == wanted || road.nodes == backwards)
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
    fn an_arc_passes_through_the_target_it_was_aimed_at() {
        for target in [
            Vec3::new(10., 0., 10.),
            Vec3::new(10., 0., -10.),
            Vec3::new(3., 0., 12.),
            Vec3::new(-4., 0., 6.),
        ] {
            let arc = Arc::through(Vec3::ZERO, Vec3::X, target);

            let start = arc.position(0.);
            let end = arc.position(arc.length);
            assert!(start.distance(Vec3::ZERO) < TOLERANCE, "starts at {start}");
            assert!(
                end.distance(target) < TOLERANCE,
                "{end} rather than {target}"
            );
        }
    }

    #[test]
    fn an_arc_aimed_along_its_own_tangent_is_straight() {
        let arc = Arc::through(Vec3::ZERO, Vec3::X, Vec3::new(7., 0., 0.));

        assert_eq!(arc.curvature, 0.);
        assert_eq!(arc.length, 7.);
    }

    #[test]
    fn cutting_a_segment_leaves_both_halves_on_the_same_arc() {
        let arc = Arc::through(Vec3::ZERO, Vec3::X, Vec3::new(10., 0., 10.));
        let whole = RoadSegment {
            arc,
            from: 0.,
            to: arc.length,
        };
        let cut = arc.length / 3.;

        let first = RoadSegment {
            arc,
            from: 0.,
            to: cut,
        };
        let second = RoadSegment {
            arc,
            from: cut,
            to: arc.length,
        };

        assert_eq!(first.arc.curvature, arc.curvature);
        assert_eq!(second.arc.curvature, arc.curvature);
        assert_eq!(first.world_position(0.), whole.world_position(0.));
        assert_eq!(second.world_position(1.), whole.world_position(1.));
        assert_eq!(first.world_position(1.), second.world_position(0.));
    }

    #[test]
    fn cutting_an_arc_a_hundred_times_moves_none_of_it() {
        /// How many pieces the arc is cut into, one after another.
        const CUTS: usize = 100;

        let arc = Arc::through(Vec3::ZERO, Vec3::X, Vec3::new(10., 0., 10.));
        let whole = RoadSegment {
            arc,
            from: 0.,
            to: arc.length,
        };

        let mut opened = 0.;
        for cut in 1..=CUTS {
            let closed = arc.length * cut as f32 / CUTS as f32;
            let piece = RoadSegment {
                arc,
                from: opened,
                to: closed,
            };

            assert_eq!(piece.arc.curvature, arc.curvature);
            assert_eq!(piece.world_position(0.), arc.position(opened));
            assert_eq!(piece.world_position(1.), arc.position(closed));
            opened = closed;
        }

        assert_eq!(arc.position(opened), whole.world_position(1.));
    }

    #[test]
    fn every_segment_of_a_road_is_about_the_target_length() {
        let (app, road) = built_road(&WINDING);

        for lane in lanes(&app, road) {
            for segment in children_of(&app, lane) {
                let length = length_of(&app, segment);
                let strayed = (length - SEGMENT_LENGTH).abs();
                assert!(strayed <= SEGMENT_LENGTH / 2., "a segment of {length}");
            }
        }
    }

    #[test]
    fn a_winding_road_is_no_longer_than_the_run_it_was_drawn_through() {
        /// How much longer than the straight runs between its nodes a road may measure.
        ///
        /// A chain of single arcs fitted from the previous tangent alone fails this by turning
        /// twice as far as it is aimed at every node, which sends the road back the way it came.
        const WANDER: f32 = 1.15;

        let (app, road) = built_road(&WINDING);
        let drawn = run_through(&WINDING);

        let driven: f32 = children_of(&app, lanes(&app, road)[0])
            .into_iter()
            .map(|segment| length_of(&app, segment))
            .sum();

        assert!(driven <= drawn * WANDER, "{driven} against {drawn} drawn");
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
