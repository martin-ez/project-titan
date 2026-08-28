use crate::building::BuildingTiles;
use crate::common::cleanup::{Destroy, DestroyOnStateChange};
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use crate::map::{HexCoordinates, LatticeNode, MapTile, MAP_TILE_INRADIUS, MAP_TILE_SIZE};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
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

/// How far back from a crossing a junction takes its legs, so a turn through it has room to be an
/// arc.
///
/// Half a lattice step. It is what makes the sixty degree turn between neighbouring tiles come out
/// at exactly `MIN_TURN_RADIUS`: a junction is the sharpest corner a network has, and this leaves
/// it no sharper than the tightest arc the road tool will build.
const JUNCTION_PULLBACK: f32 = MAP_TILE_SIZE / 4.;

/// How much road a junction takes, as the stretch between the way in and the way out on one arm.
///
/// Two crossings closer than this have no room between them for either of their turns, so the
/// second is refused rather than laid over the first.
const JUNCTION_EXTENT: f32 = 2. * JUNCTION_PULLBACK;

/// How closely a fitted turn has to meet the leg it reaches to be laid as one arc.
///
/// Equal pull-backs on two straight legs put the one arc that leaves along the leg it arrives on
/// exactly along the leg it reaches, so the pair a curved leg needs is the exception rather than
/// the rule.
const TURN_TANGENT_REACH: f32 = 1e-3;

/// How many segments a way through a junction may be made of before it is given up on.
const STEPS_THROUGH_A_JUNCTION: usize = 8;

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

/// The colour the arc a click would take off the map is drawn in
const REMOVING_COLOUR: Color = Color::srgb(1., 0.4, 0.75);

/// How wide the mark on an occupied tile is drawn, as a share of the tile's inradius.
const OCCUPIED_MARK: f32 = 0.35;

/// How closely two arms of a junction have to point the same way to be one leg.
///
/// A road crossed in its middle reaches the crossing on one lane and leaves it on the other, and
/// the two are the same arm of the junction read in opposite directions, so what pairs them is
/// that they point the same way rather than that they belong to one road.
const LEG_TOLERANCE: f32 = 0.999;

/// How far out from a junction its legs are marked, as a share of the tile's inradius.
const LEG_MARK: f32 = 0.6;

/// The colour a leg that gives way is marked in
const GIVING_WAY_COLOUR: Color = Color::srgb(0.9, 0.7, 0.25);

/// The colour a leg with priority over the others is marked in
const PRIORITY_COLOUR: Color = Color::srgb(0.35, 0.85, 0.45);

/// The colour the link from a tile to the road serving it is drawn in
const SERVED_COLOUR: Color = Color::srgb(0.4, 0.95, 0.7);

/// The colour a tile no road reaches is marked in
const UNSERVED_COLOUR: Color = Color::srgb(0.95, 0.4, 0.6);

/// How wide the mark on a tile no road reaches is drawn, as a share of the tile's inradius.
const UNSERVED_MARK: f32 = 0.45;

/// The colour a junction is marked in
const JUNCTION_COLOUR: Color = Color::srgb(0.9, 0.5, 0.95);

/// How far apart two points may stand and still be the same crossing.
///
/// Two roads meeting at a node are reached by every pair of arcs that ends there, so the same
/// point comes back several times over and has to be gathered into one junction rather than one
/// each. It is also what says a point is on an arc at all rather than beside it.
const CROSSING_TOLERANCE: f32 = 1e-3;

/// The roads on the map, the lanes a rover drives on them, and where a building meets them.
///
/// A road carries a lane in each direction unless it was built one-way, and the pair is built,
/// joined at each end and removed together, so a dead-end spur is drivable. Nothing overtakes
/// anywhere in the network: there is no lane to move into, so a slow rover is everyone's problem
/// and one badly placed building is a queue you can watch form. One lane shared both ways was
/// cheaper and made traffic a decoration; several each way bought overtaking and spent it
/// softening the jams the game is for; making the player draw the return leg charged the saving
/// to the first thing they build.
pub struct RoadPlugin;

/// The point in a frame by which the roads spawned have their lanes, segments and junctions.
///
/// A road is a record of nodes when it is spawned and is laid on the frame after, so what runs
/// before this cannot tell a stretch of road that has gone from one that has not been laid yet.
/// Anything holding a place on a road the player edited has to wait for it.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RoadsLaid;

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
    /// Whether the road carries only the lane running the way it was placed.
    ///
    /// A one-way road has no lane back and no join at either end, so its far end is a dead end
    /// rather than a place to turn round, and nothing routes onto it against its direction.
    pub one_way: bool,
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

/// A place on the network, which is a distance along one of the arcs a road was laid as.
///
/// It outlives the segment covering it, and that is what it is for: a road taken apart by a
/// removal is laid again as the stretches either side of the arc that went, and those derive the
/// arcs they already had rather than a curve refitted through the same nodes (invariant 3). The
/// ground under a place is therefore either back to the bit or gone, so whatever was standing on
/// it is put back by asking which segment covers it rather than by measuring anything
/// (invariant 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceOnTheRoad {
    arc: Arc,
    along: f32,
}

/// The segment a rover leaving this one drives onto next.
#[derive(Component)]
#[relationship(relationship_target = PreviousSegments)]
pub struct NextSegment(pub Entity);

/// The segments that lead onto this one, which is more than one where lanes meet.
#[derive(Component)]
#[relationship_target(relationship = NextSegment)]
pub struct PreviousSegments(Vec<Entity>);

/// A segment cut in two by a junction, and the stretch of it beyond the cut.
///
/// The arc under both halves is the one that was already there and so is every distance along it:
/// what the cut moved is where one segment stops answering for the road and the next starts. What
/// was standing further along than the cut is standing on the stretch beyond it and has not moved
/// (invariant 6), which is what leaves anyone holding a distance somewhere to be handed to.
#[derive(EntityEvent)]
pub struct SegmentCut {
    /// The segment that was cut, which now ends where the junction stands.
    #[event_target]
    pub segment: Entity,
    /// The stretch beyond the cut, a segment of its own on the same arc.
    pub beyond: Entity,
}

/// The stretch of a road lying inside a junction, which the junction gives rather than the lane.
///
/// It is the arc the road already had over the ground the junction covers: cutting a leg back
/// moves where one segment stops answering for the road and nothing else, so a road crossed
/// however often stands where it was drawn (invariant 6). A rover reaches it by being let through.
#[derive(Component)]
struct Inside(Entity);

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

/// One arm of a junction: the segments that reach it down that arm and the ones that set off.
///
/// A road crossed in its middle has an arm either side of the crossing and a road that ends on
/// one has a single arm, so three roads meeting and a road ending on another are the same
/// structure counted differently rather than two kinds of junction.
#[derive(Clone, Debug)]
struct JunctionLeg {
    road: Entity,
    heading: Vec3,
    arriving: Vec<Entity>,
    leaving: Vec<Entity>,
    ways: Vec<(usize, Entity)>,
}

/// The arms of a junction, and which of them a rover arriving on one may leave by.
///
/// Derived from where the segments of the crossing roads begin and end rather than stored when
/// the crossing was found, because cutting a road again moves which segment reaches a junction
/// already on it. It is rebuilt whenever any junction changes, so no leg outlives its segments.
#[derive(Component, Default)]
pub struct JunctionLegs(Vec<JunctionLeg>);

/// The junction a segment reaches at its end, and which of that junction's legs it arrives on.
///
/// A rover finds the junction ahead of it by the segment it is driving rather than by looking
/// through every junction on the map, which is what keeps the tick's cost off the fleet.
#[derive(Component, Clone, Copy)]
pub struct EndsAtJunction {
    /// The junction the segment ends at.
    pub junction: Entity,
    /// Which of its legs the segment arrives on.
    pub leg: usize,
}

/// How a junction chooses which of the rovers waiting on its legs goes through next.
///
/// The junction holds the answer rather than the handover deciding it, so a signal or a
/// roundabout is another policy at the same point rather than another kind of junction. Which
/// leg it names is decided by the tick, never by the order the world stores its rovers in.
#[derive(Component, Clone, Debug, PartialEq)]
pub enum JunctionPolicy {
    /// Every leg in turn, the tick saying which of them is asked first.
    TakeTurns,
    /// Traffic on one road goes first, and every other leg gives way to it.
    GiveWayTo(Entity),
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

/// Where whatever holds it meets the road network: one named node of the lattice.
///
/// A road serves it when one of the road's nodes stands on that one: integer equality rather than
/// a distance, so what serves it is a fact of the grid and answers the same however the arcs curve
/// over it and the same after it is cut (invariant 3). What it holds is a segment and a place
/// along it and nothing else — a rover has arrived when it reaches that place, and nothing docks.
/// An endpoint no road reaches carries none, which is what something offering it a delivery has to
/// ask before it makes one: a place off the network is useless rather than illegal.
#[derive(Component)]
pub struct RoadEndpoint {
    at: LatticeNode,
    served: Option<ServedBy>,
}

/// The segment serving an endpoint, and how far along it a rover stops.
#[derive(Clone, Copy, Debug)]
pub struct ServedBy {
    /// The segment a rover arrives on.
    pub segment: Entity,
    /// How far along that segment's arc the endpoint stands, which is where a rover stops.
    pub along: f32,
}

/// The network as a graph, and the quickest way through it.
///
/// A segment leads to the segments the junction at its end permits rather than to every segment
/// touching it, so a one-way road and a junction that refuses a turn take edges out of the graph
/// rather than being checked around it. What a stretch costs is its length over its speed limit
/// and a turn costs nothing of its own, which is what leaves the shortest way through and the
/// quickest way through two different answers.
#[derive(SystemParam)]
pub struct RoadNetwork<'w, 's> {
    segments: Query<
        'w,
        's,
        (
            &'static RoadSegment,
            Option<&'static NextSegment>,
            Option<&'static EndsAtJunction>,
        ),
    >,
    junctions: Query<'w, 's, &'static JunctionLegs>,
    endpoints: Query<'w, 's, &'static RoadEndpoint>,
    frontier: Local<'s, BinaryHeap<Reverse<Reached>>>,
    walked: Local<'s, HashMap<Entity, Step>>,
}

/// A segment the search has reached, ordered by what it costs to reach and then by when it was.
///
/// The order two segments of equal cost come out in is the order they were found in, and they
/// were found in the order the junction offers its ways out, which is the straightest turn first.
/// Nothing here is decided by the order the world stores its entities in (invariant 2).
struct Reached {
    cost: f32,
    found: usize,
    segment: Entity,
}

/// How the search reached a segment, and what it cost to get there.
///
/// A segment reached straight off the rover's own came from nowhere, which is where walking the
/// ways out back from the destination stops.
struct Step {
    cost: f32,
    came_from: Option<Entity>,
    through_a_junction: bool,
    expanded: bool,
}

#[derive(SystemParam)]
struct RoadInitializeParams<'w, 's> {
    commands: Commands<'w, 's>,
    occupied: ResMut<'w, RoadTiles>,
}

impl RoadNetwork<'_, '_> {
    /// The ways out a rover standing `along` `from` takes to reach whatever serves `to` soonest.
    ///
    /// The ways out alone, because the lane decides the rest: what comes back is what the rover
    /// has to choose at each junction and nothing else. A rover already standing on the stretch
    /// serving the destination and short of where it stops has nothing to choose at all. Where no
    /// drivable way exists nothing comes back, rather than the part of one that does.
    pub fn fastest_way(&mut self, from: Entity, along: f32, to: Entity) -> Option<Vec<Entity>> {
        let served = self
            .endpoints
            .get(to)
            .ok()
            .and_then(RoadEndpoint::served_by)?;
        let (setting_off, ..) = self.segments.get(from).ok()?;
        if from == served.segment && along <= served.along {
            return Some(Vec::new());
        }

        self.walked.clear();
        self.frontier.clear();
        let mut found = 0;
        let left_of_it = (setting_off.ends_at() - along).max(0.);
        let set_off = left_of_it / setting_off.speed_limit();
        self.open(from, None, set_off, &mut found);

        while let Some(Reverse(reached)) = self.frontier.pop() {
            let Some(step) = self.walked.get_mut(&reached.segment) else {
                continue;
            };
            if step.expanded || step.cost < reached.cost {
                continue;
            }
            step.expanded = true;
            if reached.segment == served.segment {
                return Some(self.ways_out_to(reached.segment));
            }
            self.open(
                reached.segment,
                Some(reached.segment),
                reached.cost,
                &mut found,
            );
        }
        None
    }

