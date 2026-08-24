use crate::common::cleanup::DestroyOnStateChange;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use crate::map::{HexCoordinates, LatticeNode, MapTile, MAP_TILE_INRADIUS, MAP_TILE_SIZE};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

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
/// A tile's inradius, so the bound comes off the grid rather than out of the air. It leaves the
/// sixty degree turn onto a neighbouring tile buildable, whose arc has a radius of one lattice
/// step, and refuses the same turn onto a neighbouring node, which would need two thirds of that.
/// Under it the nodes a road cannot reach from where it stands are the two discs of this radius
/// that touch its heading, one either side.
const MIN_TURN_RADIUS: f32 = MAP_TILE_INRADIUS;

/// How far off the heading a target may sit and still be aimed at straight.
///
/// A chain of arcs carries its tangent from one arc to the next, so a target that is dead ahead
/// arrives a rounding off the heading rather than exactly on it. Curving to meet that rounding
/// gives an arc of radius in the millions, whose centre is too far from the road for a position
/// on it to survive being computed in single precision.
const STRAIGHT_REACH: f32 = 1e-3;

/// How far a rover may travel in one tick on a segment that does not turn at all.
///
/// In world units, so a rover on the straight crosses a tile every sixty-four ticks. Nothing in
/// gameplay measures in seconds (invariant 2): running the world faster runs more ticks rather
/// than longer ones, and this is untouched by that.
const STRAIGHT_SPEED_LIMIT: f32 = MAP_TILE_SIZE / 64.;

/// The tightest curve still driven at the straight-road limit, as a radius in world units.
///
/// Four tiles across. The sixty-degree corner between neighbouring tiles fits arcs of about one
/// and two thirds of a tile in radius, which comes out near two thirds of the straight limit:
/// enough that a sweeping road is worth the land it costs, and not so much that a corner is a
/// wall.
const COMFORTABLE_RADIUS: f32 = 4. * MAP_TILE_SIZE;

/// How far apart an arc is walked when the tiles it runs over are worked out.
///
/// Two samples this close land on one tile or on neighbours, so the walk crosses one boundary
/// at a time rather than stepping a tile over. What it does not report is a tile the arc only
/// clips the corner of for less than a step, which at a quarter of the inradius is an eighth of
/// the way across a tile: too little for anything to stand on.
const TILE_SAMPLE_STEP: f32 = MAP_TILE_INRADIUS / 4.;

/// How far the debug view lifts a lane off the ground, so it does not fight the tiles it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a lane's arcs are drawn in
const LANE_COLOUR: Color = Color::srgb(0.35, 0.75, 0.95);

/// The colour a lane is drawn in where its curve holds a rover to the slowest a segment gets
const SLOW_LANE_COLOUR: Color = Color::srgb(0.95, 0.35, 0.45);

/// The colour the step from one segment onto the next is drawn in
const HANDOVER_COLOUR: Color = Color::srgb(0.95, 0.8, 0.3);

/// The colour the road the player is still placing is drawn in
const DRAWING_COLOUR: Color = Color::srgb(0.6, 0.95, 0.6);

/// The colour the arc a click would lay is drawn in
const PROPOSAL_COLOUR: Color = Color::srgb(0.95, 0.95, 0.4);

/// The colour the ground a road cannot turn tightly enough to reach is drawn in
const UNREACHABLE_COLOUR: Color = Color::srgb(0.95, 0.35, 0.35);

/// The colour the tiles a road runs over are marked in
const OCCUPIED_COLOUR: Color = Color::srgb(0.95, 0.45, 0.35);

/// The colour a tile under the cursor is marked in when a road already runs over it
const TAKEN_COLOUR: Color = Color::srgb(0.95, 0.25, 0.2);

/// How wide the mark on an occupied tile is drawn, as a share of the tile's inradius.
const OCCUPIED_MARK: f32 = 0.35;

/// The colour a junction is marked in
const JUNCTION_COLOUR: Color = Color::srgb(0.9, 0.5, 0.95);

/// How wide a junction is marked, as a share of the tile's inradius.
const JUNCTION_MARK: f32 = 0.25;

/// How far apart two points may stand and still be the same crossing.
///
/// Two roads meeting at a node are reached by every pair of arcs that ends there, so the same
/// point comes back several times over and has to be gathered into one junction rather than one
/// each. It is also what says a point is on an arc at all rather than beside it.
const CROSSING_TOLERANCE: f32 = 1e-3;

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
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// A place two roads cross, and how far along each of them the crossing stands.
///
/// It is a distance along an arc rather than a node of its own, so putting one in cuts the
/// segments that cover that distance and rewrites neither arc: a road crossed in its middle stays
/// exactly where it was drawn, however often it is crossed (invariant 6). Which of its arms a
/// rover may leave by, and who goes first, belongs to #68.
#[derive(Component)]
pub struct Junction {
    /// Where on the ground the roads cross.
    pub at: Vec3,
    /// The arcs that reach the crossing, and how far along each of them it stands.
    pub across: Vec<Crossing>,
}

/// How far into one of a road's arcs a junction cuts it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    /// The road the arc belongs to.
    pub road: Entity,
    /// Which of the road's arcs the junction stands on, counted from the end it was begun at.
    pub arc: usize,
    /// How far along that arc it stands.
    pub along: f32,
}

/// A road that has been measured against the roads already laid for the places it crosses them.
#[derive(Component)]
struct Crossed;

/// Which roads run over each tile of the map, and which tiles each road runs over.
///
/// Under #4 a road was a run of tiles and this was a lookup; under #93 an arc runs over the grid
/// rather than along it, and the answer has to be walked out of the geometry. It is walked once,
/// when the road is laid, and read back both ways by a key, because a rule about what may stand on
/// a tile has to answer without measuring every arc on the map.
#[derive(Resource, Default)]
pub struct RoadTiles {
    over: HashMap<HexCoordinates, Vec<Entity>>,
    under: HashMap<Entity, Vec<HexCoordinates>>,
}

