use crate::common::cleanup::DestroyOnStateChange;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{LatticeNode, MAP_TILE_WIDTH};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashSet;

/// How many straight pieces a segment's arc is drawn as.
const SEGMENT_SUBDIVISIONS: u32 = 8;

/// How many straight pieces a disc the road cannot reach into is drawn as.
const RING_SUBDIVISIONS: u32 = 24;

/// How far into a segment the arrow onto the next one reaches, at either end of the handover.
const HANDOVER_REACH: f32 = 0.1;

/// How long a stretch of an arc a rover should drive in one go.
///
/// Arcs come out of a fit at lengths of their own, so each is cut into whichever number of equal
/// stretches lands nearest this. Keeping segments close to one length is what lets #8 read a
/// segment's capacity off its geometry rather than store one beside it.
const SEGMENT_LENGTH: f32 = 5.;

/// The tightest turn a road may be built to make, as the radius of the arc a rover drives.
///
/// Half a tile across the flats, so the bound comes off the grid rather than out of the air. It
/// leaves the sixty degree turn onto a neighbouring tile buildable, whose arc has a radius of one
/// lattice step, and refuses the same turn onto a neighbouring node, which would need two thirds
/// of that. Under it the nodes a road cannot reach from where it stands are the two discs of this
/// radius that touch its heading, one either side.
const MIN_TURN_RADIUS: f32 = MAP_TILE_WIDTH / 2.;

/// How far off the heading a target may sit and still be aimed at straight.
///
/// A chain of arcs carries its tangent from one arc to the next, so a target that is dead ahead
/// arrives a rounding off the heading rather than exactly on it. Curving to meet that rounding
/// gives an arc of radius in the millions, whose centre is too far from the road for a position
/// on it to survive being computed in single precision.
const STRAIGHT_REACH: f32 = 1e-3;

/// How far the debug view lifts a lane off the ground, so it does not fight the tiles it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a lane's arcs are drawn in
const LANE_COLOUR: Color = Color::srgb(0.35, 0.75, 0.95);

/// The colour the step from one segment onto the next is drawn in
const HANDOVER_COLOUR: Color = Color::srgb(0.95, 0.8, 0.3);

/// The colour the road the player is still placing is drawn in
const DRAWING_COLOUR: Color = Color::srgb(0.6, 0.95, 0.6);

/// The colour the arc a click would lay is drawn in
const PROPOSAL_COLOUR: Color = Color::srgb(0.95, 0.95, 0.4);

/// The colour the ground a road cannot turn tightly enough to reach is drawn in
const UNREACHABLE_COLOUR: Color = Color::srgb(0.95, 0.35, 0.35);

/// The roads on the map, and the lanes a rover drives on them.
///
/// A road carries one lane in each direction, built together and removed together, and the two
/// join at each end so a dead-end spur is drivable. Nothing overtakes anywhere in the network:
/// there is no lane to move into, so a slow rover is everyone's problem and one badly placed
/// building is a queue you can watch form. One lane shared both ways was cheaper and made traffic
/// a decoration; several each way bought overtaking and spent it softening the jams the game is
/// for; making the player draw the return leg charged the saving to the first thing they build.
pub struct RoadPlugin;

/// A road the player placed: the nodes it runs through, in the order they were clicked.
///
/// The nodes are the road. Its arcs, the lanes over them and every world position a rover ever
/// stands at are derived from them when it is laid, so two roads placed through the same nodes
/// are the same shape. Invariant 3: the integers are the truth and the curve comes out of them.
#[derive(Component)]
#[require(NeedsInitialization)]
pub struct Road {
    /// The nodes the road was clicked through, from one end to the other.
    pub nodes: Vec<LatticeNode>,
    /// The direction the road sets off in, which it has where it was begun on another road's end.
    pub leaving: Option<Vec3>,
}