    /// Offer every segment a rover leaving `leaving` may drive onto a place in the search.
    ///
    /// Which those are is the junction's answer where there is one and the lane's where there is
    /// not, so a turn a junction refuses and a lane that runs one way are edges the graph does not
    /// have rather than edges walked and then rejected.
    fn open(&mut self, leaving: Entity, came_from: Option<Entity>, spent: f32, found: &mut usize) {
        let Ok((_, next, junction)) = self.segments.get(leaving) else {
            return;
        };
        let onward = next.map(|next| next.0);
        let Some(junction) = junction.copied() else {
            if let Some(onward) = onward {
                self.offer(onward, came_from, spent, false, found);
            }
            return;
        };

        let ways_out = self
            .junctions
            .get(junction.junction)
            .map(|legs| legs.exits_from(junction.leg))
            .unwrap_or_default();
        for onward in ways_out {
            self.offer(onward, came_from, spent, true, found);
        }
    }

    /// Offer `onward` a place in the search, unless it has already been reached for as little.
    fn offer(
        &mut self,
        onward: Entity,
        came_from: Option<Entity>,
        spent: f32,
        through_a_junction: bool,
        found: &mut usize,
    ) {
        let Ok((segment, ..)) = self.segments.get(onward) else {
            return;
        };
        let cost = spent + segment.length() / segment.speed_limit();
        if self
            .walked
            .get(&onward)
            .is_some_and(|step| step.cost <= cost)
        {
            return;
        }

        self.walked.insert(
            onward,
            Step {
                cost,
                came_from,
                through_a_junction,
                expanded: false,
            },
        );
        self.frontier.push(Reverse(Reached {
            cost,
            found: *found,
            segment: onward,
        }));
        *found += 1;
    }