#[derive(SystemParam)]
struct RoadInitializeParams<'w, 's> {
    commands: Commands<'w, 's>,
    occupied: ResMut<'w, RoadTiles>,
}

impl Plugin for RoadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadTiles>()
            .declare_bindings([
                Binding {
                    input: BindingInput::Mouse(MouseButton::Left),
                    action: "Place a road node, or finish on a road already there",
                    context: BindingContext::Tool(PlayerAction::EditRoads),
                },
                Binding {
                    input: BindingInput::Mouse(MouseButton::Right),
                    action: "Finish the road",
                    context: BindingContext::Tool(PlayerAction::EditRoads),
                },
            ])
            .add_observer(release_the_tiles_of_a_removed_road)
            .add_observer(forget_a_removed_road_at_the_junctions_on_it)
            .add_systems(
                PreUpdate,
                (
                    initialize_system::<Road, RoadInitializeParams>,
                    cut_the_roads_where_they_cross,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    (place_a_node, lay_the_road).chain(),
                    draw_the_lanes,
                    draw_the_junctions,
                    draw_the_road_being_placed,
                    draw_the_occupied_tiles,
                    draw_the_road_under_the_cursor,
                ),
            );
    }
}

impl RoadTiles {
    /// The roads running over `tile`, of which there is more than one where two of them meet.
    pub fn roads_over(&self, tile: HexCoordinates) -> &[Entity] {
        self.over.get(&tile).map_or(&[], Vec::as_slice)
    }

    /// The tiles `road` runs over, in the order its arcs reach them.
    pub fn tiles_under(&self, road: Entity) -> &[HexCoordinates] {
        self.under.get(&road).map_or(&[], Vec::as_slice)
    }

    fn claim(&mut self, road: Entity, tiles: Vec<HexCoordinates>) {
        for &tile in &tiles {
            self.over.entry(tile).or_default().push(road);
        }
        self.under.insert(road, tiles);
    }