/// The road the player is part way through clicking out, as far as they have taken it.
///
/// It is a record of nodes and nothing else until the road is finished: no arc, no lane and
/// nothing a rover could drive. Putting the tool down destroys it with the rest of the tool's
/// state, so a road abandoned half way leaves the network as it was.
#[derive(Component)]
#[require(DestroyOnStateChange)]
struct DrawnRoad {
    nodes: Vec<LatticeNode>,
    leaving: Option<Vec3>,
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
                    (place_a_node, lay_the_road).chain(),
                    draw_the_lanes,
                    draw_the_road_being_placed,
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
        if sideways.abs() < STRAIGHT_REACH {
            return Self {
                start,
                tangent,
                curvature: 0.,
                length: reach.length(),
            };
        }

        let curvature = 2. * sideways / span;
        let turn = 2. * sideways.atan2(tangent.dot(reach));

        Self {
            start,
            tangent,
            curvature,
            length: turn / curvature,
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
        let along = arcs_through(&self.nodes, self.leaving);
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
/// One arc joins each pair, and it is the only one that leaves the node before along the direction
/// the road arrived on and passes through the node after: `leaving` gives the first that direction,
/// and a road begun on open ground has none, so it sets off straight. Aiming at a node sixty
/// degrees off the heading turns the road a hundred and twenty, which is the player steering
/// rather than the road drifting, because they picked the node and saw the arc before clicking it.
fn arcs_through(nodes: &[LatticeNode], leaving: Option<Vec3>) -> Vec<Arc> {
    let points: Vec<Vec3> = nodes.iter().map(LatticeNode::world_position).collect();
    let Some(setting_off) = leaving.or_else(|| {
        points
            .first()
            .zip(points.get(1))
            .map(|(from, to)| (*to - *from).normalize_or_zero())
    }) else {
        return Vec::new();
    };

    let mut tangent = setting_off;
    points
        .windows(2)
        .map(|pair| {
            let arc = Arc::through(pair[0], tangent, pair[1]);
            tangent = arc.tangent_at(arc.length);
            arc
        })
        .collect()
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

/// Start the road the player is placing, or take it on to the node they clicked.
///
/// The first click lays nothing: it says where the road begins, and it inherits the direction of
/// the road it began on where there is one, so a road joined onto another leaves it without a
/// kink. Every click after it aims the one arc that leaves the node before along the direction the
/// road arrived on, and a target that arc cannot turn tightly enough to reach is refused.
fn place_a_node(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Query<&Road>,
    mut placing: Query<&mut DrawnRoad>,
) {
    if !player_input.tap || *action.get() != PlayerAction::EditRoads {
        return;
    }
    let Some(target) = player_input.cursor_node else {
        return;
    };

    let Some(mut placing) = placing.iter_mut().next() else {
        commands.spawn(DrawnRoad {
            nodes: vec![target],
            leaving: direction_leaving(target, &roads),
        });
        return;
    };

    if placing.nodes.last() == Some(&target) || proposed_arc(&placing, target).is_none() {
        return;
    }
    placing.nodes.push(target);
}

/// The arc a click on `target` would lay, or nothing where no arc can turn tightly enough.
fn proposed_arc(placing: &DrawnRoad, target: LatticeNode) -> Option<Arc> {
    let standing = placing.nodes.last()?.world_position();
    let target = target.world_position();
    let arcs = arcs_through(&placing.nodes, placing.leaving);
    let tangent = match arcs.last() {
        Some(arc) => arc.tangent_at(arc.length),
        None => placing
            .leaving
            .unwrap_or_else(|| (target - standing).normalize_or_zero()),
    };

    let arc = Arc::through(standing, tangent, target);
    (arc.curvature.abs() * MIN_TURN_RADIUS <= 1.).then_some(arc)
}

/// The direction a road already at `node` sets off from it, where `node` is an end of one.
///
/// A road met at its middle has two directions rather than one, so only its ends answer: anywhere
/// else the road being placed begins on open ground and sets off straight.
fn direction_leaving(node: LatticeNode, roads: &Query<&Road>) -> Option<Vec3> {
    roads.iter().find_map(|road| {
        let arcs = arcs_through(&road.nodes, road.leaving);
        match (road.nodes.first(), road.nodes.last()) {
            (_, Some(&last)) if last == node => arcs.last().map(|arc| arc.tangent_at(arc.length)),
            (Some(&first), _) if first == node => arcs.first().map(|arc| -arc.tangent),
            _ => None,
        }
    })
}

/// Put the road the player placed into the world, once they say it is finished.
///
/// Clicking onto a road that is already there finishes it too: reaching one is how a road is
/// joined to the network, and a road that ends on another's node meets it there rather than
/// running through it. A road of a single node is no road and lays nothing.
fn lay_the_road(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    placing: Query<(Entity, &DrawnRoad)>,
    roads: Query<(Entity, &Road)>,
) {
    for (entity, placed) in &placing {
        if !player_input.finish && !reaches_a_road(placed, &roads) {
            continue;
        }
        commands.entity(entity).despawn();

        let meetings = nodes_shared_with(&placed.nodes, &roads);
        for (nodes, leaving) in split_at(&placed.nodes, placed.leaving, &meetings) {
            commands.spawn(Road { nodes, leaving });
        }
        for (crossed, road) in &roads {
            let pieces = split_at(&road.nodes, road.leaving, &meetings);
            if pieces.len() < 2 {
                continue;
            }
            commands.entity(crossed).despawn();
            for (nodes, leaving) in pieces {
                commands.spawn(Road { nodes, leaving });
            }
        }
    }
}

/// Whether the road being placed has arrived on a road that is already there.
fn reaches_a_road(placed: &DrawnRoad, roads: &Query<(Entity, &Road)>) -> bool {
    let Some(reached) = placed.nodes.last().filter(|_| placed.nodes.len() > 1) else {
        return false;
    };
    roads.iter().any(|(_, road)| road.nodes.contains(reached))
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

/// Break a road into the roads it becomes once cut at every node in `at`.
///
/// A cut node ends the piece before it and starts the piece after, so the roads either side meet
/// there rather than running through: that shared end is what makes the node a place a rover has
/// to be handed over at. A cut at one of its own ends leaves it whole, being where it already
/// ended, and a piece of a single node is no road at all and is dropped.
///
/// Each piece keeps the direction the whole road had where that piece begins, so the arcs it is
/// rebuilt from are the arcs it already had: cutting a road moves none of it (invariant 6).
fn split_at(
    nodes: &[LatticeNode],
    leaving: Option<Vec3>,
    at: &HashSet<LatticeNode>,
) -> Vec<(Vec<LatticeNode>, Option<Vec3>)> {
    let arcs = arcs_through(nodes, leaving);
    let directions: Vec<Option<Vec3>> = (0..nodes.len())
        .map(
            |node| match node.checked_sub(1).and_then(|before| arcs.get(before)) {
                Some(arc) => Some(arc.tangent_at(arc.length)),
                None => leaving,
            },
        )
        .collect();

    let mut pieces = Vec::new();
    let mut opened = 0;

    for (node, &standing) in nodes.iter().enumerate() {
        let ends = at.contains(&standing) && node > opened;
        if !ends && node + 1 < nodes.len() {
            continue;
        }
        if node > opened {
            pieces.push((nodes[opened..=node].to_vec(), directions[opened]));
        }
        opened = node;
    }

    pieces
}

/// Draw the road being placed, the arc the next click would lay, and the ground it cannot reach.
///
/// A road being placed has no lane to be seen by until it is laid, and the arc a click lays turns
/// twice as far as it is aimed, so the player has to see it before committing to it rather than
/// after (invariant 5). The two discs are the ground no arc from here can turn tightly enough to
/// reach, and a target inside one is drawn as the refusal it is.
fn draw_the_road_being_placed(
    mut gizmos: Gizmos<DebugGizmos>,
    player_input: Res<PlayerInput>,
    placing: Query<&DrawnRoad>,
) {
    for placed in &placing {
        gizmos.linestrip(
            placed
                .nodes
                .iter()
                .map(|node| node.world_position() + GIZMO_LIFT),
            DRAWING_COLOUR,
        );

        let Some(standing) = placed.nodes.last().map(LatticeNode::world_position) else {
            continue;
        };
        if let Some(heading) = heading_of(placed) {
            for side in [1., -1.] {
                let centre = standing + left_of(heading) * side * MIN_TURN_RADIUS;
                gizmos.linestrip(ring_around(centre, MIN_TURN_RADIUS), UNREACHABLE_COLOUR);
            }
        }

        let Some(target) = player_input.cursor_node else {
            continue;
        };
        match proposed_arc(placed, target) {
            Some(arc) => gizmos.linestrip(sampled(&arc), PROPOSAL_COLOUR),
            None => gizmos.line(
                standing + GIZMO_LIFT,
                target.world_position() + GIZMO_LIFT,
                UNREACHABLE_COLOUR,
            ),
        }
    }
}

/// Which way the road being placed is pointing, which is nothing before it has been aimed at all.
fn heading_of(placed: &DrawnRoad) -> Option<Vec3> {
    let arcs = arcs_through(&placed.nodes, placed.leaving);
    match arcs.last() {
        Some(arc) => Some(arc.tangent_at(arc.length)),
        None => placed.leaving,
    }
}

fn sampled(arc: &Arc) -> impl Iterator<Item = Vec3> {
    let arc = *arc;
    (0..=SEGMENT_SUBDIVISIONS).map(move |step| {
        arc.position(arc.length * step as f32 / SEGMENT_SUBDIVISIONS as f32) + GIZMO_LIFT
    })
}

fn ring_around(centre: Vec3, radius: f32) -> impl Iterator<Item = Vec3> {
    (0..=RING_SUBDIVISIONS).map(move |step| {
        let turn = std::f32::consts::TAU * step as f32 / RING_SUBDIVISIONS as f32;
        centre + turned(Vec3::X * radius, turn) + GIZMO_LIFT
    })
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
    use crate::map::HexCoordinates;
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
                leaving: None,
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
        /// A road that runs straight and then turns twice measures 21% across: an arc is cut into
        /// whichever whole number of pieces lands nearest the target length, so a straight run
        /// between two nodes gives pieces a little under it and the arc that turns a hundred and
        /// twenty degrees gives pieces a little over.
        const SPREAD: f32 = 1.25;

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
        let turning = *lane
            .last()
            .expect("the road sets off straight and turns at its far end");

        let (start, end) = (position(&app, turning, 0.), position(&app, turning, 1.));
        let strayed = position(&app, turning, 0.5).distance(start.midpoint(end));
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

    /// Ask for the road being placed to be laid, and take the frame that reads it.
    fn finish_the_road(app: &mut App) {
        app.world_mut().resource_mut::<PlayerInput>().finish = true;
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().finish = false;
    }

    /// Click through `path` and finish, which is how a whole road is placed.
    ///
    /// The frame after the road is laid is what builds its lanes, so it takes one more.
    fn place_road(app: &mut App, path: &[LatticeNode]) {
        for &node in path {
            click_at(app, node);
        }
        finish_the_road(app);
        tick(app);
    }

    fn placing(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<DrawnRoad>>()
            .iter(app.world())
            .count()
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
    fn a_road_clicked_through_nodes_runs_through_them() {
        let mut app = app_holding(PlayerAction::EditRoads);

        place_road(&mut app, &nodes(&STRAIGHT));

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert_eq!(roads_in_the_world(&mut app), 1);
    }

    #[test]
    fn the_first_click_starts_a_road_and_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);

        click_at(&mut app, nodes(&STRAIGHT)[0]);

        assert_eq!(roads_in_the_world(&mut app), 0);
        assert_eq!(placing(&mut app), 1);
    }

    #[test]
    fn nothing_is_laid_until_the_road_is_finished() {
        let mut app = app_holding(PlayerAction::EditRoads);

        for &node in &nodes(&STRAIGHT) {
            click_at(&mut app, node);
        }

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn finishing_a_road_of_one_node_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);

        place_road(&mut app, &nodes(&[(0, 0)]));

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn clicking_the_node_the_road_already_stands_on_adds_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = nodes(&[(0, 0), (1, 0)]);

        click_at(&mut app, path[0]);
        click_at(&mut app, path[0]);
        click_at(&mut app, path[1]);
        click_at(&mut app, path[1]);
        finish_the_road(&mut app);

        assert!(a_road_runs_through(&mut app, &[(0, 0), (1, 0)]));
    }

    #[test]
    fn a_click_on_no_node_at_all_adds_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = nodes(&[(0, 0), (1, 0)]);

        click_at(&mut app, path[0]);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.cursor_node = None;
            input.tap = true;
        }
        tick(&mut app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
        click_at(&mut app, path[1]);
        finish_the_road(&mut app);

        assert!(a_road_runs_through(&mut app, &[(0, 0), (1, 0)]));
    }

    #[test]
    fn clicking_while_selecting_lays_nothing() {
        let mut app = app_holding(PlayerAction::Select);

        place_road(&mut app, &nodes(&STRAIGHT));

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn clicking_while_editing_buildings_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditBuildings);

        place_road(&mut app, &nodes(&STRAIGHT));

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    /// The node of the tile at `offset` that lies `towards` from the middle of it.
    fn corner_of(offset: (i32, i32), towards: Vec3) -> LatticeNode {
        let tile = HexCoordinates::from_offset_row(offset.0, offset.1);
        LatticeNode::nearest_on(tile, tile.world_position() + towards)
    }

    /// The road running through `offsets`, of which there is one once it has been laid.
    fn road_through(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        let wanted = nodes(offsets);
        let backwards: Vec<LatticeNode> = wanted.iter().copied().rev().collect();
        app.world_mut()
            .query::<(Entity, &Road)>()
            .iter(app.world())
            .find(|(_, road)| road.nodes == wanted || road.nodes == backwards)
            .map(|(entity, _)| entity)
            .expect("the road was laid")
    }

    /// Which way a rover setting off down `segment` is pointing.
    fn direction_leaving(app: &App, segment: Entity) -> Vec3 {
        component_of::<RoadSegment>(app, segment)
            .map(|segment| segment.arc.tangent_at(segment.from))
            .unwrap_or(Vec3::NAN)
    }

    #[test]
    fn a_click_no_arc_can_turn_tightly_enough_to_reach_adds_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let path = nodes(&STRAIGHT[..2]);
        click_at(&mut app, path[0]);
        click_at(&mut app, path[1]);

        click_at(
            &mut app,
            corner_of(STRAIGHT[1], Vec3::Z * MAP_TILE_WIDTH / 2.),
        );
        finish_the_road(&mut app);

        assert!(a_road_runs_through(&mut app, &STRAIGHT[..2]));
    }