    /// The ways out of junctions along the way the search walked to `arrived`, in driving order.
    fn ways_out_to(&self, arrived: Entity) -> Vec<Entity> {
        let mut ways_out = Vec::new();
        let mut at = arrived;
        while let Some(step) = self.walked.get(&at) {
            if step.through_a_junction {
                ways_out.push(at);
            }
            match step.came_from {
                Some(came_from) => at = came_from,
                None => break,
            }
        }
        ways_out.reverse();
        ways_out
    }
}

impl PartialEq for Reached {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Reached {}

impl PartialOrd for Reached {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Reached {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cost
            .total_cmp(&other.cost)
            .then(self.found.cmp(&other.found))
    }
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
                    action: "Finish the road, or take off the arc under the cursor",
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
                    fit_the_turns_through_the_junctions,
                    join_the_legs_at_the_junctions,
                )
                    .chain()
                    .in_set(RoadsLaid),
            )
            .add_systems(
                Update,
                (
                    (place_a_node, remove_the_arc_under_the_cursor, lay_the_road).chain(),
                    (connect_the_endpoints, draw_the_endpoints).chain(),
                    draw_the_lanes,
                    draw_the_junctions,
                    draw_the_road_being_placed,
                    draw_the_arc_the_cursor_would_remove,
                    draw_the_occupied_tiles,
                    draw_the_taken_tile_under_the_cursor,
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

    /// How far along this arc the place nearest `point` stands.
    ///
    /// A point beside the curve, off the end of it or nowhere near it at all answers the place on
    /// the arc closest to it, which is what a click on the map has to be measured against.
    fn nearest_to(&self, point: Vec3) -> f32 {
        let at = if self.curvature == 0. {
            (point - self.start).dot(self.tangent)
        } else {
            let centre = self.centre();
            let (from, to) = (self.start - centre, point - centre);
            driven(turn_of(from, to).atan2(from.dot(to)), self.curvature) / self.curvature
        };
        at.clamp(0., self.length)
    }

    /// How far along this arc `point` stands, or nothing where it is off the curve or past an end.
    ///
    /// A point beside the curve is not on this arc, and one on the circle the arc lies on but past
    /// either end is not on it either, so both answer nothing rather than the nearest place.
    fn distance_along(&self, point: Vec3) -> Option<f32> {
        let at = self.nearest_to(point);
        (self.position(at).distance(point) <= CROSSING_TOLERANCE).then_some(at)
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
    /// Where on the ground a rover standing `along` this segment's arc stands.
    ///
    /// The distance is the arc's own rather than a fraction of the stretch, so cutting the stretch
    /// shorter leaves every distance on it reading the same point off the same arc. Beyond either
    /// end the stretch is another segment's to answer for, so the answer stops at the end.
    pub fn world_position(&self, along: f32) -> Vec3 {
        self.arc.position(along.clamp(self.from, self.to))
    }

    /// How far along its arc the place on this segment's stretch nearest `at` stands.
    fn place_of(&self, at: Vec3) -> f32 {
        self.arc.nearest_to(at).clamp(self.from, self.to)
    }

    /// How far along its arc this segment's stretch begins.
    pub fn starts_at(&self) -> f32 {
        self.from
    }

    /// How far along its arc this segment's stretch ends.
    pub fn ends_at(&self) -> f32 {
        self.to
    }

    /// The place on the network a rover standing `along` this segment's arc is holding.
    pub fn place_at(&self, along: f32) -> PlaceOnTheRoad {
        PlaceOnTheRoad {
            arc: self.arc,
            along,
        }
    }

    /// Whether this segment is the stretch of road covering `place`.
    pub fn covers(&self, place: &PlaceOnTheRoad) -> bool {
        self.arc == place.arc && (self.from..=self.to).contains(&place.along)
    }

    /// Which way a rover standing `along` this segment's arc is pointing.
    fn heading_at(&self, along: f32) -> Vec3 {
        self.arc.tangent_at(along.clamp(self.from, self.to))
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

impl PlaceOnTheRoad {
    /// How far along its arc this place stands, which is the distance a rover holding it has run.
    pub fn along(&self) -> f32 {
        self.along
    }
}

impl JunctionLegs {
    /// The ways through the junction open to a rover arriving on `leg`, straightest way first.
    ///
    /// Every leg but the one it arrived on: a junction is not a place to turn round, and a road
    /// that dead-ends elsewhere is where that is done. What each hands back is the arc the rover
    /// drives to make the turn, so a turn is a stretch of road priced by its curvature like any
    /// other. The order is how far the rover has to turn to take each, so carrying on down the
    /// road it is already on comes first wherever it can.
    pub fn exits_from(&self, leg: usize) -> Vec<Entity> {
        let Some(arrived) = self.0.get(leg) else {
            return Vec::new();
        };

        let mut out: Vec<(usize, Entity)> = arrived
            .ways
            .iter()
            .copied()
            .filter(|&(other, _)| other != leg && other < self.0.len())
            .collect();
        out.sort_by(|&(one, _), &(other, _)| {
            straightness(arrived.heading, self.0[other].heading)
                .total_cmp(&straightness(arrived.heading, self.0[one].heading))
        });
        out.into_iter().map(|(_, way)| way).collect()
    }

    fn road_of(&self, leg: usize) -> Option<Entity> {
        self.0.get(leg).map(|leg| leg.road)
    }
}

/// How little a rover arriving down the arm heading `arrived` turns to leave down the one at `out`.
fn straightness(arrived: Vec3, out: Vec3) -> f32 {
    -arrived.dot(out)
}

impl JunctionPolicy {
    /// Which of the legs a rover is waiting on the junction lets through on `tick`.
    ///
    /// The rotation the tick names is what keeps two rovers arriving at once from being served in
    /// whatever order the world holds them, and it is what stops a leg being passed over forever
    /// once the legs before it are empty.
    pub fn who_goes_next(
        &self,
        legs: &JunctionLegs,
        waiting: &[usize],
        tick: u64,
    ) -> Option<usize> {
        let count = legs.0.len();
        if count == 0 {
            return None;
        }

        let favoured = match self {
            Self::TakeTurns => None,
            Self::GiveWayTo(road) => Some(*road)
                .filter(|road| waiting.iter().any(|&leg| legs.road_of(leg) == Some(*road))),
        };
        let asked = (tick % count as u64) as usize;

        waiting
            .iter()
            .copied()
            .filter(|&leg| favoured.is_none_or(|road| legs.road_of(leg) == Some(road)))
            .min_by_key(|&leg| (leg + count - asked) % count)
    }
}

impl RoadEndpoint {
    /// An endpoint standing on `node`, which nothing serves until a road reaches that node.
    pub fn at(node: LatticeNode) -> Self {
        Self {
            at: node,
            served: None,
        }
    }

    /// The node it stands on, which is the one place on the lattice a road can serve it from.
    pub fn standing_on(&self) -> LatticeNode {
        self.at
    }

    /// The segment serving it and where along it, or nothing while no road reaches its node.
    pub fn served_by(&self) -> Option<ServedBy> {
        self.served
    }
}

impl Initialize<RoadInitializeParams<'_, '_>> for Road {
    fn initialize(&mut self, entity: &Entity, params: &mut RoadInitializeParams) -> Result {
        let along = arcs_through(&self.nodes, self.leaving);
        if along.is_empty() {
            return Err("a road of no arcs".into());
        }
        params.occupied.claim(*entity, tiles_walked_by(&along));
        let forth = spawn_lane(&mut params.commands, *entity, &along)?;
        if self.one_way {
            return Ok(());
        }

        let back: Vec<Arc> = along.iter().rev().map(Arc::reversed).collect();
        let back = spawn_lane(&mut params.commands, *entity, &back)?;

        params
            .commands
            .entity(forth.last)
            .insert(NextSegment(back.first));
        params
            .commands
            .entity(back.last)
            .insert(NextSegment(forth.first));
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
/// road arrived on, and a target that arc cannot turn tightly enough to reach is refused, as is
/// one whose arc would run over ground a building stands on.
fn place_a_node(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    buildings: Res<BuildingTiles>,
    roads: Query<&Road>,
    junctions: Query<&Junction>,
    mut placing: Query<&mut DrawnRoad>,
) {
    if !player_input.tap || *action.get() != PlayerAction::EditRoads {
        return;
    }
    let Some(target) = player_input.cursor_node else {
        return;
    };

    let Some(mut placing) = placing.iter_mut().next() else {
        if stands_on_a_building(target.world_position(), &buildings) {
            return;
        }
        commands.spawn(DrawnRoad {
            nodes: vec![target],
            leaving: direction_leaving(target, &roads),
        });
        return;
    };

    let (laid, crossings) = the_network_standing(&roads, &junctions);
    if placing.nodes.last() == Some(&target)
        || proposed_arc(&placing, target, &buildings, &laid, &crossings).is_none()
    {
        return;
    }
    placing.nodes.push(target);
}

/// The arcs already laid and the crossings already on them, which a click is measured against.
fn the_network_standing(
    roads: &Query<&Road>,
    junctions: &Query<&Junction>,
) -> (Vec<Arc>, Vec<Vec3>) {
    (
        roads
            .iter()
            .flat_map(|road| arcs_through(&road.nodes, road.leaving))
            .collect(),
        junctions.iter().map(|junction| junction.at).collect(),
    )
}

/// Whether every crossing `arc` would make leaves room at it for the turns through it.
///
/// A junction reaches a pull-back down each of its arms, so two crossings closer than its whole
/// extent have no room between them for either turn. Refusing the second is what leaves the
/// junction the player already built exactly where they built it. A crossing landing on one
/// already there is that crossing rather than a second one, and is what a third road meeting two
/// others at a point makes.
fn leaves_room_for_its_turns(arc: &Arc, laid: &[Arc], crossings: &[Vec3]) -> bool {
    let mut met: Vec<Vec3> = crossings.to_vec();
    for other in laid {
        for (at, ..) in crossings_of(arc, other) {
            if !room_for_a_junction_at(at, &met) {
                return false;
            }
            met.push(at);
        }
    }
    true
}

/// Whether a crossing at `at` stands clear enough of those already `met` for both to turn.
fn room_for_a_junction_at(at: Vec3, met: &[Vec3]) -> bool {
    !met.iter().any(|met| {
        let apart = met.distance(at);
        apart > CROSSING_TOLERANCE && apart < JUNCTION_EXTENT
    })
}

/// The arc a click on `target` would lay, or nothing where the road tool refuses it.
///
/// It refuses a target no arc can turn tightly enough to reach, one whose arc would run over a
/// tile a building stands on, and one that would cross a road too near a crossing already there
/// for either of them to have room to turn. Every answer comes from here, so the tool says no in
/// one voice and draws the refusal the same way whichever of them it is.
fn proposed_arc(
    placing: &DrawnRoad,
    target: LatticeNode,
    buildings: &BuildingTiles,
    laid: &[Arc],
    crossings: &[Vec3],
) -> Option<Arc> {
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
    (arc.curvature.abs() * MIN_TURN_RADIUS <= 1.
        && nothing_stands_under(&arc, buildings)
        && leaves_room_for_its_turns(&arc, laid, crossings))
    .then_some(arc)
}

/// Whether the tiles `arc` would take are clear of buildings.
///
/// The arc is walked the way the tiles it claims are walked once it is laid, so what the road tool
/// refuses and what the road would occupy are the same tiles rather than two measurements of it.
/// It is asked of an arc that does not exist yet, which is why it walks one rather than reading a
/// road's claim back.
fn nothing_stands_under(arc: &Arc, buildings: &BuildingTiles) -> bool {
    !walk_of(arc).any(|at| stands_on_a_building(arc.position(at), buildings))
}

/// Whether a building stands on the tile `position` falls on.
fn stands_on_a_building(position: Vec3, buildings: &BuildingTiles) -> bool {
    buildings
        .building_on(HexCoordinates::from_world_position(position))
        .is_some()
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
            one_way: false,
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

/// Take the arc under the cursor off the map when the player clicks the secondary button.
///
/// A road is its nodes, so an arc between two of them is what a removal takes, and what is left is
/// the same road either side of it: derived from the same integers and the same direction it set
/// off in, so nothing that survived has moved (invariant 6). The same click finishes a road being
/// placed, which is what it is for while there is one, so nothing is taken off until there is not.
fn remove_the_arc_under_the_cursor(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    occupied: Res<RoadTiles>,
    placing: Query<&DrawnRoad>,
    roads: Query<&Road>,
) {
    if !player_input.secondary_tap
        || *action.get() != PlayerAction::EditRoads
        || !placing.is_empty()
    {
        return;
    }

    let Some(at) = player_input.ground_cursor_position else {
        return;
    };
    let Some((entity, taken, gone)) = the_arc_under(at, &occupied, &roads) else {
        return;
    };
    let Ok(road) = roads.get(entity) else {
        return;
    };

    commands.entity(entity).insert(Destroy);
    for left in roads_left_by(road, taken, &gone) {
        commands.spawn(left);
    }
}

/// The road nearest `at`, which of its arcs is nearest, and that arc.
///
/// Only the roads over the tile `at` stands on are measured, by the key `RoadTiles` already holds,
/// so a click is answered without walking a map of thousands of roads.
fn the_arc_under(
    at: Vec3,
    occupied: &RoadTiles,
    roads: &Query<&Road>,
) -> Option<(Entity, usize, Arc)> {
    occupied
        .roads_over(HexCoordinates::from_world_position(at))
        .iter()
        .filter_map(|&road| Some((road, roads.get(road).ok()?)))
        .flat_map(|(road, standing)| {
            arcs_through(&standing.nodes, standing.leaving)
                .into_iter()
                .enumerate()
                .map(move |(taken, arc)| (road, taken, arc))
        })
        .min_by(|(.., arc), (.., other)| distance_to(arc, at).total_cmp(&distance_to(other, at)))
}

/// How far `point` stands from the nearest place on `arc`.
fn distance_to(arc: &Arc, point: Vec3) -> f32 {
    arc.position(arc.nearest_to(point)).distance_squared(point)
}

/// The roads left when the arc `taken` of `road`, which is `gone`, is taken off it.
///
/// A road of one arc leaves nothing behind, and one that loses an arc at either end leaves the
/// rest of itself whole rather than a road and a node. The far half sets off along the tangent the
/// removed arc ended on, which is the one the arc after it was built from, so each half derives
/// the arcs it already had rather than a refitted curve through the same nodes.
fn roads_left_by(road: &Road, taken: usize, gone: &Arc) -> Vec<Road> {
    let mut left = Vec::new();
    if taken > 0 {
        left.push(Road {
            nodes: road.nodes[..=taken].to_vec(),
            leaving: road.leaving,
            one_way: road.one_way,
        });
    }
    if taken + 2 < road.nodes.len() {
        left.push(Road {
            nodes: road.nodes[taken + 1..].to_vec(),
            leaving: Some(gone.tangent_at(gone.length)),
            one_way: road.one_way,
        });
    }
    left
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
    before: Query<&PreviousSegments>,
    mut junctions: Query<(Entity, &mut Junction)>,
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

    let mut made: Vec<Vec3> = junctions.iter().map(|(_, junction)| junction.at).collect();
    for (at, across) in found {
        if !room_for_a_junction_at(at, &made) {
            continue;
        }
        made.push(at);
        let standing = roads_crossing(&across);
        let standing_at = junctions
            .iter()
            .find(|(_, junction)| junction.at.distance(at) <= CROSSING_TOLERANCE)
            .map(|(entity, _)| entity);
        let junction = standing_at.unwrap_or_else(|| commands.spawn_empty().id());
        for &road in &standing {
            cut_the_road_back_from(
                road,
                at,
                junction,
                &children,
                &mut segments,
                &before,
                &mut commands,
            );
        }
        match standing_at.and_then(|entity| junctions.get_mut(entity).ok()) {
            Some((entity, mut standing)) => {
                for crossing in across {
                    note(&mut standing.across, crossing);
                }
                commands.entity(entity).remove::<JunctionLegs>();
            }
            None => {
                let policy = gives_way_at(&standing, &laid);
                commands
                    .entity(junction)
                    .insert((Junction { at, across }, policy));
            }
        }
    }
}

/// Who has right of way where the roads in `standing` cross, before the player has said.
///
/// A road drawn across one that was already there gives way to it, which makes the network the
/// player has built the one that keeps running and the road they are adding the one that waits.
/// Two roads laid on the same frame have no such order between them, so they take turns.
fn gives_way_at(standing: &[Entity], laid: &[(Entity, Vec<Arc>, bool)]) -> JunctionPolicy {
    let mut older = standing.iter().filter(|road| {
        laid.iter()
            .any(|(entity, _, fresh)| entity == *road && !fresh)
    });
    match (older.next(), older.next()) {
        (Some(&road), None) => JunctionPolicy::GiveWayTo(road),
        _ => JunctionPolicy::TakeTurns,
    }
}

/// Work out the arms of every junction, and tell each segment which one it runs into.
///
/// Rebuilt for every junction rather than for the ones just found, because cutting a road again
/// splits the segment that used to reach a junction further along it and hands the far half to a
/// new entity. Nothing here derives a curve: a leg is the segments whose ends already stand at
/// the crossing, gathered by the way they point (invariant 3).
fn join_the_legs_at_the_junctions(
    mut commands: Commands,
    mut removed: RemovedComponents<Junction>,
    changed: Query<(), Changed<Junction>>,
    junctions: Query<(Entity, &Junction)>,
    joined: Query<Entity, With<EndsAtJunction>>,
    children: Query<&Children>,
    segments: Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) {
    let gone = removed.read().count() > 0;
    if !gone && changed.is_empty() {
        return;
    }

    for segment in &joined {
        commands.entity(segment).remove::<EndsAtJunction>();
    }

    for (entity, junction) in &junctions {
        let mut legs = legs_of(entity, junction, &children, &segments);
        join_the_ways_through(entity, &mut legs, &children, &segments);
        for (index, leg) in legs.iter().enumerate() {
            for &arriving in &leg.arriving {
                commands.entity(arriving).insert(EndsAtJunction {
                    junction: entity,
                    leg: index,
                });
            }
        }
        commands.entity(entity).insert(JunctionLegs(legs));
    }
}

/// Fit an arc between every pair of legs a junction's own roads do not already join.
///
/// A rover carrying straight on drives the stretch of its road inside the junction, which is that
/// road's arc and priced by that road's curvature, so it pays nothing to cross a straight. Every
/// other turn is an arc of its own, fitted between the legs it joins and priced the same way, so
/// the sharpest corner in a network is charged for by the rule already there rather than a term
/// added beside it.
fn fit_the_turns_through_the_junctions(
    mut commands: Commands,
    junctions: Query<(Entity, &Junction), Changed<Junction>>,
    children: Query<&Children>,
    segments: Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) {
    for (entity, junction) in &junctions {
        for turn in children.get(entity).into_iter().flat_map(Children::iter) {
            commands.entity(turn).despawn();
        }

        let legs = legs_of(entity, junction, &children, &segments);
        for (index, leg) in legs.iter().enumerate() {
            let carried_on: Vec<usize> = leg
                .arriving
                .iter()
                .filter_map(|&arriving| segments.get(arriving).ok())
                .filter_map(|(_, onward, _)| onward.map(|onward| onward.0))
                .filter_map(|head| the_leg_reached_by(head, &legs, &segments))
                .collect();

            for (other, out) in legs.iter().enumerate() {
                if other == index || carried_on.contains(&other) {
                    continue;
                }
                lay_the_turn_between(entity, leg, out, &segments, &mut commands);
            }
        }
    }
}

/// Lay the arc a rover arriving on `from` drives to leave a junction by `out`.
fn lay_the_turn_between(
    junction: Entity,
    from: &JunctionLeg,
    out: &JunctionLeg,
    segments: &Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
    commands: &mut Commands,
) {
    let sets_off = from
        .arriving
        .first()
        .and_then(|&arriving| segments.get(arriving).ok())
        .map(|(segment, _, _)| {
            (
                segment.world_position(segment.ends_at()),
                segment.heading_at(segment.ends_at()),
            )
        });
    let reaches = out
        .leaving
        .first()
        .and_then(|&leaving| segments.get(leaving).ok().map(|found| (leaving, found)))
        .map(|(leaving, (segment, _, _))| {
            (
                leaving,
                segment.world_position(segment.starts_at()),
                segment.heading_at(segment.starts_at()),
            )
        });
    let (Some((sets_off, heading)), Some((leaving, reaches, along))) = (sets_off, reaches) else {
        return;
    };

    let mut laid: Option<Entity> = None;
    for arc in turn_between(sets_off, heading, reaches, along) {
        let piece = commands
            .spawn((
                RoadSegment {
                    arc,
                    from: 0.,
                    to: arc.length,
                },
                ChildOf(junction),
            ))
            .id();
        if let Some(laid) = laid {
            commands.entity(laid).insert(NextSegment(piece));
        }
        laid = Some(piece);
    }
    if let Some(laid) = laid {
        commands.entity(laid).insert(NextSegment(leaving));
    }
}

/// The arcs a rover drives to turn from `heading` at `from` onto `along` at `to`.
///
/// One where the arc leaving along the leg arrived on also reaches the far leg along it, which is
/// what equal pull-backs either side of a crossing of straights give. Two where a leg curves
/// through the junction: a pair joined at a shared tangent meets both ends exactly, where a single
/// arc has a curvature and a length to spend and three ends' worth of geometry to spend them on.
fn turn_between(from: Vec3, heading: Vec3, to: Vec3, along: Vec3) -> Vec<Arc> {
    let one = Arc::through(from, heading, to);
    if one.tangent_at(one.length).dot(along) >= 1. - TURN_TANGENT_REACH {
        return vec![one];
    }

    let reach = to - from;
    let opening = 2. * (1. - heading.dot(along));
    let leaning = 2. * reach.dot(heading + along);
    let span = reach.length_squared();
    let chord = if opening.abs() < STRAIGHT_REACH {
        if leaning.abs() < STRAIGHT_REACH {
            return vec![one];
        }
        span / leaning
    } else {
        (-leaning + (leaning * leaning + 4. * opening * span).sqrt()) / (2. * opening)
    };

    let joint = ((from + heading * chord) + (to - along * chord)) / 2.;
    let first = Arc::through(from, heading, joint);
    let second = Arc::through(joint, first.tangent_at(first.length), to);
    vec![first, second]
}

/// Tell each leg which way through the junction reaches each of the others.
///
/// A way is the stretch of road inside the junction where it carries the rover straight on and the
/// arc fitted between two legs where it turns, so both are found the same way: follow what leaves
/// the leg until it runs into a segment another leg is left by.
fn join_the_ways_through(
    entity: Entity,
    legs: &mut [JunctionLeg],
    children: &Query<&Children>,
    segments: &Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) {
    let mut ways: Vec<Vec<(usize, Entity)>> = vec![Vec::new(); legs.len()];
    for (index, leg) in legs.iter().enumerate() {
        for &arriving in &leg.arriving {
            let Some(head) = segments
                .get(arriving)
                .ok()
                .and_then(|(_, onward, _)| onward.map(|onward| onward.0))
            else {
                continue;
            };
            if let Some(out) = the_leg_reached_by(head, legs, segments) {
                ways[index].push((out, head));
            }
        }
    }

    for turn in children.get(entity).into_iter().flat_map(Children::iter) {
        let Ok((segment, _, _)) = segments.get(turn) else {
            continue;
        };
        let sets_off = segment.world_position(segment.starts_at());
        let Some(index) = the_leg_setting_off_at(sets_off, legs, segments) else {
            continue;
        };
        if let Some(out) = the_leg_reached_by(turn, legs, segments) {
            ways[index].push((out, turn));
        }
    }

    for (leg, ways) in legs.iter_mut().zip(ways) {
        leg.ways = ways;
    }
}

/// Which leg the way through a junction beginning at `head` comes out on.
fn the_leg_reached_by(
    head: Entity,
    legs: &[JunctionLeg],
    segments: &Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) -> Option<usize> {
    let mut standing = head;
    for _ in 0..STEPS_THROUGH_A_JUNCTION {
        let (_, onward, _) = segments.get(standing).ok()?;
        let onward = onward?.0;
        if let Some(leg) = legs.iter().position(|leg| leg.leaving.contains(&onward)) {
            return Some(leg);
        }
        standing = onward;
    }
    None
}

/// Which leg a way through a junction that begins at `sets_off` is entered from.
fn the_leg_setting_off_at(
    sets_off: Vec3,
    legs: &[JunctionLeg],
    segments: &Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) -> Option<usize> {
    legs.iter().position(|leg| {
        leg.arriving.iter().any(|&arriving| {
            segments.get(arriving).is_ok_and(|(segment, _, _)| {
                segment.world_position(segment.ends_at()).distance(sets_off) <= CROSSING_TOLERANCE
            })
        })
    })
}

/// The arms of `junction`, as the segments of its roads that reach the stretch it holds.
///
/// A leg begins where the road stops being the lane's to drive, which is a pull-back short of the
/// crossing rather than at it, so a junction has an extent on every arm. Nothing here derives a
/// curve: the arms are read off the segments already cut (invariant 3).
fn legs_of(
    entity: Entity,
    junction: &Junction,
    children: &Query<&Children>,
    segments: &Query<(&RoadSegment, Option<&NextSegment>, Option<&Inside>)>,
) -> Vec<JunctionLeg> {
    let mut legs: Vec<JunctionLeg> = Vec::new();
    for road in roads_crossing(&junction.across) {
        let pieces = pieces_of(road, children);
        let held = |piece: Entity| {
            segments
                .get(piece)
                .is_ok_and(|(_, _, inside)| inside.is_some_and(|inside| inside.0 == entity))
        };
        let left_by: Vec<Entity> = pieces
            .iter()
            .filter(|&&piece| held(piece))
            .filter_map(|&piece| segments.get(piece).ok())
            .filter_map(|(_, onward, _)| onward.map(|onward| onward.0))
            .filter(|&onward| !held(onward))
            .collect();

        for &piece in &pieces {
            let Ok((segment, onward, _)) = segments.get(piece) else {
                continue;
            };
            if held(piece) {
                continue;
            }
            if onward.map(|onward| onward.0).is_some_and(held) {
                arm_of(&mut legs, road, -segment.heading_at(segment.ends_at()))
                    .arriving
                    .push(piece);
            }
            if left_by.contains(&piece) {
                arm_of(&mut legs, road, segment.heading_at(segment.starts_at()))
                    .leaving
                    .push(piece);
            }
        }
    }
    legs
}

/// Every segment of `road`, on either of its lanes.
fn pieces_of(road: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let Ok(lanes) = children.get(road) else {
        return Vec::new();
    };
    lanes
        .iter()
        .filter_map(|lane| children.get(lane).ok())
        .flat_map(|pieces| pieces.iter())
        .collect()
}

/// The arm of `legs` running `heading` out of the junction, opened where there is not one yet.
fn arm_of(legs: &mut Vec<JunctionLeg>, road: Entity, heading: Vec3) -> &mut JunctionLeg {
    let found = legs
        .iter()
        .position(|leg| leg.road == road && leg.heading.dot(heading) >= LEG_TOLERANCE);
    let at = found.unwrap_or_else(|| {
        legs.push(JunctionLeg {
            road,
            heading,
            arriving: Vec::new(),
            leaving: Vec::new(),
            ways: Vec::new(),
        });
        legs.len() - 1
    });
    &mut legs[at]
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

/// Take every lane of `road` back from `at` by a pull-back, giving the stretch between to `junction`.
///
/// Every arc is the one the road already had and so is every distance along it: a cut moves where
/// one segment stops answering for the road and the next starts, and nothing else. A road crossed
/// however often therefore stands exactly where it was drawn (invariant 6).
fn cut_the_road_back_from(
    road: Entity,
    at: Vec3,
    junction: Entity,
    children: &Query<&Children>,
    segments: &mut Query<(&mut RoadSegment, Option<&NextSegment>)>,
    before: &Query<&PreviousSegments>,
    commands: &mut Commands,
) {
    let Ok(lanes) = children.get(road) else {
        return;
    };
    for lane in lanes.iter() {
        let Ok(pieces) = children.get(lane) else {
            continue;
        };
        let held: Vec<Entity> = pieces.iter().collect();
        let Some((piece, along)) = the_piece_covering(at, &held, segments) else {
            continue;
        };
        let (back, enters_at) = walked_back_from(piece, along, &held, segments, before);
        let (on, leaves_at) = walked_on_from(piece, along, &held, segments);
        let mut spanned = back;
        spanned.extend(on.into_iter().skip(1));
        give_the_stretch_to(
            lane, junction, &spanned, enters_at, leaves_at, segments, commands,
        );
    }
}

/// Which of `held` covers `at`, and how far along its arc that stands.
fn the_piece_covering(
    at: Vec3,
    held: &[Entity],
    segments: &Query<(&mut RoadSegment, Option<&NextSegment>)>,
) -> Option<(Entity, f32)> {
    held.iter().find_map(|&piece| {
        let (segment, _) = segments.get(piece).ok()?;
        let along = segment.arc.distance_along(at)?;
        (along >= segment.from - CROSSING_TOLERANCE && along <= segment.to + CROSSING_TOLERANCE)
            .then(|| (piece, along.clamp(segment.from, segment.to)))
    })
}

/// The pieces a pull-back back down the lane from `along` on `piece` covers, and where it stops.
///
/// They come back in the order a rover drives them, so the first is the one the junction is
/// entered on. A lane that runs out first stops there: a road beginning on another is inside the
/// junction from its first node, and has nothing further back to give.
fn walked_back_from(
    piece: Entity,
    along: f32,
    held: &[Entity],
    segments: &Query<(&mut RoadSegment, Option<&NextSegment>)>,
    before: &Query<&PreviousSegments>,
) -> (Vec<Entity>, f32) {
    let mut walked = vec![piece];
    let mut standing = piece;
    let mut reached = along;
    let mut left = JUNCTION_PULLBACK;
    for _ in 0..held.len() {
        let Ok((segment, _)) = segments.get(standing) else {
            break;
        };
        let room = reached - segment.from;
        if room >= left {
            walked.reverse();
            return (walked, reached - left);
        }
        left -= room;
        let earlier = before.get(standing).ok().and_then(|earlier| {
            earlier
                .0
                .iter()
                .copied()
                .find(|piece| held.contains(piece) && !walked.contains(piece))
        });
        let Some(earlier) = earlier else {
            walked.reverse();
            return (walked, segment.from);
        };
        reached = segments.get(earlier).map_or(0., |(segment, _)| segment.to);
        standing = earlier;
        walked.push(earlier);
    }
    walked.reverse();
    (walked, reached)
}

/// The pieces a pull-back on down the lane from `along` on `piece` covers, and where it stops.
fn walked_on_from(
    piece: Entity,
    along: f32,
    held: &[Entity],
    segments: &Query<(&mut RoadSegment, Option<&NextSegment>)>,
) -> (Vec<Entity>, f32) {
    let mut walked = vec![piece];
    let mut standing = piece;
    let mut reached = along;
    let mut left = JUNCTION_PULLBACK;
    for _ in 0..held.len() {
        let Ok((segment, onward)) = segments.get(standing) else {
            break;
        };
        let room = segment.to - reached;
        if room >= left {
            return (walked, reached + left);
        }
        left -= room;
        let onward = onward
            .map(|onward| onward.0)
            .filter(|piece| held.contains(piece) && !walked.contains(piece));
        let Some(onward) = onward else {
            return (walked, segment.to);
        };
        reached = segments.get(onward).map_or(0., |(segment, _)| segment.from);
        standing = onward;
        walked.push(onward);
    }
    (walked, reached)
}

/// Mark the stretch of `spanned` between `enters_at` and `leaves_at` as `junction`'s, cutting the
/// ends off the pieces that reach past it.
fn give_the_stretch_to(
    lane: Entity,
    junction: Entity,
    spanned: &[Entity],
    enters_at: f32,
    leaves_at: f32,
    segments: &mut Query<(&mut RoadSegment, Option<&NextSegment>)>,
    commands: &mut Commands,
) {
    let (Some(&first), Some(&last)) = (spanned.first(), spanned.last()) else {
        return;
    };
    if first == last {
        give_one_piece_to(
            lane, junction, first, enters_at, leaves_at, segments, commands,
        );
        return;
    }

    let mut inside = spanned.to_vec();
    if let Some(beyond) = cut_beyond(lane, first, enters_at, segments, commands) {
        inside[0] = beyond;
    }
    cut_beyond(lane, last, leaves_at, segments, commands);
    for piece in inside {
        commands.entity(piece).insert(Inside(junction));
    }
}

/// Give `junction` the stretch of one piece between `enters_at` and `leaves_at`.
fn give_one_piece_to(
    lane: Entity,
    junction: Entity,
    piece: Entity,
    enters_at: f32,
    leaves_at: f32,
    segments: &mut Query<(&mut RoadSegment, Option<&NextSegment>)>,
    commands: &mut Commands,
) {
    let Ok((segment, onward)) = segments.get(piece) else {
        return;
    };
    let (arc, from, to) = (segment.arc, segment.from, segment.to);
    let onward = onward.map(|onward| onward.0);
    let tail = (leaves_at < to - CROSSING_TOLERANCE).then(|| {
        let tail = commands
            .spawn((
                RoadSegment {
                    arc,
                    from: leaves_at,
                    to,
                },
                ChildOf(lane),
            ))
            .id();
        if let Some(onward) = onward {
            commands.entity(tail).insert(NextSegment(onward));
        }
        tail
    });

    if enters_at > from + CROSSING_TOLERANCE {
        let middle = commands
            .spawn((
                RoadSegment {
                    arc,
                    from: enters_at,
                    to: leaves_at,
                },
                ChildOf(lane),
                Inside(junction),
            ))
            .id();
        if let Some(onward) = tail.or(onward) {
            commands.entity(middle).insert(NextSegment(onward));
        }
        commands.entity(piece).insert(NextSegment(middle));
        commands.trigger(SegmentCut {
            segment: piece,
            beyond: middle,
        });
        if let Some(tail) = tail {
            commands.trigger(SegmentCut {
                segment: middle,
                beyond: tail,
            });
        }
        if let Ok((mut segment, _)) = segments.get_mut(piece) {
            segment.to = enters_at;
        }
        return;
    }

    commands.entity(piece).insert(Inside(junction));
    if let Some(tail) = tail {
        commands.entity(piece).insert(NextSegment(tail));
        commands.trigger(SegmentCut {
            segment: piece,
            beyond: tail,
        });
        if let Ok((mut segment, _)) = segments.get_mut(piece) {
            segment.to = leaves_at;
        }
    }
}

/// Cut `piece` at `along`, handing the stretch past it to a segment of its own on the same arc.
///
/// The arc is copied rather than worked out again, so both halves hold the same curve to the bit
/// and neither has moved. A cut that lands on an end of the piece moves nothing and makes nothing.
fn cut_beyond(
    lane: Entity,
    piece: Entity,
    along: f32,
    segments: &mut Query<(&mut RoadSegment, Option<&NextSegment>)>,
    commands: &mut Commands,
) -> Option<Entity> {
    let Ok((segment, onward)) = segments.get(piece) else {
        return None;
    };
    let (arc, from, to) = (segment.arc, segment.from, segment.to);
    if along <= from + CROSSING_TOLERANCE || along >= to - CROSSING_TOLERANCE {
        return None;
    }
    let onward = onward.map(|onward| onward.0);

    let beyond = commands
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
        commands.entity(beyond).insert(NextSegment(onward));
    }
    commands.entity(piece).insert(NextSegment(beyond));
    commands.trigger(SegmentCut {
        segment: piece,
        beyond,
    });
    if let Ok((mut segment, _)) = segments.get_mut(piece) {
        segment.to = along;
    }
    Some(beyond)
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
    buildings: Res<BuildingTiles>,
    roads: Query<&Road>,
    junctions: Query<&Junction>,
    placing: Query<&DrawnRoad>,
) {
    if placing.is_empty() {
        return;
    }
    let (laid, crossings) = the_network_standing(&roads, &junctions);
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
        match proposed_arc(placed, target, &buildings, &laid, &crossings) {
            Some(arc) => gizmos.linestrip(sampled(&arc), PROPOSAL_COLOUR),
            None => gizmos.line(
                standing + GIZMO_LIFT,
                target.world_position() + GIZMO_LIFT,
                UNREACHABLE_COLOUR,
            ),
        }
    }
}

/// Draw the arc a secondary click would take off the map.
///
/// One arc of a road looks like the one beside it, and which of them the click would take is a
/// measurement from the cursor rather than anything standing on the ground, so a player who cannot
/// see it finds out by losing the wrong one (invariant 5). Nothing is drawn while a road is being
/// placed, because that is what the click does instead.
fn draw_the_arc_the_cursor_would_remove(
    mut gizmos: Gizmos<DebugGizmos>,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    occupied: Res<RoadTiles>,
    placing: Query<&DrawnRoad>,
    roads: Query<&Road>,
) {
    if *action.get() != PlayerAction::EditRoads || !placing.is_empty() {
        return;
    }
    let Some(at) = player_input.ground_cursor_position else {
        return;
    };
    let Some((.., arc)) = the_arc_under(at, &occupied, &roads) else {
        return;
    };
    gizmos.linestrip(sampled(&arc), REMOVING_COLOUR);
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

/// Mark how far every junction reaches, its legs, and which of them the policy lets go first.
///
/// A crossing is a point on both roads rather than anything either of them stores, so a road
/// drawn over another looks exactly like a road drawn beside it until the junction is drawn. The
/// circle is the pull-back the junction takes its legs back by, which is the ground the turns
/// through it are fitted over. Nor does anything say which arms it gathered into one leg, or
/// which of them a rover waits on, so the arrows say both.
fn draw_the_junctions(
    mut gizmos: Gizmos<DebugGizmos>,
    junctions: Query<(&Junction, Option<&JunctionLegs>, Option<&JunctionPolicy>)>,
) {
    for (junction, legs, policy) in &junctions {
        gizmos.circle(
            Isometry3d::new(junction.at + GIZMO_LIFT, Quat::from_rotation_x(FRAC_PI_2)),
            JUNCTION_PULLBACK,
            JUNCTION_COLOUR,
        );

        let Some(legs) = legs else {
            continue;
        };
        for leg in &legs.0 {
            gizmos.arrow(
                junction.at + GIZMO_LIFT,
                junction.at + GIZMO_LIFT + leg.heading * MAP_TILE_INRADIUS * LEG_MARK,
                colour_of(leg, policy),
            );
        }
    }
}

/// The colour a leg is marked in, which says whether the policy makes it give way or go first.
fn colour_of(leg: &JunctionLeg, policy: Option<&JunctionPolicy>) -> Color {
    match policy {
        Some(JunctionPolicy::GiveWayTo(road)) if *road == leg.road => PRIORITY_COLOUR,
        Some(JunctionPolicy::GiveWayTo(_)) => GIVING_WAY_COLOUR,
        _ => JUNCTION_COLOUR,
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

/// Mark the tile under the cursor when something already stands on it.
///
/// A road running over it is the tile question building placement refuses a building with; a
/// building on it is the same question the other way round, and is what the road tool refuses a
/// first click with — which lays no arc and so has no proposal to draw red. Reading either off the
/// things themselves would mean reading every one of them, which is what the tile is keyed for.
fn draw_the_taken_tile_under_the_cursor(
    mut gizmos: Gizmos<DebugGizmos>,
    occupied: Res<RoadTiles>,
    buildings: Res<BuildingTiles>,
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
    if occupied.roads_over(tile).is_empty() && buildings.building_on(tile).is_none() {
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

/// Give every endpoint nothing serves the segment of a road that does.
///
/// One that is already served keeps what it has, so a road laid later does not take a building
/// that is already on the network. One whose segment no longer reaches the place it stops at looks
/// again, which is what a road cut under it leaves behind: the segment that was serving it answers
/// for a shorter stretch than it did, and the place is another's to give.
fn connect_the_endpoints(
    mut endpoints: Query<&mut RoadEndpoint>,
    occupied: Res<RoadTiles>,
    roads: Query<&Road>,
    children: Query<&Children>,
    segments: Query<&RoadSegment>,
) {
    for mut endpoint in &mut endpoints {
        if endpoint.served.is_some_and(|served| {
            segments
                .get(served.segment)
                .is_ok_and(|piece| (piece.starts_at()..=piece.ends_at()).contains(&served.along))
        }) {
            continue;
        }

        let at = endpoint.at;
        endpoint.served = the_road_serving(at, &occupied, &roads, &children, &segments);
    }
}

/// The place on the network serving `at`: the road standing on that node, if one does.
///
/// Only the roads over the three tiles sharing the node are read. A road standing on it runs over
/// one of those three, so nothing outside them can serve it and no road further off is measured.
/// A node that is a tile's own middle is shared by no tiles and is served by nothing, which is
/// the answer a road through the middle of a tile already gave.
fn the_road_serving(
    at: LatticeNode,
    occupied: &RoadTiles,
    roads: &Query<&Road>,
    children: &Query<&Children>,
    segments: &Query<&RoadSegment>,
) -> Option<ServedBy> {
    at.tiles_sharing()?
        .into_iter()
        .flat_map(|near| occupied.roads_over(near))
        .copied()
        .find(|&road| {
            roads
                .get(road)
                .is_ok_and(|standing| standing.nodes.contains(&at))
        })
        .and_then(|road| segment_standing_on(at, road, children, segments))
}

/// The place on `road` standing on `node`, which is where a rover arriving there stops.
///
/// The nearest place on any of its stretches, rather than the nearest end of one: a node is
/// usually where a segment stops and the next starts, and a junction standing on it leaves it in
/// the middle of the stretch the junction holds. Both answer the node itself.
fn segment_standing_on(
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
        .filter_map(|segment| segments.get(segment).ok().map(|piece| (segment, piece)))
        .map(|(segment, piece)| {
            let along = piece.place_of(standing);
            (ServedBy { segment, along }, piece.world_position(along))
        })
        .min_by(|(_, place), (_, other)| {
            place
                .distance_squared(standing)
                .total_cmp(&other.distance_squared(standing))
        })
        .map(|(served, _)| served)
}

/// Draw what serves each endpoint, and mark the ones nothing does.
///
/// Whether a building is on the network is otherwise invisible: it stands on its tile looking the
/// same either way, and a road running past it looks like a road serving it (invariant 5).
fn draw_the_endpoints(
    mut gizmos: Gizmos<DebugGizmos>,
    endpoints: Query<&RoadEndpoint>,
    segments: Query<&RoadSegment>,
) {
    for endpoint in &endpoints {
        let standing = endpoint.at.world_position() + GIZMO_LIFT;
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
                let along = step as f32 / SEGMENT_SUBDIVISIONS as f32 * segment.length();
                segment.world_position(segment.starts_at() + along) + GIZMO_LIFT
            }),
            SLOW_LANE_COLOUR.mix(&LANE_COLOUR, segment.speed_limit() / STRAIGHT_SPEED_LIMIT),
        );

        let Some(next) = next.and_then(|next| onward.get(next.0).ok()) else {
            continue;
        };
        gizmos.arrow(
            segment.world_position(segment.ends_at() - HANDOVER_REACH * segment.length())
                + GIZMO_LIFT,
            next.world_position(next.starts_at() + HANDOVER_REACH * next.length()) + GIZMO_LIFT,
            HANDOVER_COLOUR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::common::initialize::InitializationFailed;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::map::MAP_TILE_SIZE;
    use crate::testing::{headless_app, tick};
    use std::collections::HashSet;

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// How many straight pieces a segment is measured in.
    const LENGTH_SAMPLES: usize = 128;

    /// A straight run of tiles, in offset-row coordinates.
    const STRAIGHT: [(i32, i32); 4] = [(0, 0), (1, 0), (2, 0), (3, 0)];

    /// A straight run of tiles crossing `STRAIGHT` at the last of them, in offset-row coordinates.
    ///
    /// A rover arriving down `STRAIGHT` may leave by either arm: the one turning sixty degrees, or
    /// the one turning a hundred and twenty back on itself.
    const TURNING_OFF: [(i32, i32); 3] = [(2, 1), (3, 0), (3, -1)];

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

    /// A run whose arc crosses tiles its two nodes do not stand on, in offset-row coordinates.
    const ACROSS_THE_TILES: [(i32, i32); 2] = SPANNING[0];

    /// A tile the run of `ACROSS_THE_TILES` crosses on its way, in offset-row coordinates.
    const IN_THE_WAY: (i32, i32) = (3, 0);

    /// A tile the run of `ACROSS_THE_TILES` passes beside, in offset-row coordinates.
    const OFF_TO_THE_SIDE: (i32, i32) = (3, 1);

    /// A run of tiles setting off from the last tile of `STRAIGHT`, in offset-row coordinates.
    const ONWARD: [(i32, i32); 2] = [(3, 0), (3, 1)];

    /// A direction from a tile's middle far enough towards a corner of it to settle on that corner.
    const TOWARDS_A_CORNER: Vec3 = Vec3::new(0., 0., MAP_TILE_INRADIUS);

    /// A direction from a tile's middle far enough towards the corner two round from that one.
    const TOWARDS_THE_CORNER_TWO_ROUND: Vec3 =
        Vec3::new(MAP_TILE_INRADIUS, 0., -MAP_TILE_INRADIUS / 2.);

    /// A direction from a tile's middle towards the corner one round from `TOWARDS_A_CORNER`.
    const TOWARDS_THE_NEXT_CORNER: Vec3 = Vec3::new(MAP_TILE_INRADIUS, 0., MAP_TILE_INRADIUS / 2.);

    /// A direction from a tile's middle towards the corner opposite `TOWARDS_THE_NEXT_CORNER`.
    const TOWARDS_THE_CORNER_OPPOSITE: Vec3 =
        Vec3::new(-MAP_TILE_INRADIUS, 0., -MAP_TILE_INRADIUS / 2.);

    /// A straight run crossing the curve of `TURNING` between its nodes, in offset-row coordinates.
    const ACROSS_THE_CURVE: [(i32, i32); 2] = [(0, 2), (2, 0)];

    /// A run that turns a corner of its own and crosses the curve of `TURNING` while it is turning,
    /// in offset-row coordinates. Both roads meet on an arc, which is the pair of circles.
    const CURVING_ACROSS: [(i32, i32); 3] = [(2, -1), (2, 0), (0, 1)];

    fn app_holding(tool: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(tool)
            .insert_resource(PlayerInput::default())
            .add_plugins((DebugGizmosPlugin, CleanupPlugin, RoadPlugin, BuildingPlugin));
        app
    }

    fn road_app() -> App {
        app_holding(PlayerAction::Select)
    }

    fn tile(offset: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offset.0, offset.1)
    }

    fn tiles(offsets: &[(i32, i32)]) -> Vec<HexCoordinates> {
        offsets.iter().copied().map(tile).collect()
    }

    fn centre_of(offset: (i32, i32)) -> LatticeNode {
        LatticeNode::from_tile(tile(offset))
    }

    fn nodes(offsets: &[(i32, i32)]) -> Vec<LatticeNode> {
        offsets.iter().copied().map(centre_of).collect()
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
                one_way: false,
            })
            .id()
    }

    fn built_road(offsets: &[(i32, i32)]) -> (App, Entity) {
        let mut app = road_app();
        let road = spawn_road(&mut app, offsets);
        tick(&mut app);
        (app, road)
    }

    fn spawn_one_way_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        spawn_one_way_road_through(app, &nodes(offsets))
    }

    fn spawn_one_way_road_through(app: &mut App, path: &[LatticeNode]) -> Entity {
        app.world_mut()
            .spawn(Road {
                nodes: path.to_vec(),
                leaving: None,
                one_way: true,
            })
            .id()
    }

    /// An app holding a one-way road through `offsets`, laid and cut into segments.
    fn built_one_way_road(offsets: &[(i32, i32)]) -> (App, Entity) {
        let mut app = road_app();
        let road = spawn_one_way_road(&mut app, offsets);
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

    /// Where a rover `along` of the way down `segment` stands, as a fraction of its stretch.
    fn position(app: &App, segment: Entity, along: f32) -> Vec3 {
        component_of::<RoadSegment>(app, segment)
            .map(|piece| piece.world_position(piece.starts_at() + along * piece.length()))
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
    fn a_one_way_road_gets_a_single_lane() {
        let (app, road) = built_one_way_road(&STRAIGHT);

        assert_eq!(lanes(&app, road).len(), 1);
    }

    #[test]
    fn the_lane_of_a_one_way_road_runs_the_way_it_was_drawn() {
        let path = tiles(&STRAIGHT);
        let (app, road) = built_one_way_road(&STRAIGHT);

        let lane = lane_from(&app, road, path[0]);

        let end = position(&app, *lane.last().expect("the lane has segments"), 1.);
        assert!(end.distance(path[STRAIGHT.len() - 1].world_position()) < TOLERANCE);
    }

    #[test]
    fn no_lane_of_a_one_way_road_sets_off_from_its_far_end() {
        let path = tiles(&STRAIGHT);
        let (app, road) = built_one_way_road(&STRAIGHT);

        assert!(lane_from(&app, road, path[STRAIGHT.len() - 1]).is_empty());
    }

    #[test]
    fn the_segments_of_a_one_way_road_cover_the_whole_road_once() {
        let (app, road) = built_one_way_road(&STRAIGHT);
        let drawn = run_through(&STRAIGHT);

        let driven: f32 = lanes(&app, road)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
            .map(|segment| length_of(&app, segment))
            .sum();

        assert!(
            (driven - drawn).abs() < drawn * TOLERANCE,
            "{driven} driven against {drawn} drawn"
        );
    }

    #[test]
    fn a_one_way_road_ends_rather_than_turning_round() {
        for offsets in [STRAIGHT.as_slice(), TURNING.as_slice()] {
            let (mut app, road) = built_one_way_road(offsets);
            let lane = lane_from(&app, road, tiles(offsets)[0]);

            let mut driven = vec![*lane.first().expect("the lane has segments")];
            while let Some(next) = next_of(&app, *driven.last().expect("the drive has a segment")) {
                assert!(!driven.contains(&next), "the drive came back on itself");
                driven.push(next);
            }

            assert_eq!(driven.len(), segments_in_the_world(&mut app));
        }
    }

    #[test]
    fn no_segment_of_a_one_way_road_runs_against_it() {
        let path = tiles(&STRAIGHT);
        let (app, road) = built_one_way_road(&STRAIGHT);
        let drawn = path[STRAIGHT.len() - 1].world_position() - path[0].world_position();

        for segment in lanes(&app, road)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
        {
            let heading = position(&app, segment, 1.) - position(&app, segment, 0.);
            assert!(
                heading.dot(drawn) > 0.,
                "a segment runs back along the road"
            );
        }
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
        LatticeNode::nearest_on(tile(offset), tile(offset).world_position() + towards)
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

    /// Pick up `tool`, and take the frame the change lands on.
    fn take_up(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
        tick(app);
    }

    /// Put a building on the tile at `offset`, and give the road tool back.
    ///
    /// The building tool is what places one, so the test picks it up the way a player does rather
    /// than writing the building into the world behind the rule that refuses it.
    fn put_a_building_on(app: &mut App, offset: (i32, i32)) -> Entity {
        let (col, row) = offset;
        let tile = app
            .world_mut()
            .spawn(MapTile {
                coordinates: HexCoordinates::from_offset_row(col, row),
            })
            .id();
        take_up(app, PlayerAction::EditBuildings);
        tap_on(app, tile);
        take_up(app, PlayerAction::EditRoads);
        tile
    }

    /// Take the building on `tile` off again, with the tool that put it there.
    fn take_the_building_off(app: &mut App, tile: Entity) {
        take_up(app, PlayerAction::EditBuildings);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = true;
            input.cursor_tile = Some(tile);
        }
        tick(app);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = false;
            input.cursor_tile = None;
        }
        take_up(app, PlayerAction::EditRoads);
    }

    /// Click on `tile` with the tool in hand, and take the frame that reads the click.
    fn tap_on(app: &mut App, tile: Entity) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = Some(tile);
        }
        tick(app);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = false;
            input.cursor_tile = None;
        }
    }

    #[test]
    fn an_arc_drawn_across_a_tile_a_building_stands_on_is_not_laid() {
        let mut app = app_holding(PlayerAction::EditRoads);
        put_a_building_on(&mut app, IN_THE_WAY);
        let across = nodes(&ACROSS_THE_TILES);

        click_at(&mut app, across[0]);
        click_at(&mut app, across[1]);
        finish_the_road(&mut app);

        assert_eq!(roads_in_the_world(&mut app), 0);
    }

    #[test]
    fn the_arcs_placed_before_the_building_are_kept() {
        let mut app = app_holding(PlayerAction::EditRoads);
        put_a_building_on(&mut app, IN_THE_WAY);
        let reached = [ACROSS_THE_TILES[0], (1, 0)];

        for &node in &nodes(&reached) {
            click_at(&mut app, node);
        }
        click_at(&mut app, nodes(&ACROSS_THE_TILES)[1]);
        finish_the_road(&mut app);

        assert!(a_road_runs_through(&mut app, &reached));
    }

    #[test]
    fn an_arc_that_passes_beside_a_building_is_laid() {
        let mut app = app_holding(PlayerAction::EditRoads);
        put_a_building_on(&mut app, OFF_TO_THE_SIDE);

        place_road(&mut app, &nodes(&ACROSS_THE_TILES));

        let road = road_through(&mut app, &ACROSS_THE_TILES);
        let beside = HexCoordinates::from_offset_row(OFF_TO_THE_SIDE.0, OFF_TO_THE_SIDE.1);
        let occupied = occupied_tiles(&app, road);
        assert!(!occupied.contains(&beside), "{occupied:?}");
        assert!(
            occupied.iter().any(|&tile| are_neighbours(tile, beside)),
            "{occupied:?} runs nowhere near the building"
        );
    }

    #[test]
    fn a_road_cannot_be_begun_on_a_tile_a_building_stands_on() {
        let mut app = app_holding(PlayerAction::EditRoads);
        put_a_building_on(&mut app, ACROSS_THE_TILES[0]);

        click_at(&mut app, nodes(&ACROSS_THE_TILES)[0]);

        assert_eq!(placing(&mut app), 0);
    }

    #[test]
    fn a_route_a_building_blocked_is_drawable_once_it_is_taken_away() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let tile = put_a_building_on(&mut app, IN_THE_WAY);
        take_the_building_off(&mut app, tile);

        place_road(&mut app, &nodes(&ACROSS_THE_TILES));

        assert!(a_road_runs_through(&mut app, &ACROSS_THE_TILES));
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
        assert_eq!(
            first.world_position(first.starts_at()),
            whole.world_position(whole.starts_at())
        );
        assert_eq!(
            second.world_position(second.ends_at()),
            whole.world_position(whole.ends_at())
        );
        assert_eq!(
            first.world_position(first.ends_at()),
            second.world_position(second.starts_at())
        );
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
            assert_eq!(
                piece.world_position(piece.starts_at()),
                arc.position(opened)
            );
            assert_eq!(piece.world_position(piece.ends_at()), arc.position(closed));
            opened = closed;
        }

        assert_eq!(arc.position(opened), whole.world_position(whole.ends_at()));
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

    /// The two corners of the tile at `offset` a road drawn between crosses `STRAIGHT` at its
    /// middle.
    ///
    /// The tile's other diameter, so the crossing it makes stands nearer the one `crossing_arm`
    /// makes than a junction's extent: the two have no room between them for either turn.
    fn a_second_arm_across(offset: (i32, i32)) -> Vec<LatticeNode> {
        vec![
            corner_of(offset, TOWARDS_THE_NEXT_CORNER),
            corner_of(offset, TOWARDS_THE_CORNER_OPPOSITE),
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

    /// Every cut a test heard announced, as the segment cut and the stretch beyond it.
    #[derive(Resource, Default)]
    struct Announced(Vec<(Entity, Entity)>);

    #[test]
    fn cutting_a_road_announces_the_stretch_beyond_the_cut() {
        let mut app = road_app();
        app.init_resource::<Announced>();
        app.add_observer(|cut: On<SegmentCut>, mut heard: ResMut<Announced>| {
            heard.0.push((cut.segment, cut.beyond));
        });
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        let heard = app.world().resource::<Announced>().0.clone();
        assert!(!heard.is_empty(), "a segment was cut and nothing said so");
        for (segment, beyond) in heard {
            assert_eq!(next_of(&app, segment), Some(beyond));
            assert_eq!(position(&app, segment, 1.), position(&app, beyond, 0.));
        }
    }

    #[test]
    fn cutting_a_one_way_road_at_a_junction_leaves_it_one_way() {
        let path = tiles(&STRAIGHT);
        let mut app = road_app();
        let crossed = spawn_one_way_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        let placed = path[STRAIGHT.len() - 1].world_position() - path[0].world_position();
        assert_eq!(lanes(&app, crossed).len(), 1);
        for segment in lanes(&app, crossed)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
        {
            let heading = position(&app, segment, 1.) - position(&app, segment, 0.);
            assert!(
                heading.dot(placed) > 0.,
                "a segment runs back along the road"
            );
        }
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
            let ends = position(&app, segment, 1.).distance(at);
            if (ends - JUNCTION_PULLBACK).abs() > TOLERANCE {
                continue;
            }
            let onward = next_of(&app, segment).expect("the segment past the cut");
            let before = component_of::<RoadSegment>(&app, segment).expect("the segment before");
            let after = component_of::<RoadSegment>(&app, onward).expect("the segment after");

            assert_eq!(before.arc, after.arc);
            assert_eq!(before.to, after.from);
            met += 1;
        }

        assert_eq!(
            met,
            2 * lanes(&app, crossed).len(),
            "a lane the junction did not take a stretch of at both ends"
        );
    }

    #[test]
    fn a_road_that_would_cross_another_too_near_a_crossing_already_there_is_not_laid() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);
        let standing = roads_in_the_world(&mut app);
        assert_eq!(junctions(&mut app).len(), 1);

        place_road(&mut app, &a_second_arm_across(STRAIGHT[1]));

        assert_eq!(
            junctions(&mut app).len(),
            1,
            "a second crossing with no room for the turns through either"
        );
        assert_eq!(roads_in_the_world(&mut app), standing, "the road was laid");
    }

    #[test]
    fn a_road_crossed_far_enough_from_a_crossing_already_there_is_laid() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        place_road(&mut app, &crossing_arm(STRAIGHT[2]));

        assert_eq!(junctions(&mut app).len(), 2);
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
            before + 4 * lanes(&app, crossed).len()
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

    /// The one junction in the world, as the entity it is.
    fn the_junction_entity(app: &mut App) -> Entity {
        let mut found: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Junction>>()
            .iter(app.world())
            .collect();
        let one = found.pop();
        assert!(found.is_empty(), "more than one junction");
        one.expect("a junction")
    }

    /// The legs of the one junction in the world.
    fn the_legs(app: &mut App) -> Vec<JunctionLeg> {
        let junction = the_junction_entity(app);
        component_of::<JunctionLegs>(app, junction)
            .map(|legs| legs.0.clone())
            .expect("the junction has legs")
    }

    /// The policy the one junction in the world holds.
    fn the_policy(app: &mut App) -> JunctionPolicy {
        let junction = the_junction_entity(app);
        component_of::<JunctionPolicy>(app, junction)
            .cloned()
            .expect("the junction has a policy")
    }

    /// The ways out of the one junction in the world, for a rover arriving on `leg`.
    fn exits_from(app: &mut App, leg: usize) -> Vec<Entity> {
        let junction = the_junction_entity(app);
        component_of::<JunctionLegs>(app, junction)
            .map(|legs| legs.exits_from(leg))
            .expect("the junction has legs")
    }

    /// `STRAIGHT`, and a road laid across its far end a frame later.
    fn a_road_turned_off() -> (App, Entity) {
        let mut app = road_app();
        let arriving = spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road(&mut app, &TURNING_OFF);
        tick(&mut app);
        (app, arriving)
    }

    /// Which leg of the one junction in the world is the arm of `road`, of which there is one.
    fn the_leg_of(app: &mut App, road: Entity) -> usize {
        the_legs(app)
            .iter()
            .position(|leg| leg.road == road)
            .expect("the road has a leg")
    }

    /// The arc under `segment`.
    fn arc_under(app: &App, segment: Entity) -> Arc {
        component_of::<RoadSegment>(app, segment)
            .map(|piece| piece.arc)
            .expect("the segment is there")
    }

    /// How fast `segment` allows.
    fn speed_limit_of(app: &App, segment: Entity) -> f32 {
        component_of::<RoadSegment>(app, segment)
            .map(RoadSegment::speed_limit)
            .expect("the segment is there")
    }

    /// Where `segment` ends, and which way a rover leaving it is pointing.
    fn leaves(app: &App, segment: Entity) -> (Vec3, Vec3) {
        let piece = component_of::<RoadSegment>(app, segment).expect("the segment is there");
        (
            piece.world_position(piece.ends_at()),
            piece.heading_at(piece.ends_at()),
        )
    }

    /// Where `segment` begins, and which way a rover setting off down it is pointing.
    fn sets_off(app: &App, segment: Entity) -> (Vec3, Vec3) {
        let piece = component_of::<RoadSegment>(app, segment).expect("the segment is there");
        (
            piece.world_position(piece.starts_at()),
            piece.heading_at(piece.starts_at()),
        )
    }

    #[test]
    fn a_rover_turning_at_a_junction_leaves_the_leg_it_arrived_on_along_it() {
        let (mut app, arriving) = a_road_turned_off();
        let leg = the_leg_of(&mut app, arriving);
        let reached = the_legs(&mut app)[leg].arriving.clone();

        for way in exits_from(&mut app, leg) {
            let (ends, heading) = leaves(&app, reached[0]);
            let (begins, along) = sets_off(&app, way);
            assert!(
                begins.distance(ends) < TOLERANCE,
                "a turn beginning off the leg"
            );
            assert!(
                along.dot(heading) > 1. - TOLERANCE,
                "a corner in the junction"
            );
        }
    }

    #[test]
    fn a_rover_turning_at_a_junction_reaches_the_leg_it_leaves_by_along_it() {
        let (mut app, arriving) = a_road_turned_off();
        let leg = the_leg_of(&mut app, arriving);

        let legs = the_legs(&mut app);
        for way in exits_from(&mut app, leg) {
            let last = the_end_of_the_way(&app, way, &legs);
            let onward = next_of(&app, last).expect("the way runs onto a leg");
            let (ends, heading) = leaves(&app, last);
            let (begins, along) = sets_off(&app, onward);
            assert!(
                ends.distance(begins) < TOLERANCE,
                "a turn ending off the leg"
            );
            assert!(
                heading.dot(along) > 1. - TOLERANCE,
                "a corner in the junction"
            );
        }
    }

    /// The last segment of the way through a junction that begins at `way`.
    fn the_end_of_the_way(app: &App, way: Entity, legs: &[JunctionLeg]) -> Entity {
        let mut standing = way;
        for _ in 0..STEPS_THROUGH_A_JUNCTION {
            let Some(onward) = next_of(app, standing) else {
                break;
            };
            if legs.iter().any(|leg| leg.leaving.contains(&onward)) {
                break;
            }
            standing = onward;
        }
        standing
    }

    #[test]
    fn a_turn_where_a_leg_curves_through_a_junction_still_meets_both_legs_along_them() {
        let mut app = road_app();
        spawn_road(&mut app, &CURVING_ACROSS);
        tick(&mut app);
        spawn_road(&mut app, &ACROSS_THE_CURVE);
        tick(&mut app);

        let mut met = 0;
        for (_, legs) in legs_in_the_world(&mut app) {
            let arms = JunctionLegs(legs.clone());
            for (index, leg) in legs.iter().enumerate() {
                for way in arms.exits_from(index) {
                    let (ends, heading) = leaves(&app, leg.arriving[0]);
                    let (begins, along) = sets_off(&app, way);
                    assert!(
                        begins.distance(ends) < TOLERANCE,
                        "a turn beginning off the leg"
                    );
                    assert!(
                        along.dot(heading) > 1. - TOLERANCE,
                        "a corner leaving the junction"
                    );

                    let last = the_end_of_the_way(&app, way, &legs);
                    let onward = next_of(&app, last).expect("the way runs onto a leg");
                    let (stops, leaving) = leaves(&app, last);
                    let (takes_up, onto) = sets_off(&app, onward);
                    assert!(
                        stops.distance(takes_up) < TOLERANCE,
                        "a turn ending off the leg"
                    );
                    assert!(
                        leaving.dot(onto) > 1. - TOLERANCE,
                        "a corner reaching the leg"
                    );
                    met += 1;
                }
            }
        }

        assert!(
            met > 0,
            "no way through any junction of two crossing curves"
        );
    }

    #[test]
    fn a_sixty_degree_turn_through_a_junction_is_as_tight_as_the_road_tool_will_build() {
        let (mut app, arriving) = a_road_turned_off();
        let leg = the_leg_of(&mut app, arriving);
        let sweeping = exits_from(&mut app, leg)[0];

        let radius = arc_under(&app, sweeping).radius();

        assert!(
            (radius - MIN_TURN_RADIUS).abs() < TOLERANCE,
            "a turn of radius {radius} against the tightest arc the tool lays, {MIN_TURN_RADIUS}"
        );
    }

    #[test]
    fn a_sharper_turn_through_a_junction_is_held_slower_than_a_sweeping_one() {
        let (mut app, arriving) = a_road_turned_off();
        let leg = the_leg_of(&mut app, arriving);
        let ways = exits_from(&mut app, leg);

        assert_eq!(ways.len(), 2, "a junction of other than two ways out");
        let (sweeping, sharp) = (speed_limit_of(&app, ways[0]), speed_limit_of(&app, ways[1]));
        assert!(
            sharp < sweeping,
            "{sharp} round the sharp turn against {sweeping}"
        );
    }

    #[test]
    fn carrying_straight_on_through_a_junction_is_driven_at_the_speed_of_the_road() {
        let (mut app, crossed, _) = a_crossed_road();
        let leg = the_leg_of(&mut app, crossed);

        let ways = exits_from(&mut app, leg);
        let straight_on = ways.first().copied().expect("a way out");

        assert!(
            (speed_limit_of(&app, straight_on) - STRAIGHT_SPEED_LIMIT).abs() < TOLERANCE,
            "a straight road slowed by being crossed"
        );
    }

    #[test]
    fn a_road_ending_on_another_meets_it_at_a_junction_of_three_legs() {
        let mut app = a_road_placed_onto_another();

        assert_eq!(the_legs(&mut app).len(), 3);
    }

    #[test]
    fn a_crossroads_has_a_leg_for_each_way_out_of_it() {
        let (mut app, ..) = a_crossed_road();

        assert_eq!(the_legs(&mut app).len(), 4);
    }

    #[test]
    fn a_leg_of_a_two_way_crossroads_is_both_arrived_on_and_left_by() {
        let (mut app, ..) = a_crossed_road();

        for leg in the_legs(&mut app) {
            assert!(!leg.arriving.is_empty(), "a leg nothing arrives on");
            assert!(!leg.leaving.is_empty(), "a leg nothing leaves by");
        }
    }

    #[test]
    fn every_segment_a_leg_names_stops_a_pull_back_short_of_the_crossing() {
        let (mut app, ..) = a_crossed_road();
        let (at, _) = the_junction(&mut app);

        for leg in the_legs(&mut app) {
            for segment in leg.arriving {
                let ends = position(&app, segment, 1.).distance(at);
                assert!(
                    (ends - JUNCTION_PULLBACK).abs() < TOLERANCE,
                    "arriving {ends} out"
                );
            }
            for segment in leg.leaving {
                let starts = position(&app, segment, 0.).distance(at);
                assert!(
                    (starts - JUNCTION_PULLBACK).abs() < TOLERANCE,
                    "leaving {starts} out"
                );
            }
        }
    }

    #[test]
    fn a_junction_offers_no_way_out_down_the_leg_it_was_arrived_on() {
        let (mut app, ..) = a_crossed_road();

        for (index, leg) in the_legs(&mut app).into_iter().enumerate() {
            let out = exits_from(&mut app, index);
            for back in leg.leaving {
                assert!(!out.contains(&back), "a way back out of the leg arrived on");
            }
        }
    }

    #[test]
    fn the_first_way_out_of_a_crossroads_carries_on_down_the_road_arrived_on() {
        let (mut app, crossed, _) = a_crossed_road();
        let straight_on = segments_under(&app, crossed);

        for (index, leg) in the_legs(&mut app).into_iter().enumerate() {
            if leg.road != crossed {
                continue;
            }
            let out = exits_from(&mut app, index);
            let first = out.first().expect("a way out");
            assert!(
                straight_on.contains(first),
                "a turn taken before the straight"
            );
        }
    }

    #[test]
    fn a_road_drawn_across_one_already_standing_gives_way_to_it() {
        let (mut app, crossed, _) = a_crossed_road();

        assert_eq!(the_policy(&mut app), JunctionPolicy::GiveWayTo(crossed));
    }

    #[test]
    fn two_roads_laid_on_one_frame_take_turns_where_they_cross() {
        let mut app = road_app();
        spawn_road(&mut app, &STRAIGHT);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);

        assert_eq!(the_policy(&mut app), JunctionPolicy::TakeTurns);
    }

    #[test]
    fn a_road_crossed_again_still_names_the_segments_that_reach_its_first_junction() {
        let mut app = road_app();
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[2]));
        tick(&mut app);

        for (junction, legs) in legs_in_the_world(&mut app) {
            for leg in legs {
                for segment in leg.arriving {
                    let ends = position(&app, segment, 1.).distance(junction);
                    assert!((ends - JUNCTION_PULLBACK).abs() < TOLERANCE, "{ends} out");
                }
            }
        }
    }