    fn release(&mut self, road: Entity) {
        for tile in self.under.remove(&road).unwrap_or_default() {
            let Some(claimants) = self.over.get_mut(&tile) else {
                continue;
            };
            claimants.retain(|&claimant| claimant != road);
            if claimants.is_empty() {
                self.over.remove(&tile);
            }
        }
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

    /// Where the circle this arc lies on has its middle, which a straight has nowhere.
    fn centre(&self) -> Vec3 {
        self.start + left_of(self.tangent) / self.curvature
    }

    /// How far that middle is from the arc.
    fn radius(&self) -> f32 {
        (1. / self.curvature).abs()
    }

    /// How far along this arc `point` stands, or nothing where it is off the curve or past an end.
    ///
    /// A point beside the curve is not on this arc, and one on the circle the arc lies on but past
    /// either end is not on it either, so both answer nothing rather than the nearest place.
    fn distance_along(&self, point: Vec3) -> Option<f32> {
        let at = if self.curvature == 0. {
            let reach = point - self.start;
            let at = reach.dot(self.tangent);
            if (reach - self.tangent * at).length() > CROSSING_TOLERANCE {
                return None;
            }
            at
        } else {
            let centre = self.centre();
            let (from, to) = (self.start - centre, point - centre);
            if (to.length() - self.radius()).abs() > CROSSING_TOLERANCE {
                return None;
            }
            driven(turn_of(from, to).atan2(from.dot(to)), self.curvature) / self.curvature
        };

        (at >= -CROSSING_TOLERANCE && at <= self.length + CROSSING_TOLERANCE)
            .then(|| at.clamp(0., self.length))
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

/// The turn `angle` is, measured the way a road of `curvature` drives rather than the shorter way.
///
/// A turn just short of nothing reads as a turn just short of the whole circle when it is measured
/// backwards, so the tolerance the crossing is found to is what separates the two.
fn driven(angle: f32, curvature: f32) -> f32 {
    let behind = CROSSING_TOLERANCE * curvature.abs();
    match curvature > 0. {
        true if angle < -behind => angle + std::f32::consts::TAU,
        false if angle > behind => angle - std::f32::consts::TAU,
        _ => angle,
    }
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

    /// How long the stretch of arc this segment covers is, in world units.
    pub fn length(&self) -> f32 {
        self.to - self.from
    }

    /// How far a rover may travel along this segment in one tick.
    ///
    /// Read off the arc's curvature rather than stored beside it, which is what makes a tight turn
    /// a cost the player trades land against rather than a rule the build tool enforces. An arc
    /// holds one curvature along its whole length, so there is no spike anywhere to read the wrong
    /// number off, and two segments cut from one arc are equally fast however often the road
    /// between them was cut (invariant 6).
    pub fn speed_limit(&self) -> f32 {
        let radius = self.arc.curvature.abs().recip();
        STRAIGHT_SPEED_LIMIT * (radius / COMFORTABLE_RADIUS).sqrt().min(1.)
    }
}

impl Initialize<RoadInitializeParams<'_, '_>> for Road {
    fn initialize(&mut self, entity: &Entity, params: &mut RoadInitializeParams) -> Result {
        let along = arcs_through(&self.nodes, self.leaving);
        if along.is_empty() {
            return Err("a road of no arcs".into());
        }
        params.occupied.claim(*entity, tiles_walked_by(&along));
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

/// Give up the tiles a road held, whichever way it left the world.
fn release_the_tiles_of_a_removed_road(removed: On<Remove, Road>, mut occupied: ResMut<RoadTiles>) {
    occupied.release(removed.entity);
}

/// The tiles `arcs` run over, each of them reported once.
///
/// Both ends of every arc are stood on exactly rather than merely walked near, so a road is always
/// found under the tiles its own nodes stand on: a node is where one arc ends and the next begins.
fn tiles_walked_by(arcs: &[Arc]) -> Vec<HexCoordinates> {
    let mut walked: Vec<HexCoordinates> = Vec::new();
    for arc in arcs {
        for at in walk_of(arc) {
            let tile = HexCoordinates::from_world_position(arc.position(at));
            if !walked.contains(&tile) {
                walked.push(tile);
            }
        }
    }
    walked
}

/// How far along `arc` each place it is stood on stands, its far end included.
fn walk_of(arc: &Arc) -> impl Iterator<Item = f32> {
    let length = arc.length;
    let steps = (length / TILE_SAMPLE_STEP).ceil().max(1.);
    (0..=steps as usize).map(move |step| length * step as f32 / steps)
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
/// joined to the network. Neither road is taken apart by the meeting, which is cut into both of
/// them as a junction instead. A road of a single node is no road and lays nothing.
fn lay_the_road(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    placing: Query<(Entity, &DrawnRoad)>,
    roads: Query<&Road>,
) {
    for (entity, placed) in &placing {
        if !player_input.finish && !reaches_a_road(placed, &roads) {
            continue;
        }
        commands.entity(entity).despawn();

        if placed.nodes.len() < 2 {
            continue;
        }
        commands.spawn(Road {
            nodes: placed.nodes.clone(),
            leaving: placed.leaving,
        });
    }
}

/// Whether the road being placed has arrived on a road that is already there.
fn reaches_a_road(placed: &DrawnRoad, roads: &Query<&Road>) -> bool {
    let Some(reached) = placed.nodes.last().filter(|_| placed.nodes.len() > 1) else {
        return false;
    };
    roads.iter().any(|road| road.nodes.contains(reached))
}

/// Put a junction wherever a road just laid crosses one that was already there.
///
/// It runs beside the initialization that lays a road, so the crossing is worked out from the arcs
/// once and is a fact of record after that (invariant 3), and a frame that laid no road does no
/// work at all. A road is measured against the roads laid alongside it on the same frame as well
/// as against the ones already standing, and against each of them once.
fn cut_the_roads_where_they_cross(
    mut commands: Commands,
    roads: Query<(Entity, &Road, Has<Crossed>), Without<NeedsInitialization>>,
    children: Query<&Children>,
    mut segments: Query<(&mut RoadSegment, Option<&NextSegment>)>,
    mut junctions: Query<&mut Junction>,
) {
    let laid: Vec<(Entity, Vec<Arc>, bool)> = roads
        .iter()
        .map(|(entity, road, crossed)| (entity, arcs_through(&road.nodes, road.leaving), !crossed))
        .collect();
    if !laid.iter().any(|(.., fresh)| *fresh) {
        return;
    }

    let mut found: Vec<(Vec3, Vec<Crossing>)> = Vec::new();
    for (taken, (road, arcs, fresh)) in laid.iter().enumerate() {
        if *fresh {
            commands.entity(*road).insert(Crossed);
        }
        for (other, other_arcs, other_fresh) in &laid[taken + 1..] {
            if !fresh && !other_fresh {
                continue;
            }
            gather_the_crossings(*road, arcs, *other, other_arcs, &mut found);
        }
    }

    for (at, across) in found {
        for road in roads_crossing(&across) {
            cut_the_segments_of(road, at, &children, &mut segments, &mut commands);
        }
        match junctions
            .iter_mut()
            .find(|junction| junction.at.distance(at) <= CROSSING_TOLERANCE)
        {
            Some(mut junction) => {
                for crossing in across {
                    note(&mut junction.across, crossing);
                }
            }
            None => {
                commands.spawn(Junction { at, across });
            }
        }
    }
}

/// The roads a crossing stands on, each once however many of its arcs reach the point.
fn roads_crossing(across: &[Crossing]) -> Vec<Entity> {
    let mut roads: Vec<Entity> = Vec::new();
    for crossing in across {
        if !roads.contains(&crossing.road) {
            roads.push(crossing.road);
        }
    }
    roads
}

/// Note where two roads' arcs cross, gathering a point met by several pairs into one crossing.
fn gather_the_crossings(
    road: Entity,
    arcs: &[Arc],
    other: Entity,
    other_arcs: &[Arc],
    found: &mut Vec<(Vec3, Vec<Crossing>)>,
) {
    for (index, arc) in arcs.iter().enumerate() {
        for (other_index, other_arc) in other_arcs.iter().enumerate() {
            for (at, along, other_along) in crossings_of(arc, other_arc) {
                let met = met_at(found, at);
                note(
                    &mut found[met].1,
                    Crossing {
                        road,
                        arc: index,
                        along,
                    },
                );
                note(
                    &mut found[met].1,
                    Crossing {
                        road: other,
                        arc: other_index,
                        along: other_along,
                    },
                );
            }
        }
    }
}

/// Which crossing of `found` stands at `at`, opened where nothing has been found there yet.
fn met_at(found: &mut Vec<(Vec3, Vec<Crossing>)>, at: Vec3) -> usize {
    match found
        .iter()
        .position(|(met, _)| met.distance(at) <= CROSSING_TOLERANCE)
    {
        Some(met) => met,
        None => {
            found.push((at, Vec::new()));
            found.len() - 1
        }
    }
}

fn note(across: &mut Vec<Crossing>, crossing: Crossing) {
    if !across
        .iter()
        .any(|noted| noted.road == crossing.road && noted.arc == crossing.arc)
    {
        across.push(crossing);
    }
}

/// Cut every segment of `road` that covers `at` in two, both halves on the arc it already had.
///
/// The arc is copied rather than worked out again, so the halves hold the same curve to the bit
/// and neither of them has moved. A crossing that lands on an end of a segment cuts nothing: the
/// junction already stands where two segments meet.
fn cut_the_segments_of(
    road: Entity,
    at: Vec3,
    children: &Query<&Children>,
    segments: &mut Query<(&mut RoadSegment, Option<&NextSegment>)>,
    commands: &mut Commands,
) {
    let Ok(lanes) = children.get(road) else {
        return;
    };
    for lane in lanes.iter() {
        let Ok(pieces) = children.get(lane) else {
            continue;
        };
        for piece in pieces.iter() {
            let Ok((segment, onward)) = segments.get(piece) else {
                continue;
            };
            let (arc, from, to) = (segment.arc, segment.from, segment.to);
            let onward = onward.map(|onward| onward.0);
            let Some(along) = arc.distance_along(at) else {
                continue;
            };
            if along <= from + CROSSING_TOLERANCE || along >= to - CROSSING_TOLERANCE {
                continue;
            }

            let cut = commands
                .spawn((
                    RoadSegment {
                        arc,
                        from: along,
                        to,
                    },
                    ChildOf(lane),
                ))
                .id();
            if let Some(onward) = onward {
                commands.entity(cut).insert(NextSegment(onward));
            }
            commands.entity(piece).insert(NextSegment(cut));
            if let Ok((mut segment, _)) = segments.get_mut(piece) {
                segment.to = along;
            }
        }
    }
}

/// Where two arcs cross, and how far along each of them the crossing stands.
///
/// A straight is an arc of zero curvature, so a pair is a line meeting a line, a line meeting a
/// circle, or two circles meeting. What those answer is a point of the whole line or the whole
/// circle, which is a crossing only where both arcs reach as far as it.
fn crossings_of(one: &Arc, other: &Arc) -> Vec<(Vec3, f32, f32)> {
    let meetings = match (one.curvature == 0., other.curvature == 0.) {
        (true, true) => where_the_lines_meet(one, other),
        (true, false) => where_a_line_meets_a_circle(one, other),
        (false, true) => where_a_line_meets_a_circle(other, one),
        (false, false) => where_the_circles_meet(one, other),
    };

    meetings
        .into_iter()
        .filter_map(|at| Some((at, one.distance_along(at)?, other.distance_along(at)?)))
        .collect()
}

/// The one point two straights meet at, which two running the same way have nowhere.
fn where_the_lines_meet(one: &Arc, other: &Arc) -> Vec<Vec3> {
    let crossing = turn_of(one.tangent, other.tangent);
    if crossing.abs() < STRAIGHT_REACH {
        return Vec::new();
    }
    vec![one.start + one.tangent * (turn_of(other.start - one.start, other.tangent) / crossing)]
}

/// The points a straight meets a circle at, of which there are two unless it misses or grazes.
fn where_a_line_meets_a_circle(line: &Arc, arc: &Arc) -> Vec<Vec3> {
    let reach = line.start - arc.centre();
    let towards = reach.dot(line.tangent);
    let beyond = towards * towards - reach.length_squared() + arc.radius() * arc.radius();
    if beyond < 0. {
        return Vec::new();
    }

    let step = beyond.sqrt();
    vec![
        line.start + line.tangent * (step - towards),
        line.start + line.tangent * (-step - towards),
    ]
}

/// The points two circles meet at, of which there are two unless one misses, holds or graze.
fn where_the_circles_meet(one: &Arc, other: &Arc) -> Vec<Vec3> {
    let (centre, middle) = (one.centre(), other.centre());
    let between = middle - centre;
    let apart = between.length();
    if apart == 0. {
        return Vec::new();
    }

    let (radius, span) = (one.radius(), other.radius());
    let along = (radius * radius - span * span + apart * apart) / (2. * apart);
    let across = radius * radius - along * along;
    if across < 0. {
        return Vec::new();
    }

    let met = centre + between * (along / apart);
    let sideways = left_of(between) * (across.sqrt() / apart);
    vec![met + sideways, met - sideways]
}

/// Drop a removed road from the junctions on it, and with it any junction left on one road alone.
fn forget_a_removed_road_at_the_junctions_on_it(
    removed: On<Remove, Road>,
    mut commands: Commands,
    mut junctions: Query<(Entity, &mut Junction)>,
) {
    for (entity, mut junction) in &mut junctions {
        if !junction
            .across
            .iter()
            .any(|crossing| crossing.road == removed.entity)
        {
            continue;
        }
        junction
            .across
            .retain(|crossing| crossing.road != removed.entity);

        let left = junction.across.first().map(|crossing| crossing.road);
        if junction
            .across
            .iter()
            .all(|crossing| Some(crossing.road) == left)
        {
            commands.entity(entity).despawn();
        }
    }
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

/// Mark every junction, which two lanes drawn across each other do not say is there.
///
/// A crossing is a point on both roads rather than anything either of them stores, so a road
/// drawn over another looks exactly like a road drawn beside it until the junction is drawn.
fn draw_the_junctions(mut gizmos: Gizmos<DebugGizmos>, junctions: Query<&Junction>) {
    for junction in &junctions {
        gizmos.circle(
            Isometry3d::new(junction.at + GIZMO_LIFT, Quat::from_rotation_x(FRAC_PI_2)),
            MAP_TILE_INRADIUS * JUNCTION_MARK,
            JUNCTION_COLOUR,
        );
    }
}

/// Mark every tile a road runs over, which the lanes drawn across them do not say.
///
/// A road curves over the grid rather than following it, so which tiles it takes is a question the
/// map cannot be looked at to answer, and a lane drawn over a corner of one looks the same as a
/// lane that misses it.
fn draw_the_occupied_tiles(
    mut gizmos: Gizmos<DebugGizmos>,
    occupied: Res<RoadTiles>,
    roads: Query<Entity, With<Road>>,
) {
    for road in &roads {
        for tile in occupied.tiles_under(road) {
            gizmos.circle(
                Isometry3d::new(
                    tile.world_position() + GIZMO_LIFT,
                    Quat::from_rotation_x(FRAC_PI_2),
                ),
                MAP_TILE_INRADIUS * OCCUPIED_MARK,
                OCCUPIED_COLOUR,
            );
        }
    }
}

/// Mark the tile under the cursor when a road already runs over it.
///
/// This is the tile question asked from the other end, and the one #98 refuses a building with.
/// Reading it off a road would mean reading every road, which is what the tile is keyed for.
fn draw_the_road_under_the_cursor(
    mut gizmos: Gizmos<DebugGizmos>,
    occupied: Res<RoadTiles>,
    player_input: Res<PlayerInput>,
    tiles: Query<&MapTile>,
) {
    let Some(tile) = player_input
        .cursor_tile
        .and_then(|tile| tiles.get(tile).ok())
        .map(|tile| tile.coordinates)
    else {
        return;
    };
    if occupied.roads_over(tile).is_empty() {
        return;
    }

    gizmos.circle(
        Isometry3d::new(
            tile.world_position() + GIZMO_LIFT,
            Quat::from_rotation_x(FRAC_PI_2),
        ),
        MAP_TILE_INRADIUS,
        TAKEN_COLOUR,
    );
}

/// Draw every lane, the order a rover drives its segments in, and how fast each of them allows.
///
/// A chain of segments is otherwise only visible in a test: two lanes lying on the same road look
/// like one road, and the join at a dead end looks like a rover turning round of its own accord.
/// A lane's colour is its speed limit, so the cost of a corner can be seen while the road is being
/// built rather than inferred afterwards from rovers arriving late.
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
            SLOW_LANE_COLOUR.mix(&LANE_COLOUR, segment.speed_limit() / STRAIGHT_SPEED_LIMIT),
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
    use std::collections::HashSet;

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// How many straight pieces a segment is measured in.
    const LENGTH_SAMPLES: usize = 128;

    /// A straight run of tiles, in offset-row coordinates.
    const STRAIGHT: [(i32, i32); 4] = [(0, 0), (1, 0), (2, 0), (3, 0)];

    /// A run of tiles that turns a corner, in offset-row coordinates.
    const TURNING: [(i32, i32); 3] = [(0, 0), (1, 0), (1, 1)];

    /// A run of tiles turning `TURNING`'s corner over twice the ground, in offset-row coordinates.
    ///
    /// An arc turns by twice the angle its target sits off the heading, so aiming further off does
    /// not tighten it past a point: sixty degrees off and a hundred and twenty give the same
    /// radius. Reaching the same corner over twice the distance is what draws it twice as wide.
    const WIDE_CORNER: [(i32, i32); 3] = [(0, 0), (2, 0), (3, 2)];

    /// A run of tiles that runs straight and then turns twice, in offset-row coordinates.
    const WINDING: [(i32, i32); 5] = [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)];

    /// A run of tiles crossing `STRAIGHT` at its third tile, in offset-row coordinates.
    const CROSSING: [(i32, i32); 3] = [(2, -1), (2, 0), (2, 1)];

    /// Nodes far enough apart that the arcs between them cross tiles of their own, in offset-row
    /// coordinates. A drag records the nodes the cursor reached and nothing between them, so this
    /// is what a flick lays and what the road tool hands the walk to measure.
    const SWEEPING: [(i32, i32); 3] = [(0, 0), (3, 0), (3, 3)];

    /// Pairs of nodes whose runs cross the grid at lengths and angles of their own, in offset-row
    /// coordinates. One length would only say the walk lands on tile middles as often as it is
    /// spaced to; several say it lands on every tile between them whatever it is spaced against.
    const SPANNING: [[(i32, i32); 2]; 6] = [
        [(0, 0), (5, 0)],
        [(0, 0), (0, 5)],
        [(0, 0), (4, 3)],
        [(0, 0), (-3, 4)],
        [(0, 0), (5, -4)],
        [(0, 0), (-6, -1)],
    ];

    /// A run of tiles setting off from the last tile of `STRAIGHT`, in offset-row coordinates.
    const ONWARD: [(i32, i32); 2] = [(3, 0), (3, 1)];

    /// A direction from a tile's middle far enough towards a corner of it to settle on that corner.
    const TOWARDS_A_CORNER: Vec3 = Vec3::new(0., 0., MAP_TILE_INRADIUS);

    /// A direction from a tile's middle far enough towards the corner two round from that one.
    const TOWARDS_THE_CORNER_TWO_ROUND: Vec3 =
        Vec3::new(MAP_TILE_INRADIUS, 0., -MAP_TILE_INRADIUS / 2.);

    /// A straight run crossing the curve of `TURNING` between its nodes, in offset-row coordinates.
    const ACROSS_THE_CURVE: [(i32, i32); 2] = [(0, 2), (2, 0)];

    /// A run that turns a corner of its own and crosses the curve of `TURNING` while it is turning,
    /// in offset-row coordinates. Both roads meet on an arc, which is the pair of circles.
    const CURVING_ACROSS: [(i32, i32); 3] = [(2, -1), (2, 0), (0, 1)];

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
        spawn_road_through(app, &nodes(offsets))
    }

    fn spawn_road_through(app: &mut App, path: &[LatticeNode]) -> Entity {
        app.world_mut()
            .spawn(Road {
                nodes: path.to_vec(),
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

    /// How fast every segment of the lane of `road` setting off from `tile` allows.
    fn speed_limits_along(app: &App, road: Entity, tile: HexCoordinates) -> Vec<f32> {
        lane_from(app, road, tile)
            .into_iter()
            .filter_map(|segment| component_of::<RoadSegment>(app, segment))
            .map(RoadSegment::speed_limit)
            .collect()
    }

    /// The lowest speed limit anywhere on `road`, which is its tightest curve.
    fn slowest_on(app: &App, road: Entity) -> f32 {
        lanes(app, road)
            .into_iter()
            .flat_map(|lane| children_of(app, lane))
            .filter_map(|segment| component_of::<RoadSegment>(app, segment))
            .map(RoadSegment::speed_limit)
            .fold(f32::INFINITY, f32::min)
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
            corner_of(STRAIGHT[1], Vec3::Z * MAP_TILE_INRADIUS),
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
    fn the_road_it_arrived_on_is_left_whole() {
        let mut app = a_road_placed_onto_another();

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert_eq!(roads_in_the_world(&mut app), 2);
    }

    #[test]
    fn both_roads_of_a_meeting_keep_their_lanes() {
        let mut app = a_road_placed_onto_another();

        let laid: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .collect();

        assert_eq!(laid.len(), 2);
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
    /// The two corners of the tile at `offset` a road drawn between crosses `STRAIGHT` at.
    ///
    /// Neither corner stands on `STRAIGHT`, and the straight between them meets it two thirds of
    /// the way along itself, which is a node of neither road and an end of no segment of either.
    fn crossing_arm(offset: (i32, i32)) -> Vec<LatticeNode> {
        vec![
            corner_of(offset, TOWARDS_A_CORNER),
            corner_of(offset, TOWARDS_THE_CORNER_TWO_ROUND),
        ]
    }

    /// `STRAIGHT`, and a road laid across the middle of it a frame later.
    fn a_crossed_road() -> (App, Entity, Entity) {
        let mut app = road_app();
        let crossed = spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        let crossing = spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);
        (app, crossed, crossing)
    }

    /// Every junction in the world, as where it stands and the arcs that reach it.
    fn junctions(app: &mut App) -> Vec<(Vec3, Vec<Crossing>)> {
        app.world_mut()
            .query::<&Junction>()
            .iter(app.world())
            .map(|junction| (junction.at, junction.across.clone()))
            .collect()
    }

    /// The one junction in the world.
    fn the_junction(app: &mut App) -> (Vec3, Vec<Crossing>) {
        let mut found = junctions(app);
        let one = found.pop();
        assert!(found.is_empty(), "more than one junction");
        one.expect("a junction")
    }

    fn segments_under(app: &App, road: Entity) -> Vec<Entity> {
        lanes(app, road)
            .into_iter()
            .flat_map(|lane| children_of(app, lane))
            .collect()
    }

    /// The arc under every segment of `road`, on either of its lanes.
    fn arcs_under(app: &App, road: Entity) -> Vec<Arc> {
        segments_under(app, road)
            .into_iter()
            .filter_map(|segment| component_of::<RoadSegment>(app, segment).map(|piece| piece.arc))
            .collect()
    }

    /// Where the straight from `from` to `to` crosses the straight from `across` to `beyond`.
    fn where_the_straights_cross(from: Vec3, to: Vec3, across: Vec3, beyond: Vec3) -> Vec3 {
        let along = to - from;
        let other = beyond - across;
        from + along * (turn_of(across - from, other) / turn_of(along, other))
    }

    #[test]
    fn a_road_drawn_across_another_crosses_it_between_its_nodes() {
        let (mut app, ..) = a_crossed_road();
        let crossed = nodes(&STRAIGHT[1..3]);
        let arm = crossing_arm(STRAIGHT[1]);
        let met = where_the_straights_cross(
            crossed[0].world_position(),
            crossed[1].world_position(),
            arm[0].world_position(),
            arm[1].world_position(),
        );

        let (at, _) = the_junction(&mut app);

        assert!(
            at.distance(met) < TOLERANCE,
            "a junction at {at}, not {met}"
        );
    }

    #[test]
    fn a_junction_names_both_the_roads_that_cross_at_it() {
        let (mut app, crossed, crossing) = a_crossed_road();

        let (_, across) = the_junction(&mut app);

        let roads: HashSet<Entity> = across.iter().map(|crossing| crossing.road).collect();
        assert_eq!(roads, HashSet::from([crossed, crossing]));
    }

    /// The arc a crossing is recorded against, worked out from the road's own nodes.
    fn arc_of(app: &App, crossing: &Crossing) -> Option<Arc> {
        let road = component_of::<Road>(app, crossing.road)?;
        arcs_through(&road.nodes, road.leaving)
            .get(crossing.arc)
            .copied()
    }

    /// Whether every arc a junction names puts the crossing where the junction stands.
    fn stands_where_it_says(app: &App, at: Vec3, across: &[Crossing]) -> bool {
        !across.is_empty()
            && across.iter().all(|crossing| {
                arc_of(app, crossing)
                    .is_some_and(|arc| arc.position(crossing.along).distance(at) < TOLERANCE)
            })
    }

    /// Whether a junction was found on a curve of `road` rather than on a straight run of it.
    fn found_on_a_curve_of(app: &App, road: Entity, across: &[Crossing]) -> bool {
        across
            .iter()
            .filter(|crossing| crossing.road == road)
            .any(|crossing| arc_of(app, crossing).is_some_and(|arc| arc.curvature != 0.))
    }

    #[test]
    fn a_junction_stands_at_a_distance_along_the_arc_of_each_road_it_crosses() {
        let (mut app, ..) = a_crossed_road();

        let (at, across) = the_junction(&mut app);

        assert!(
            stands_where_it_says(&app, at, &across),
            "{across:?} at {at}"
        );
    }

    #[test]
    fn a_straight_road_drawn_across_a_curve_crosses_it_on_the_arc() {
        let mut app = road_app();
        let curved = spawn_road(&mut app, &TURNING);
        tick(&mut app);
        spawn_road(&mut app, &ACROSS_THE_CURVE);
        tick(&mut app);

        let (at, across) = the_junction(&mut app);

        assert!(
            stands_where_it_says(&app, at, &across),
            "{across:?} at {at}"
        );
        assert!(found_on_a_curve_of(&app, curved, &across), "{across:?}");
    }

    #[test]
    fn two_curves_drawn_across_each_other_cross_on_both_arcs() {
        let mut app = road_app();
        let curved = spawn_road(&mut app, &TURNING);
        tick(&mut app);
        let curving = spawn_road(&mut app, &CURVING_ACROSS);
        tick(&mut app);

        let (at, across) = the_junction(&mut app);

        assert!(
            stands_where_it_says(&app, at, &across),
            "{across:?} at {at}"
        );
        assert!(found_on_a_curve_of(&app, curved, &across), "{across:?}");
        assert!(found_on_a_curve_of(&app, curving, &across), "{across:?}");
    }

    #[test]
    fn crossing_a_curve_moves_none_of_its_arcs() {
        let mut app = road_app();
        let curved = spawn_road(&mut app, &TURNING);
        tick(&mut app);
        let before = arcs_under(&app, curved);

        spawn_road(&mut app, &ACROSS_THE_CURVE);
        tick(&mut app);

        let after = arcs_under(&app, curved);
        assert!(after.len() > before.len(), "a curve that was never cut");
        for arc in after {
            assert!(before.contains(&arc), "an arc that moved: {arc:?}");
        }
    }

    #[test]
    fn crossing_a_road_moves_none_of_its_arcs() {
        let mut app = road_app();
        let crossed = spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        let before = arcs_under(&app, crossed);

        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        let after = arcs_under(&app, crossed);
        assert!(after.len() > before.len(), "a road that was never cut");
        for arc in after {
            assert!(before.contains(&arc), "an arc that moved: {arc:?}");
        }
    }

    #[test]
    fn the_segments_either_side_of_a_junction_are_intervals_of_one_arc() {
        let (mut app, crossed, _) = a_crossed_road();
        let (at, _) = the_junction(&mut app);

        let mut met = 0;
        for segment in segments_under(&app, crossed) {
            if position(&app, segment, 1.).distance(at) > TOLERANCE {
                continue;
            }
            let onward = next_of(&app, segment).expect("the segment past the junction");
            let before = component_of::<RoadSegment>(&app, segment).expect("the segment before");
            let after = component_of::<RoadSegment>(&app, onward).expect("the segment after");

            assert_eq!(before.arc, after.arc);
            assert_eq!(before.to, after.from);
            met += 1;
        }

        assert_eq!(met, lanes(&app, crossed).len(), "a lane that was not cut");
    }

    #[test]
    fn a_road_crossed_twice_is_cut_at_both_crossings() {
        let mut app = road_app();
        let crossed = spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        let before = segments_under(&app, crossed).len();

        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[2]));
        tick(&mut app);

        assert_eq!(junctions(&mut app).len(), 2);
        assert_eq!(
            segments_under(&app, crossed).len(),
            before + 2 * lanes(&app, crossed).len()
        );
    }

    #[test]
    fn two_roads_laid_on_the_same_frame_cross_each_other() {
        let mut app = road_app();
        spawn_road(&mut app, &STRAIGHT);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        assert_eq!(junctions(&mut app).len(), 1);
    }

    #[test]
    fn two_roads_that_meet_at_a_node_they_share_make_one_junction() {
        let mut app = a_road_placed_onto_another();

        let (at, _) = the_junction(&mut app);

        let met = nodes(&STRAIGHT)[2].world_position();
        assert!(
            at.distance(met) < TOLERANCE,
            "a junction at {at}, not {met}"
        );
    }

    #[test]
    fn a_road_that_reaches_no_other_makes_no_junction() {
        let (mut app, _) = built_road(&WINDING);

        assert!(junctions(&mut app).is_empty());
    }

    #[test]
    fn the_segments_of_a_crossed_road_still_drive_the_whole_of_it() {
        let (app, crossed, _) = a_crossed_road();
        let start = children_of(&app, lanes(&app, crossed)[0])[0];

        let mut driven = vec![start];
        while let Some(next) = next_of(&app, *driven.last().expect("the drive has a segment")) {
            if next == start {
                break;
            }
            driven.push(next);
        }

        assert_eq!(driven.len(), segments_under(&app, crossed).len());
    }

    #[test]
    fn a_removed_road_leaves_no_junction_naming_it() {
        let (mut app, crossed, _) = a_crossed_road();
        assert_eq!(junctions(&mut app).len(), 1);

        app.world_mut().entity_mut(crossed).despawn();
        tick(&mut app);

        assert!(junctions(&mut app).is_empty());
    }

    fn occupied_tiles(app: &App, road: Entity) -> Vec<HexCoordinates> {
        app.world()
            .resource::<RoadTiles>()
            .tiles_under(road)
            .to_vec()
    }

    fn roads_over(app: &App, offset: (i32, i32)) -> Vec<Entity> {
        app.world()
            .resource::<RoadTiles>()
            .roads_over(HexCoordinates::from_offset_row(offset.0, offset.1))
            .to_vec()
    }

    #[test]
    fn a_straight_road_occupies_exactly_the_tiles_it_was_drawn_through() {
        let (app, road) = built_road(&STRAIGHT);

        assert_eq!(
            occupied_tiles(&app, road)
                .into_iter()
                .collect::<HashSet<_>>(),
            tiles(&STRAIGHT).into_iter().collect::<HashSet<_>>()
        );
    }

    #[test]
    fn a_road_does_not_occupy_a_tile_it_runs_beside() {
        let (app, road) = built_road(&STRAIGHT);

        let occupied: HashSet<HexCoordinates> = occupied_tiles(&app, road).into_iter().collect();

        for beside in tiles(&[(0, 1), (1, 1), (2, -1)]) {
            assert!(!occupied.contains(&beside), "{beside:?} carries no road");
        }
    }

    #[test]
    fn a_turning_road_occupies_the_tiles_it_curves_through() {
        let (app, road) = built_road(&TURNING);

        let occupied: HashSet<HexCoordinates> = occupied_tiles(&app, road).into_iter().collect();

        for drawn in tiles(&TURNING) {
            assert!(occupied.contains(&drawn), "{drawn:?} was drawn through");
        }
    }

    #[test]
    fn every_tile_a_road_occupies_has_the_road_running_over_it() {
        /// How many places along each segment the road is looked for, far more closely spaced than
        /// the walk that worked the tiles out.
        const PLACES: usize = 512;

        let (app, road) = built_road(&SWEEPING);

        let mut driven: HashSet<HexCoordinates> = HashSet::new();
        for lane in lanes(&app, road) {
            for segment in children_of(&app, lane) {
                for place in 0..=PLACES {
                    let along = place as f32 / PLACES as f32;
                    driven.insert(HexCoordinates::from_world_position(position(
                        &app, segment, along,
                    )));
                }
            }
        }

        for occupied in occupied_tiles(&app, road) {
            assert!(driven.contains(&occupied), "{occupied:?} carries no road");
        }
    }

    #[test]
    fn a_road_occupies_the_tiles_its_arcs_cross_between_its_nodes() {
        let (app, road) = built_road(&SWEEPING);

        let occupied: HashSet<HexCoordinates> = occupied_tiles(&app, road).into_iter().collect();

        for between in tiles(&[(1, 0), (2, 0)]) {
            assert!(
                occupied.contains(&between),
                "{between:?} lies between nodes"
            );
        }
        assert!(
            occupied.len() > SWEEPING.len(),
            "the arcs cross no tile their nodes do not stand on"
        );
    }

    /// Whether `tile` and `other` are neighbours, which is to say one tile width apart. Anything
    /// further off stands at least half as far again.
    fn are_neighbours(tile: HexCoordinates, other: HexCoordinates) -> bool {
        let apart = tile.world_position().distance(other.world_position());
        (apart - MAP_TILE_INRADIUS * 2.).abs() < TOLERANCE
    }

    /// The tiles of `occupied` that can be walked to from the first of them, neighbour by
    /// neighbour.
    fn reachable_within(occupied: &[HexCoordinates]) -> Vec<HexCoordinates> {
        let mut reached: Vec<HexCoordinates> = occupied.iter().copied().take(1).collect();
        let mut standing = 0;
        while standing < reached.len() {
            let stood = reached[standing];
            for &tile in occupied {
                if !reached.contains(&tile) && are_neighbours(stood, tile) {
                    reached.push(tile);
                }
            }
            standing += 1;
        }
        reached
    }

    #[test]
    fn the_tiles_a_road_occupies_join_up_with_no_gap_between_them() {
        let spans = SPANNING.iter().map(|span| &span[..]).chain([&SWEEPING[..]]);
        for offsets in spans {
            let (app, road) = built_road(offsets);

            let occupied = occupied_tiles(&app, road);

            assert!(occupied.len() > offsets.len(), "{occupied:?}");
            assert_eq!(
                reachable_within(&occupied).len(),
                occupied.len(),
                "{occupied:?} falls apart"
            );
        }
    }

    #[test]
    fn a_tile_reports_the_road_running_over_it() {
        let (app, road) = built_road(&STRAIGHT);

        assert_eq!(roads_over(&app, STRAIGHT[2]), [road]);
    }

    #[test]
    fn a_tile_no_road_reaches_reports_none() {
        let (app, _) = built_road(&STRAIGHT);

        assert!(roads_over(&app, (0, 3)).is_empty());
    }

    #[test]
    fn a_removed_road_leaves_no_tile_occupied_by_it() {
        let (mut app, road) = built_road(&STRAIGHT);
        assert!(!occupied_tiles(&app, road).is_empty());

        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert!(occupied_tiles(&app, road).is_empty());
        for drawn in STRAIGHT {
            assert!(roads_over(&app, drawn).is_empty(), "{drawn:?} still taken");
        }
    }

    #[test]
    fn a_road_occupies_its_tiles_from_the_frame_it_was_laid() {
        /// How many frames the road is left standing before its tiles are read a second time.
        const SETTLING_FRAMES: usize = 8;

        let (mut app, road) = built_road(&STRAIGHT);
        let laid: HashSet<HexCoordinates> = occupied_tiles(&app, road).into_iter().collect();

        assert!(!laid.is_empty());
        for _ in 0..SETTLING_FRAMES {
            tick(&mut app);
        }

        assert_eq!(
            occupied_tiles(&app, road)
                .into_iter()
                .collect::<HashSet<_>>(),
            laid
        );
    }

    #[test]
    fn the_tile_two_roads_met_on_is_occupied_by_both_of_them() {
        let mut app = a_road_placed_onto_another();

        let laid: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .collect();
        let met = roads_over(&app, STRAIGHT[2]);

        assert_eq!(met.len(), laid.len());
        for road in met {
            assert!(laid.contains(&road), "a road that is no longer laid");
        }
    }
    #[test]
    fn a_straight_segment_is_the_fastest_a_segment_gets() {
        let (app, road) = built_road(&STRAIGHT);

        let limits = speed_limits_along(&app, road, tiles(&STRAIGHT)[0]);

        assert!(!limits.is_empty(), "the lane has no segments to be fast on");
        for limit in limits {
            assert!(
                (limit - STRAIGHT_SPEED_LIMIT).abs() < TOLERANCE,
                "{limit} against the straight limit of {STRAIGHT_SPEED_LIMIT}"
            );
        }
    }

    #[test]
    fn a_road_that_turns_is_slower_than_one_that_does_not() {
        let (straight, laid) = built_road(&STRAIGHT);
        let (turning, bent) = built_road(&TURNING);

        assert!(slowest_on(&turning, bent) < slowest_on(&straight, laid));
    }

    #[test]
    fn a_tighter_corner_is_slower_than_a_sweeping_one() {
        let (sweeping, wide) = built_road(&WIDE_CORNER);
        let (tight, corner) = built_road(&TURNING);

        let bend = slowest_on(&tight, corner);
        let curve = slowest_on(&sweeping, wide);
        assert!(
            bend < curve,
            "{bend} round the tighter corner against {curve}"
        );
    }

    #[test]
    fn a_road_is_as_fast_driven_one_way_as_the_other() {
        let path = tiles(&WINDING);
        let (app, road) = built_road(&WINDING);

        let there = speed_limits_along(&app, road, path[0]);
        let mut back = speed_limits_along(&app, road, path[WINDING.len() - 1]);
        back.reverse();

        assert_eq!(there.len(), back.len());
        for (there, back) in there.iter().zip(&back) {
            assert!((there - back).abs() < TOLERANCE, "{there} against {back}");
        }
    }
}