    #[test]
    fn a_road_begun_on_open_ground_sets_off_towards_the_node_it_was_aimed_at() {
        let mut app = app_holding(PlayerAction::EditRoads);

        place_road(&mut app, &nodes(&TURNING));

        let road = road_through(&mut app, &TURNING);
        let lane = lane_from(&app, road, tiles(&TURNING)[0]);
        let setting_off = direction_leaving(&app, lane[0]);

        assert!(setting_off.abs_diff_eq(Vec3::X, TOLERANCE), "{setting_off}");
    }

    #[test]
    fn a_road_begun_on_another_s_end_leaves_it_without_a_kink() {
        let mut app = app_holding(PlayerAction::EditRoads);
        place_road(&mut app, &nodes(&STRAIGHT));

        place_road(&mut app, &nodes(&ONWARD));

        let onward = road_through(&mut app, &ONWARD);
        let lane = lane_from(&app, onward, tiles(&ONWARD)[0]);
        let setting_off = direction_leaving(&app, lane[0]);

        assert!(setting_off.abs_diff_eq(Vec3::X, TOLERANCE), "{setting_off}");
    }

    #[test]
    fn putting_the_tool_down_mid_road_lays_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        for &node in &nodes(&STRAIGHT) {
            click_at(&mut app, node);
        }

        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::Select);
        tick(&mut app);
        finish_the_road(&mut app);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    /// Lay `STRAIGHT`, then place a road that arrives on the node in the middle of it.
    fn a_road_placed_onto_another() -> App {
        let mut app = app_holding(PlayerAction::EditRoads);
        place_road(&mut app, &nodes(&STRAIGHT));

        let meeting = nodes(&CROSSING);
        click_at(&mut app, meeting[0]);
        click_at(&mut app, meeting[1]);
        tick(&mut app);
        app
    }

    #[test]
    fn arriving_on_a_road_finishes_the_one_being_placed() {
        let mut app = a_road_placed_onto_another();

        assert!(a_road_runs_through(&mut app, &CROSSING[..2]));
        assert_eq!(placing(&mut app), 0);
    }

    #[test]
    fn the_road_it_arrived_on_is_split_at_the_node_they_share() {
        let mut app = a_road_placed_onto_another();

        assert!(a_road_runs_through(&mut app, &STRAIGHT[..3]));
        assert!(a_road_runs_through(&mut app, &STRAIGHT[2..]));
        assert_eq!(roads_in_the_world(&mut app), 3);
    }

    #[test]
    fn every_road_a_meeting_leaves_behind_gets_its_lanes() {
        let mut app = a_road_placed_onto_another();

        let laid: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .collect();

        assert_eq!(laid.len(), 3);
        for road in laid {
            assert!(!app.world().entity(road).contains::<InitializationFailed>());
            assert_eq!(lanes(&app, road).len(), 2);
        }
    }

    #[test]
    fn a_road_lays_one_arc_between_each_pair_of_nodes() {
        let path = nodes(&WINDING);

        let arcs = arcs_through(&path, None);

        assert_eq!(arcs.len(), path.len() - 1);
    }

    #[test]
    fn every_arc_of_a_road_ends_on_the_node_it_was_aimed_at() {
        let path = nodes(&WINDING);

        let arcs = arcs_through(&path, None);

        for (arc, node) in arcs.iter().zip(path.iter().skip(1)) {
            let end = arc.position(arc.length);
            assert!(
                end.distance(node.world_position()) < TOLERANCE,
                "{end} rather than {}",
                node.world_position()
            );
        }
    }

    #[test]
    fn every_arc_of_a_road_leaves_the_one_before_it_at_the_same_tangent() {
        let arcs = arcs_through(&nodes(&WINDING), None);

        for pair in arcs.windows(2) {
            let arriving = pair[0].tangent_at(pair[0].length);
            assert!(
                pair[1].tangent.abs_diff_eq(arriving, TOLERANCE),
                "{} rather than {arriving}",
                pair[1].tangent
            );
        }
    }

    #[test]
    fn a_road_begun_on_open_ground_starts_straight() {
        let arcs = arcs_through(&nodes(&TURNING), None);

        assert_eq!(arcs[0].curvature, 0.);
    }

    #[test]
    fn a_road_begun_on_a_direction_leaves_along_it() {
        let leaving = Vec3::new(0., 0., 1.);

        let arcs = arcs_through(&nodes(&STRAIGHT), Some(leaving));

        assert!(arcs[0].tangent.abs_diff_eq(leaving, TOLERANCE));
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
    fn a_road_placed_onto_the_end_of_another_leaves_it_whole() {
        let mut app = app_holding(PlayerAction::EditRoads);
        place_road(&mut app, &nodes(&STRAIGHT));

        place_road(&mut app, &nodes(&ONWARD));

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert!(a_road_runs_through(&mut app, &ONWARD));
        assert_eq!(roads_in_the_world(&mut app), 2);
    }
}