    /// Every junction in the world, as where it stands and the legs it has.
    fn legs_in_the_world(app: &mut App) -> Vec<(Vec3, Vec<JunctionLeg>)> {
        app.world_mut()
            .query::<(&Junction, &JunctionLegs)>()
            .iter(app.world())
            .map(|(junction, legs)| (junction.at, legs.0.clone()))
            .collect()
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
    fn a_one_way_road_occupies_the_same_tiles_as_a_two_way_one() {
        let (both_ways, two_way) = built_road(&TURNING);
        let (one_way, single) = built_one_way_road(&TURNING);

        assert_eq!(
            occupied_tiles(&one_way, single),
            occupied_tiles(&both_ways, two_way)
        );
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

    /// The tile whose corner the endpoint under test stands on, in offset-row coordinates.
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

    /// The corner of `BUILT_ON` the roads in these tests reach it on.
    fn served_corner() -> LatticeNode {
        corner_of(BUILT_ON, Vec3::Z * MAP_TILE_SIZE)
    }

    fn spawn_endpoint(app: &mut App, at: LatticeNode) -> Entity {
        app.world_mut().spawn(RoadEndpoint::at(at)).id()
    }

    fn served_by(app: &App, endpoint: Entity) -> Option<ServedBy> {
        component_of::<RoadEndpoint>(app, endpoint).and_then(RoadEndpoint::served_by)
    }

    /// Where on the ground the road serving `endpoint` stops for it.
    fn served_at(app: &App, endpoint: Entity) -> Option<Vec3> {
        let served = served_by(app, endpoint)?;
        let segment = component_of::<RoadSegment>(app, served.segment)?;
        Some(segment.world_position(served.along))
    }

    /// An app holding an endpoint on a corner of `BUILT_ON` and a road ending on that corner.
    fn served_app() -> (App, Entity) {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        spawn_road_through(&mut app, &[centre_of(BESIDE), served_corner()]);
        tick(&mut app);
        (app, endpoint)
    }

    #[test]
    fn an_endpoint_is_served_by_a_road_standing_on_the_corner_it_names() {
        let (app, endpoint) = served_app();

        assert!(served_by(&app, endpoint).is_some());
    }

    #[test]
    fn an_endpoint_is_served_where_the_road_stands_on_the_corner_it_names() {
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
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        spawn_road(&mut app, &[NEXT_DOOR, AWAY]);

        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn a_road_on_another_corner_of_the_same_tile_does_not_serve_the_endpoint() {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        let elsewhere = corner_of(BUILT_ON, Vec3::NEG_Z * MAP_TILE_SIZE);
        spawn_road_through(&mut app, &[centre_of(NEXT_DOOR), elsewhere]);

        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn a_road_through_the_middle_of_a_tile_does_not_serve_what_stands_on_it() {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        spawn_road(&mut app, &[BUILT_ON, NEXT_DOOR]);

        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn an_endpoint_is_served_by_a_road_laid_after_it() {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());

        tick(&mut app);
        assert!(
            served_by(&app, endpoint).is_none(),
            "served before any road was laid"
        );

        spawn_road_through(&mut app, &[centre_of(BESIDE), served_corner()]);
        tick(&mut app);

        assert!(served_by(&app, endpoint).is_some());
    }

    #[test]
    fn an_endpoint_whose_road_is_removed_reports_that_nothing_serves_it() {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        let road = spawn_road_through(&mut app, &[centre_of(BESIDE), served_corner()]);
        tick(&mut app);

        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert!(served_by(&app, endpoint).is_none());
    }

    #[test]
    fn cutting_the_road_leaves_the_endpoint_served_in_the_same_place() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let endpoint = spawn_endpoint(&mut app, served_corner());
        spawn_road_through(
            &mut app,
            &[centre_of(ALONG), centre_of(BESIDE), served_corner()],
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
    fn an_endpoint_is_served_by_a_one_way_road_setting_off_from_its_corner() {
        let mut app = road_app();
        let endpoint = spawn_endpoint(&mut app, served_corner());
        spawn_one_way_road_through(&mut app, &[served_corner(), centre_of(BESIDE)]);

        tick(&mut app);

        let standing = served_at(&app, endpoint).expect("the endpoint is served");
        let corner = served_corner().world_position();
        assert!(
            standing.distance(corner) < TOLERANCE,
            "served at {standing}, not at the corner {corner} the lane sets off from"
        );
    }

    /// A straight run whose arcs cross tiles neither of their nodes stands on, in offset-row
    /// coordinates. Removing the first of them gives back ground no surviving node stands on.
    const SPANNING_RUN: [(i32, i32); 3] = [(0, 0), (5, 0), (10, 0)];

    /// A tile far enough from anything laid that no road runs over it, in offset-row coordinates.
    const OFF_THE_MAP: (i32, i32) = (-6, -6);

    /// Right-click on the ground at `at`, and take the frame that reads it and the one after it.
    ///
    /// A right click is a secondary tap and a finish at once, which is what the mouse reports, so
    /// a test of removing a road is also a test of not finishing one. The frame after is what lays
    /// the roads a removal left and cuts them where they cross.
    fn right_click_at(app: &mut App, at: Vec3) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.ground_cursor_position = Some(at);
            input.secondary_tap = true;
            input.finish = true;
        }
        tick(app);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = false;
            input.finish = false;
        }
        tick(app);
    }

    /// Half way along the straight between the `index`th node of `offsets` and the one after it.
    fn middle_of(offsets: &[(i32, i32)], index: usize) -> Vec3 {
        let run = nodes(offsets);
        (run[index].world_position() + run[index + 1].world_position()) / 2.
    }

    /// `STRAIGHT` and an arm laid across the middle of it, with the road tool held.
    fn a_crossed_road_to_edit() -> App {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        spawn_road_through(&mut app, &crossing_arm(STRAIGHT[1]));
        tick(&mut app);
        app
    }

    /// The arc under every segment in the world, whichever road it belongs to.
    fn arcs_in_the_world(app: &mut App) -> Vec<Arc> {
        app.world_mut()
            .query::<&RoadSegment>()
            .iter(app.world())
            .map(|segment| segment.arc)
            .collect()
    }

    /// Whether any segment in the world covers the place `at`.
    fn a_segment_stands_at(app: &mut App, at: Vec3) -> bool {
        app.world_mut()
            .query::<&RoadSegment>()
            .iter(app.world())
            .any(|segment| {
                segment
                    .arc
                    .distance_along(at)
                    .is_some_and(|along| along >= segment.from && along <= segment.to)
            })
    }

    fn all_roads(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .collect()
    }

    #[test]
    fn removing_the_arc_under_the_cursor_leaves_the_road_either_side_of_it() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        assert_eq!(roads_in_the_world(&mut app), 2);
        assert!(a_road_runs_through(&mut app, &STRAIGHT[..2]));
        assert!(a_road_runs_through(&mut app, &STRAIGHT[2..]));
    }

    #[test]
    fn the_arcs_either_side_of_the_one_removed_stand_exactly_where_they_did() {
        let mut app = app_holding(PlayerAction::EditRoads);
        let road = spawn_road(&mut app, &WINDING);
        tick(&mut app);
        let before = arcs_under(&app, road);

        right_click_at(&mut app, middle_of(&WINDING, 1));

        let after = arcs_in_the_world(&mut app);
        assert!(!after.is_empty(), "a removal that left nothing standing");
        for arc in after {
            assert!(before.contains(&arc), "an arc that moved: {arc:?}");
        }
    }

    #[test]
    fn nothing_stands_where_the_arc_that_went_stood() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        let taken = middle_of(&STRAIGHT, 1);
        assert!(a_segment_stands_at(&mut app, taken));

        right_click_at(&mut app, taken);

        assert!(!a_segment_stands_at(&mut app, taken));
    }

    #[test]
    fn removing_the_arc_at_the_end_of_a_road_leaves_the_rest_of_it_whole() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        right_click_at(&mut app, middle_of(&STRAIGHT, 2));

        assert_eq!(roads_in_the_world(&mut app), 1);
        assert!(a_road_runs_through(&mut app, &STRAIGHT[..3]));
    }

    #[test]
    fn removing_the_only_arc_of_a_road_takes_the_road_with_it() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT[..2]);
        tick(&mut app);

        right_click_at(&mut app, middle_of(&STRAIGHT, 0));

        assert_eq!(roads_in_the_world(&mut app), 0);
        assert_eq!(segments_in_the_world(&mut app), 0);
    }

    #[test]
    fn removing_every_arc_of_a_road_takes_the_road_off_the_map() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        for arc in [1, 0, 2] {
            right_click_at(&mut app, middle_of(&STRAIGHT, arc));
        }

        assert_eq!(roads_in_the_world(&mut app), 0);
        assert_eq!(segments_in_the_world(&mut app), 0);
    }

    #[test]
    fn nothing_in_the_network_points_at_a_segment_that_went() {
        let mut app = a_crossed_road_to_edit();

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        let standing: HashSet<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<RoadSegment>>()
            .iter(app.world())
            .collect();
        let onward: Vec<Entity> = app
            .world_mut()
            .query::<&NextSegment>()
            .iter(app.world())
            .map(|next| next.0)
            .collect();
        assert!(!onward.is_empty(), "a network of nothing to drive");
        for next in onward {
            assert!(standing.contains(&next), "a segment onto nothing: {next}");
        }
    }

    #[test]
    fn no_segment_runs_into_a_junction_that_went() {
        let mut app = a_crossed_road_to_edit();

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        let standing: HashSet<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Junction>>()
            .iter(app.world())
            .collect();
        let reached: Vec<Entity> = app
            .world_mut()
            .query::<&EndsAtJunction>()
            .iter(app.world())
            .map(|ends| ends.junction)
            .collect();
        for junction in reached {
            assert!(
                standing.contains(&junction),
                "a segment into nothing: {junction}"
            );
        }
    }

    #[test]
    fn the_tiles_under_the_arc_that_went_take_a_road_again() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &SPANNING_RUN);
        tick(&mut app);
        assert!(
            !roads_over(&app, IN_THE_WAY).is_empty(),
            "a tile the run misses"
        );

        right_click_at(&mut app, middle_of(&SPANNING_RUN, 0));

        assert!(roads_over(&app, IN_THE_WAY).is_empty());
    }

    #[test]
    fn a_junction_a_removal_leaves_on_one_road_is_no_longer_a_junction() {
        let mut app = a_crossed_road_to_edit();
        assert_eq!(junctions(&mut app).len(), 1);

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        assert!(junctions(&mut app).is_empty());
    }

    #[test]
    fn a_junction_away_from_a_removal_keeps_the_legs_it_had() {
        let mut app = a_crossed_road_to_edit();
        let (before, _) = the_junction(&mut app);
        let legs = the_legs(&mut app).len();

        right_click_at(&mut app, middle_of(&STRAIGHT, 0));

        let (after, _) = the_junction(&mut app);
        assert!(
            after.distance(before) < TOLERANCE,
            "a junction at {after}, having stood at {before}"
        );
        assert_eq!(the_legs(&mut app).len(), legs);
    }

    #[test]
    fn a_one_way_road_a_removal_splits_leaves_two_one_way_roads() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_one_way_road(&mut app, &STRAIGHT);
        tick(&mut app);

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        let left = all_roads(&mut app);
        assert_eq!(left.len(), 2);
        for road in left {
            assert_eq!(lanes(&app, road).len(), 1);
        }
    }

    #[test]
    fn right_clicking_while_a_road_is_being_placed_finishes_it_and_removes_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);
        for node in nodes(&ONWARD) {
            click_at(&mut app, node);
        }

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert!(a_road_runs_through(&mut app, &ONWARD));
    }

    #[test]
    fn right_clicking_where_no_road_runs_removes_nothing() {
        let mut app = app_holding(PlayerAction::EditRoads);
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        right_click_at(&mut app, centre_of(OFF_THE_MAP).world_position());

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert_eq!(roads_in_the_world(&mut app), 1);
    }

    #[test]
    fn right_clicking_while_another_tool_is_held_removes_nothing() {
        let mut app = road_app();
        spawn_road(&mut app, &STRAIGHT);
        tick(&mut app);

        right_click_at(&mut app, middle_of(&STRAIGHT, 1));

        assert!(a_road_runs_through(&mut app, &STRAIGHT));
        assert_eq!(roads_in_the_world(&mut app), 1);
    }
}
