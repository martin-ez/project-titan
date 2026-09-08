//! The rovers a port has been given, and the shuttle each of them runs.
//!
//! This is where a rover stops being something a test spawns and becomes something a building
//! has. An input port is given a number of rovers, every one of them drives to a port making
//! what that input takes, takes on a load, drives back, hands it over and sets off again. The
//! lever is how many rovers serve an input, never which road they take: routing is
//! [`crate::road`]'s answer and the road itself is the player's, so the only way to move more is
//! to put more rovers on the road they built and live with what that does to it.
//!
//! Which port they collect from is found rather than named, an intake already saying what it
//! takes, and found again whenever the roads or the ports on them move. What crosses between the
//! two ends is [`crate::rover::Cargo`], carrying the item of the outlet it was collected from.

use crate::building::{Flow, Holding, Item, Port};
use crate::common::cleanup::Destroy;
use crate::diagnostics::DebugGizmos;
use crate::input::{DeclareCommands, PlayerAction, PlayerCommand, Requested};
use crate::map::LatticeNode;
use crate::road::{RoadEndpoint, RoadNetwork, RoadTiles};
use crate::rover::{Cargo, Route, Rover, RoversDriven, SentTo, Stranded};
use crate::simulation::Simulation;
use crate::ui::legend::{BindingContext, BindingInput};
use crate::ui::selection::{Picked, Selection};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashMap;

/// How much a rover takes on in one trip.
///
/// The same for every item, weight and bulk being no part of the game. What it has to be is more
/// than one, so a source holding less than a full load hands over what it has rather than nothing
/// at all, and less than what a port holds, so a full port takes more than one trip to empty.
const ROVER_LOAD: u32 = 4;

/// How far the debug view lifts a fleet's mark off the ground, so it does not fight the tiles.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.3, 0.);

/// The colour the way a fleet collects along is drawn in
const FLEET_COLOUR: Color = Color::srgb(0.6, 0.5, 0.9);

/// The keys that change the selected port's fleet, how many rovers each asks for, and what to
/// call that
const FLEET_KEYS: [(KeyCode, RoverRequest, &str); 2] = [
    (
        KeyCode::Minus,
        RoverRequest(-1),
        "Take a rover off the port you picked out",
    ),
    (
        KeyCode::Equal,
        RoverRequest(1),
        "Put another rover on the port you picked out",
    ),
];

/// How many rovers the player asked the port they picked out for, negative to ask for some back.
#[derive(Clone, Copy, PartialEq)]
struct RoverRequest(i32);

/// How many rovers the player has to give out across the whole map.
///
/// Scarce against the map as it stands: a couple of chains can be served well or several badly,
/// so which building gets a rover is a trade from the second one onwards. What the number ought
/// to grow with is progression's answer rather than this one's.
const ROVER_POOL: u32 = 12;

/// The rovers each port has been given, and the shuttle they run.
pub struct FleetPlugin;

/// The set the shuttles run in, so a system reading a port after they collected can follow them.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FleetsServed;

/// How many rovers the player has, all told.
///
/// One pool behind every [`Fleet`] on the map, which is what makes giving a rover to one port the
/// same act as taking it off another. What is spare is derived from the fleets themselves rather
/// than tallied here: a port that leaves the world takes its claim with it, and a tally kept
/// beside them is one that can disagree with them.
#[derive(Resource)]
pub struct RoverPool {
    /// How many rovers there are to give out.
    pub size: u32,
}

/// The port whose last request for a rover the pool could not fill.
///
/// A refused request writes nothing to the world — the count it asked to raise stays where it
/// was — so without a record of it the screen says the same thing before the press as after, and
/// the player is told nothing. It is a fact about the frame a key arrived on, read by what draws
/// and by no tick (invariant 2).
#[derive(Resource, Default, PartialEq)]
pub struct Refused(Option<Entity>);

/// The rovers a port has been given, and the port they were found to collect from.
///
/// The count is the port's own record rather than a fact about a frame, which is what leaves a
/// player free to write it on the frame their click arrives on (invariant 2): the next tick reads
/// the number they asked for and puts that many rovers on the road. The source beside it is the
/// tick's alone. Whatever carries this needs a [`RoadEndpoint`] for a rover to stand at, and a
/// fleet no road reaches or nothing supplies is idle rather than illegal — it starts running when
/// a road or a producer arrives.
#[derive(Component)]
#[require(OnTheRoad, LookingForASupplier)]
pub struct Fleet {
    /// How many rovers serve this port.
    pub rovers: u32,
    /// The port they collect their load from, where anything on the network makes what this takes.
    pub source: Option<Entity>,
}

/// A fleet with no supplier settled on it, waiting on the tick to find it one.
///
/// A fleet arrives carrying it, and a road laid or a port built gives it back, so the search runs
/// on the ticks the answer can have moved and on no others. Giving it up is what makes that one
/// search an assignment rather than one a tick: a fleet nothing supplies gives it up too, and
/// waits for the world to move rather than asking again.
#[derive(Component, Default)]
struct LookingForASupplier;

/// How many of a fleet's rovers are on the road, kept as it gains and loses them.
///
/// A tally rather than a count taken each tick: working out which port every rover on the map
/// belongs to, once a tick, is the one thing the fleet-scale corollary rules out. A rover already
/// carries the port it serves, so joining and leaving are what move this.
#[derive(Component, Default)]
struct OnTheRoad(u32);

/// A rover belonging to a fleet, naming the port that fleet serves.
///
/// Which way it is going is not stored. A rover carrying nothing is on its way for a load and one
/// carrying a load is bringing it home, so there is no second record of a trip that can disagree
/// with what is on the back of it.
#[derive(Component)]
struct Serving {
    port: Entity,
}

/// A rover its fleet has already given up, waiting on the world to take it.
///
/// A rover is marked for destruction on the tick and leaves the world at the end of the frame, and
/// a frame carries as many ticks as the speed the world is run at asks of it. This is what tells
/// the ticks in between that its place has already been given back, so a fleet counts a rover it
/// gave up once rather than once a tick until the frame ends.
#[derive(Component)]
struct Retired;

/// A rover with nothing to do: no route, no order for one, and not stopped for want of one.
type StandingIdle = (Without<Route>, Without<SentTo>, Without<Stranded>);

/// The intake the player has picked out to give rovers to, if that is what they have picked out.
///
/// The tool they hold, what they picked and whether it takes goods in are one question rather
/// than three, because every one of them has to answer before a key means anything.
#[derive(SystemParam)]
struct PickedOutIntake<'w, 's> {
    action: Res<'w, State<PlayerAction>>,
    selection: Res<'w, Selection>,
    ports: Query<'w, 's, &'static Port>,
}

impl PickedOutIntake<'_, '_> {
    /// The port a key is aimed at, which is an intake picked out with the select tool and nothing
    /// else — a fleet being a shuttle that brings a building what it takes in.
    fn port(&self) -> Option<Entity> {
        if *self.action.get() != PlayerAction::Select {
            return None;
        }
        let port = self.selection.port()?;
        self.ports
            .get(port)
            .is_ok_and(|door| door.flow == Flow::Intake)
            .then_some(port)
    }
}

impl Default for RoverPool {
    fn default() -> Self {
        Self { size: ROVER_POOL }
    }
}

impl RoverPool {
    /// How many of the pool are still to give, `assigned` being what the fleets hold between them.
    pub fn spare(&self, assigned: u32) -> u32 {
        self.size.saturating_sub(assigned)
    }
}

impl Refused {
    /// The port that last asked for a rover the pool had none of, if one did.
    pub fn port(&self) -> Option<Entity> {
        self.0
    }
}

impl Plugin for FleetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoverPool>()
            .init_resource::<Refused>()
            .add_observer(take_the_rovers_of_a_fleet_that_is_gone_off_the_road)
            .add_observer(give_back_the_place_of_a_rover_that_left_the_world)
            .declare_commands(FLEET_KEYS.map(|(key, asks, action)| PlayerCommand {
                input: BindingInput::Key(key),
                asks,
                action,
                context: BindingContext::Tool(PlayerAction::Select),
            }))
            .add_systems(
                FixedUpdate,
                (
                    ask_every_fleet_to_look_again_when_the_world_changes,
                    find_each_fleet_a_supplier,
                    turn_the_rovers_round_at_the_port_they_reached,
                    retire_the_rovers_a_fleet_no_longer_wants,
                    put_the_rovers_a_fleet_is_owed_on_the_road,
                    let_the_parked_rovers_try_again_when_the_world_changes,
                    set_the_idle_rovers_off_again,
                )
                    .chain()
                    .in_set(FleetsServed)
                    .after(RoversDriven)
                    .in_set(Simulation),
            )
            .add_systems(
                Update,
                (
                    forget_a_refusal_when_the_player_picks_something_else_out,
                    set_the_rovers_the_player_asked_for,
                    draw_the_way_a_fleet_collects_along,
                )
                    .chain()
                    .after(Picked),
            );
    }
}

/// Put another rover on the port the player picked out, or take one off it.
///
/// The count written is the one the simulation reads on the next tick, so what the player asked
/// for and what is running are one number rather than two that can disagree. An intake given its
/// first rover gains its fleet here, where those rovers collect from being the tick's to find
/// rather than the player's to say.
///
/// Every fleet's count together is what the pool has been spent on, so a rover put on one port is
/// one another cannot have, and one taken off is free to the next that asks on the same frame. A
/// request the pool cannot fill leaves the count where it was and is recorded as refused.
fn set_the_rovers_the_player_asked_for(
    mut commands: Commands,
    asked_for: Res<Requested<RoverRequest>>,
    picked_out: PickedOutIntake,
    pool: Res<RoverPool>,
    mut refused: ResMut<Refused>,
    mut fleets: Query<&mut Fleet>,
) {
    let Some(port) = picked_out.port() else {
        return;
    };
    let spare = pool.spare(fleets.iter().map(|fleet| fleet.rovers).sum());
    for RoverRequest(asked) in asked_for.iter() {
        if asked.is_positive() && spare < asked.unsigned_abs() {
            refused.set_if_neq(Refused(Some(port)));
            continue;
        }
        match fleets.get_mut(port) {
            Ok(mut fleet) => fleet.rovers = fleet.rovers.saturating_add_signed(asked),
            Err(_) if asked.is_positive() => {
                commands.entity(port).insert(Fleet {
                    rovers: asked.unsigned_abs(),
                    source: None,
                });
            }
            Err(_) => continue,
        }
        refused.set_if_neq(Refused(None));
    }
}

/// Forget a refusal once the player has picked something else out.
///
/// A refusal is about the port that asked for a rover, so one that outlives the picking of
/// another leaves the player reading about a request they made somewhere else. It is dropped
/// rather than carried over: what is spare may well have moved by the time they come back.
fn forget_a_refusal_when_the_player_picks_something_else_out(
    selection: Res<Selection>,
    mut refused: ResMut<Refused>,
) {
    if selection.is_changed() {
        refused.set_if_neq(Refused(None));
    }
}

/// Ask every fleet to look for its supplier again, the world having moved under the answer.
///
/// A road laid or taken up moves what a fleet can reach and how far off it is; a port built or
/// taken down moves what there is to reach. Nothing else can change which producer a fleet
/// settles on, so nothing else asks — which is what leaves a settled fleet costing no search at
/// all on the ticks in between.
fn ask_every_fleet_to_look_again_when_the_world_changes(
    mut commands: Commands,
    roads: Res<RoadTiles>,
    built: Query<(), Added<Port>>,
    mut taken_down: RemovedComponents<Port>,
    fleets: Query<Entity, With<Fleet>>,
) {
    let any_gone = taken_down.read().count() > 0;
    if !roads.is_changed() && built.is_empty() && !any_gone {
        return;
    }
    for fleet in &fleets {
        commands.entity(fleet).insert(LookingForASupplier);
    }
}

/// Find each fleet that is looking the port it collects from: the one making what the intake it
/// serves takes in, reached soonest by road.
///
/// Quickest by road rather than nearest across the grid, because a well-connected producer
/// further off is one a rover gets back from sooner than a near one at the end of a bad road,
/// which is what the network the player built is worth. One search outward serves every producer
/// at once, so a fleet costs a walk of the network when it is assigned rather than one a trip.
///
/// The candidates are put in the grid's order before the search, so two producers it reaches for
/// the same cost settle it between them the same way twice over (invariant 2).
fn find_each_fleet_a_supplier(
    mut commands: Commands,
    mut network: RoadNetwork,
    ports: Query<(Entity, &Port, &RoadEndpoint)>,
    mut looking: Query<(Entity, &mut Fleet), With<LookingForASupplier>>,
) {
    if looking.is_empty() {
        return;
    }
    let mut standing: HashMap<Item, Vec<(LatticeNode, Entity)>> = HashMap::new();
    for (port, door, endpoint) in &ports {
        if door.flow == Flow::Outlet {
            standing
                .entry(door.item)
                .or_default()
                .push((endpoint.standing_on(), port));
        }
    }
    let producing: HashMap<Item, Vec<Entity>> = standing
        .into_iter()
        .map(|(item, mut doors)| {
            doors.sort_unstable();
            (item, doors.into_iter().map(|(_, port)| port).collect())
        })
        .collect();

    for (port, mut fleet) in &mut looking {
        commands.entity(port).remove::<LookingForASupplier>();
        let Ok((_, door, endpoint)) = ports.get(port) else {
            continue;
        };
        let among = producing.get(&door.item).map_or(&[][..], Vec::as_slice);
        fleet.source = endpoint
            .served_by()
            .and_then(|place| network.quickest_of(place.segment, place.along, among));
    }
}

/// Let go of the route of every rover that has reached the end of it, taking on a load if it came
/// for one.
///
/// A route is spent the moment it is driven, and dropping it here leaves the next leg to be
/// decided from what the rover is carrying rather than from a record of the trip it just made. A
/// load only ever comes out of the outlet the rover is standing at (invariant 1).
///
/// A door with nothing to give and a door with no room for what was brought both leave the rover
/// waiting there holding the route it drove, so a jam costs no search on any tick it goes on for.
fn turn_the_rovers_round_at_the_port_they_reached(
    mut commands: Commands,
    fleets: Query<&Fleet>,
    mut ports: Query<(&RoadEndpoint, &Port, &mut Holding)>,
    rovers: Query<(Entity, &Rover, &Route, &Serving, Has<Cargo>)>,
) {
    for (entity, standing, route, serving, carrying) in &rovers {
        let arrived = ports
            .get(route.destination)
            .is_ok_and(|(endpoint, _, _)| standing.standing_at(endpoint));
        if !arrived {
            continue;
        }
        let Ok(fleet) = fleets.get(serving.port) else {
            continue;
        };
        if carrying && route.destination == serving.port {
            continue;
        }
        if carrying || Some(route.destination) != fleet.source {
            commands.entity(entity).remove::<Route>();
            continue;
        }

        let Ok((_, source, mut stood)) = ports.get_mut(route.destination) else {
            continue;
        };
        let taken = stood.give_out(ROVER_LOAD);
        if taken == 0 {
            continue;
        }
        commands
            .entity(entity)
            .insert(Cargo {
                item: source.item,
                quantity: taken,
            })
            .remove::<Route>();
    }
}

/// Take off the road the rovers of every fleet given fewer than it has out.
///
/// A rover leaves at the port it serves and nowhere else, so one taken away finishes the trip it
/// is on and hands over what it is carrying before it goes. Anything else is a load that stops
/// existing halfway down a road, which is the free transfer invariant 1 is there to forbid.
///
/// The tally comes down here rather than when the rover leaves the world, so a second rover at
/// the same port on the same tick is measured against what the fleet will have. One already given
/// up is marked as such and passed over, the world not having taken it yet.
fn retire_the_rovers_a_fleet_no_longer_wants(
    mut commands: Commands,
    mut fleets: Query<(&Fleet, &RoadEndpoint, &mut OnTheRoad)>,
    rovers: Query<(Entity, &Rover, &Serving), Without<Retired>>,
) {
    for (entity, standing, serving) in &rovers {
        let Ok((fleet, home, mut out)) = fleets.get_mut(serving.port) else {
            continue;
        };
        if out.0 <= fleet.rovers || !standing.standing_at(home) {
            continue;
        }
        out.0 -= 1;
        commands.entity(entity).insert((Retired, Destroy));
    }
}

/// Give a fleet back the place of a rover that left the world without being given up.
///
/// A rover is taken off the map by more than its own fleet: the ground it stands on can be
/// bulldozed out from under it, and the port it serves can be taken down with it still driving.
/// A place that is not given back is a place the fleet counts against a count it can no longer
/// fill, which leaves it running short of what the player asked for with no way to say so. One
/// already given up is not given back twice.
fn give_back_the_place_of_a_rover_that_left_the_world(
    removed: On<Remove, Serving>,
    rovers: Query<&Serving, Without<Retired>>,
    mut fleets: Query<&mut OnTheRoad>,
) {
    let Ok(serving) = rovers.get(removed.entity) else {
        return;
    };
    let Ok(mut out) = fleets.get_mut(serving.port) else {
        return;
    };
    out.0 = out.0.saturating_sub(1);
}

/// Put on the road however many rovers each fleet is short of what it was given.
///
/// They stand at the port they serve, which is both where a shuttle begins and the only place on
/// the network the fleet is sure of. A port no road reaches gets none and is owed them still, so
/// laying a road to it is what puts them out rather than asking for them again.
fn put_the_rovers_a_fleet_is_owed_on_the_road(
    mut commands: Commands,
    mut fleets: Query<(Entity, &Fleet, &RoadEndpoint, &mut OnTheRoad)>,
) {
    for (port, fleet, home, mut out) in &mut fleets {
        let Some(place) = home.served_by() else {
            continue;
        };
        while out.0 < fleet.rovers {
            commands.spawn((
                Rover {
                    segment: place.segment,
                    along: place.along,
                    speed: 0.,
                },
                Serving { port },
            ));
            out.0 += 1;
        }
    }
}

/// Offer every parked rover another go, once the roads or the fleet it belongs to have moved.
///
/// A standing assignment outlives the road it was made over, so a fleet whose source is bulldozed
/// away has to set off again when one comes back rather than staying parked for good, and one
/// that was supplied by nothing has to when a producer turns up. What it must not do is pay for
/// that with a search a tick: `RoadTiles` moves when a road is laid and when one is removed and
/// at no other time, and a fleet is written to only by the search that settled it. Both are facts
/// about ticks rather than frames, so a tick cannot miss one and two ticks in one frame cannot
/// both take it.
fn let_the_parked_rovers_try_again_when_the_world_changes(
    mut commands: Commands,
    roads: Res<RoadTiles>,
    settled: Query<(), Changed<Fleet>>,
    parked: Query<(Entity, &Serving), With<Stranded>>,
) {
    let roads_moved = roads.is_changed();
    for (entity, serving) in &parked {
        if roads_moved || settled.contains(serving.port) {
            commands.entity(entity).remove::<Stranded>();
        }
    }
}

/// Send every rover standing idle on to wherever the next leg of its shuttle takes it.
///
/// What it is carrying is what decides: an empty rover is going for a load and a loaded one is
/// bringing it home. A rover that cannot be routed is left standing where it is rather than sent
/// out to run out of road — which, since a shuttle is only ever sent from a port, is the port it
/// serves or the one it came to collect from.
///
/// A fleet nothing supplies parks its empty rovers rather than leaving them to drive on down the
/// lane they stand in: a rover with nowhere to be sent is as stopped as one that cannot reach
/// where it was sent.
fn set_the_idle_rovers_off_again(
    mut commands: Commands,
    fleets: Query<&Fleet>,
    idle: Query<(Entity, &Serving, Has<Cargo>), StandingIdle>,
) {
    for (entity, serving, carrying) in &idle {
        let Ok(fleet) = fleets.get(serving.port) else {
            continue;
        };
        let bound_for = if carrying {
            Some(serving.port)
        } else {
            fleet.source
        };
        match bound_for {
            Some(bound_for) => commands.entity(entity).insert(SentTo(bound_for)),
            None => commands.entity(entity).insert(Stranded),
        };
    }
}

/// Take a fleet's rovers off the road when the port they served leaves the world.
///
/// They would otherwise go on driving to a door that is not there, which is a delivery nothing
/// can ever take and a rover nothing can ever reassign. Buildings come down rarely enough that
/// reading every rover to find them is affordable here and nowhere on the tick.
fn take_the_rovers_of_a_fleet_that_is_gone_off_the_road(
    removed: On<Remove, Fleet>,
    mut commands: Commands,
    rovers: Query<(Entity, &Serving)>,
) {
    for (entity, serving) in &rovers {
        if serving.port == removed.entity {
            commands.entity(entity).insert(Destroy);
        }
    }
}

/// Draw the way each fleet collects along, from the port it takes from to the port it serves.
///
/// An assignment is otherwise invisible: the rovers running it look like any others on the road,
/// and which of the ports on screen a fleet was pointed at is the whole of what it was given
/// (invariant 5).
fn draw_the_way_a_fleet_collects_along(
    mut gizmos: Gizmos<DebugGizmos>,
    fleets: Query<(&Fleet, &RoadEndpoint)>,
    endpoints: Query<&RoadEndpoint>,
) {
    for (fleet, home) in &fleets {
        let Some(source) = fleet.source.and_then(|port| endpoints.get(port).ok()) else {
            continue;
        };
        gizmos.arrow(
            source.standing_on().world_position() + GIZMO_LIFT,
            home.standing_on().world_position() + GIZMO_LIFT,
            FLEET_COLOUR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{
        BuildingPlugin, BuildingTiles, BuildingType, ChosenBuildingType, Item, PORT_CAPACITY,
    };
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{Deposit, HexCoordinates, LatticeNode, MapTile, TileCorner};
    use crate::road::{Road, RoadPlugin, ServedBy};
    use crate::rover::RoverPlugin;
    use crate::simulation::SimulationPlugin;
    use crate::testing::{advance, ask_for, headless_app, press_key, release_key, tick};
    use crate::ui::selection::SelectionPlugin;

    /// The corner of a tile the road runs through and a port stands on.
    ///
    /// A road serves an endpoint only where one of its nodes stands on that endpoint's node, and
    /// only a corner is served at all: a tile's own middle is shared by no tiles, which is the
    /// answer a road through the middle of one already gave. The same corner of every tile in a
    /// run is a straight line of nodes, so a road laid through them is the road a rover drives.
    const PORT_CORNER: TileCorner = TileCorner::North;

    /// A straight run of tiles with a port at either end, in offset-row coordinates.
    const HAULAGE: [(i32, i32); 3] = [(0, 0), (1, 0), (2, 0)];

    /// The tile whose middle the port a fleet collects from stands on, in offset-row coordinates.
    const SOURCE: (i32, i32) = (0, 0);

    /// The tile whose middle the port a fleet serves stands on, in offset-row coordinates.
    const HOME: (i32, i32) = (2, 0);

    /// The tile a second port on the same road stands on, in offset-row coordinates.
    const OTHER_HOME: (i32, i32) = (1, 0);

    /// A run of tiles branching off `HAULAGE` at its first tile, in offset-row coordinates.
    const BRANCH: [(i32, i32); 4] = [(0, 0), (0, 1), (0, 2), (0, 3)];

    /// The tile a port stands on that only `BRANCH` reaches, in offset-row coordinates.
    const UP_THE_BRANCH: (i32, i32) = (0, 3);

    /// A road that sets off from the intake, runs out and comes back round, in offset-row
    /// coordinates.
    ///
    /// The intake stands on its first tile. `QUICK_BY_ROAD` is three tiles along it and
    /// `NEAR_ON_THE_GRID` is at the far end of it, eleven tiles of driving away, so which of the
    /// two is nearer depends entirely on whether the grid or the road is what measures it.
    const THE_LONG_WAY_ROUND: [(i32, i32); 12] = [
        (0, 0),
        (1, 0),
        (2, 0),
        (3, 0),
        (3, 1),
        (3, 2),
        (3, 3),
        (2, 3),
        (1, 3),
        (0, 3),
        (0, 2),
        (0, 1),
    ];

    /// The tile the intake stands on where the road bends back on itself, in offset-row
    /// coordinates.
    const BEND_HOME: (i32, i32) = (0, 0);

    /// The tile of the producer three tiles of road from `BEND_HOME`, in offset-row coordinates.
    const QUICK_BY_ROAD: (i32, i32) = (3, 0);

    /// The tile of the producer one tile from `BEND_HOME` across the grid, in offset-row
    /// coordinates, which the road only reaches by running all the way round.
    const NEAR_ON_THE_GRID: (i32, i32) = (0, 1);

    /// How many rovers a fleet under test is given.
    const A_FLEET: u32 = 2;

    /// How many rovers a fleet is given that the road it runs on has room for.
    const A_FLEET_THAT_FITS: u32 = 4;

    /// How many rovers a fleet is given that its road has nowhere to put.
    ///
    /// Well past what a round trip along `HAULAGE` holds, so the road is packed and a rover
    /// spends its ticks standing behind another rather than driving between the two ports.
    const A_FLEET_TOO_LARGE_FOR_ITS_ROAD: u32 = 64;

    /// How much a source under test is stocked with, which is everything a port will hold.
    ///
    /// A finite figure now that a port is bounded, and the whole of what the world holds in the
    /// tests that count what came out the other end.
    const A_STOCK: u32 = PORT_CAPACITY;

    /// The one item these tests haul, every port standing here being a door for it.
    const HAULED: Item = Item::Water;

    /// A second item, for the tests that need two chains or an intake nothing on the map supplies.
    const ANOTHER_ITEM: Item = Item::Hydrogen;

    /// How many ticks a fleet is given to do something before the test gives up on it.
    ///
    /// A tile is ten world units across and a straight road is driven at a sixty-fourth of one a
    /// tick, so a round trip along `HAULAGE` is a few hundred ticks and this is several of them.
    const TICKS_ALLOWED: u32 = 4096;

    /// How many ticks each fleet is driven for before what it delivered is compared.
    ///
    /// Long enough for a single rover to run several round trips, so what separates two fleets is
    /// how many rovers were carrying rather than where in a trip each happened to stop.
    const TICKS_MEASURED: u32 = 2048;

    /// How many round trips a single rover has to manage for the comparison to be worth making.
    ///
    /// Two fleets separated by where one of them happened to stop is a difference of one load; a
    /// run several trips long is one where the fleets are separated by how much they carried.
    const TRIPS_MEASURED: u32 = 4;

    /// How many ticks a frame carries when the world is run faster than it is drawn.
    ///
    /// The top rung of the speed ladder is four times real time against a frame rate that does
    /// not rise with it, so a frame carrying several ticks is what the game ordinarily does and
    /// not an edge of it. A fleet has to reach the same place however a frame divides its ticks.
    const TICKS_A_BUSY_FRAME: u32 = 4;

    /// How many ticks a fleet is left running to see that it settled where it was asked to.
    ///
    /// Long enough for a rover to be sent, routed and driven several times over, so a count that
    /// only looks right on the tick it was read has somewhere to go wrong before it is read again.
    const TICKS_A_FEW: u32 = 64;

    /// An outlet whose building keeps making what it hands out, until production is #26's.
    ///
    /// A bounded port is three rover-loads of slack and no more, so a measurement over hundreds of
    /// ticks needs something behind the door refilling it. This is the smallest stand-in for that.
    #[derive(Component)]
    struct Producing;

    /// An intake whose building consumes what arrives, keeping count of how much did.
    ///
    /// The tally is what makes throughput measurable now that a port cannot bank without limit:
    /// what a run delivered is what was consumed, not what happens to be standing at the door.
    #[derive(Component, Default)]
    struct Consuming(u32);

    fn keep_the_producing_ports_full(mut ports: Query<&mut Holding, With<Producing>>) {
        for mut holding in &mut ports {
            holding.take_in(PORT_CAPACITY);
        }
    }

    fn take_in_what_reached_the_consuming_ports(mut ports: Query<(&mut Holding, &mut Consuming)>) {
        for (mut holding, mut consuming) in &mut ports {
            consuming.0 += holding.give_out(PORT_CAPACITY);
        }
    }

    /// How many searches for a supplier have run over the ticks so far.
    ///
    /// A search is otherwise invisible: one that ran and settled on the producer it settled on
    /// last time leaves the world exactly as one that never ran. Writing the answer is what a
    /// search costs, so counting the fleets written to is what says whether a settled fleet is
    /// asking the network again on every tick of a run.
    #[derive(Resource, Default)]
    struct SearchesRun(u32);

    fn count_the_searches_that_ran(
        mut counted: ResMut<SearchesRun>,
        looked: Query<(), Changed<Fleet>>,
    ) {
        counted.0 += looked.iter().count() as u32;
    }

    fn searches_run(app: &App) -> u32 {
        app.world().resource::<SearchesRun>().0
    }

    fn fleet_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::Select)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                SimulationPlugin,
                DebugGizmosPlugin,
                CleanupPlugin,
                BuildingPlugin,
                RoadPlugin,
                RoverPlugin,
                FleetPlugin,
                SelectionPlugin,
            ))
            .init_resource::<SearchesRun>()
            .add_systems(
                FixedUpdate,
                (
                    keep_the_producing_ports_full,
                    take_in_what_reached_the_consuming_ports,
                )
                    .before(RoversDriven),
            )
            .add_systems(FixedUpdate, count_the_searches_that_ran.after(Simulation));
        app
    }

    fn tile(offset: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offset.0, offset.1)
    }

    /// The node a port on the tile at `offset` stands on, which a road runs through.
    fn node_at(offset: (i32, i32)) -> LatticeNode {
        PORT_CORNER.node_of(tile(offset))
    }

    /// Lay a road through the port corners of `offsets` and let it take its tiles.
    fn lay_road(app: &mut App, offsets: &[(i32, i32)]) {
        let nodes = offsets.iter().copied().map(node_at).collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
        tick(app);
    }

    /// Lay a road as `lay_road` does, on a frame carrying several ticks rather than one.
    fn lay_road_on_a_busy_frame(app: &mut App, offsets: &[(i32, i32)]) {
        let nodes = offsets.iter().copied().map(node_at).collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
        busy_frame(app);
    }

    /// Advance one frame carrying `TICKS_A_BUSY_FRAME` ticks, as a warped world does.
    fn busy_frame(app: &mut App) {
        let timestep = app.world().resource::<Time<Fixed>>().timestep();
        advance(app, timestep * TICKS_A_BUSY_FRAME);
    }

    /// The road the test laid, which is the only one on the map until it lays another.
    fn the_road(app: &mut App) -> Entity {
        let mut query = app.world_mut().query_filtered::<Entity, With<Road>>();
        query
            .iter(app.world())
            .next()
            .expect("the test laid a road")
    }

    /// Stand a port for `item` moving goods `flow` on the tile at `offset`, which a road running
    /// through its corner reaches.
    fn port_at(app: &mut App, offset: (i32, i32), flow: Flow, item: Item) -> Entity {
        app.world_mut()
            .spawn((
                Port { flow, item },
                Holding::default(),
                RoadEndpoint::at(node_at(offset)),
            ))
            .id()
    }

    /// Stand the door a fleet collects from on the tile at `offset`.
    fn outlet_at(app: &mut App, offset: (i32, i32)) -> Entity {
        port_at(app, offset, Flow::Outlet, HAULED)
    }

    /// Stand a door handing out `item` rather than the one the rest of the map is plumbed for.
    fn outlet_of(app: &mut App, offset: (i32, i32), item: Item) -> Entity {
        port_at(app, offset, Flow::Outlet, item)
    }

    /// Stand the door a fleet delivers to on the tile at `offset`.
    fn intake_at(app: &mut App, offset: (i32, i32)) -> Entity {
        port_at(app, offset, Flow::Intake, HAULED)
    }

    /// Stand a door taking in `item` rather than the one the rest of the map is plumbed for.
    fn intake_of(app: &mut App, offset: (i32, i32), item: Item) -> Entity {
        port_at(app, offset, Flow::Intake, item)
    }

    /// Give `port` a fleet of `rovers`, leaving the tick to find where they collect from.
    fn assign(app: &mut App, port: Entity, rovers: u32) {
        app.world_mut().entity_mut(port).insert(Fleet {
            rovers,
            source: None,
        });
    }

    /// The port the fleet on `port` was found to collect from.
    fn source_of(app: &App, port: Entity) -> Option<Entity> {
        app.world().entity(port).get::<Fleet>()?.source
    }

    /// Where the port a fleet collects from stands, which names it across two runs of a fixture.
    fn source_stands_on(app: &App, port: Entity) -> Option<LatticeNode> {
        let source = source_of(app, port)?;
        Some(
            app.world()
                .entity(source)
                .get::<RoadEndpoint>()?
                .standing_on(),
        )
    }

    /// Put `quantity` at `port` for a fleet to collect.
    fn stock(app: &mut App, port: Entity, quantity: u32) {
        app.world_mut()
            .entity_mut(port)
            .get_mut::<Holding>()
            .expect("a port holds stock")
            .take_in(quantity);
    }

    /// Have the building behind `port` keep making what it hands out, so a fleet never runs it dry.
    fn produce_at(app: &mut App, port: Entity) {
        app.world_mut().entity_mut(port).insert(Producing);
    }

    /// Have the building behind `port` consume what arrives, tallying it as it goes.
    fn consume_at(app: &mut App, port: Entity) {
        app.world_mut()
            .entity_mut(port)
            .insert(Consuming::default());
    }

    /// What every rover on the map that is carrying anything has on its back.
    fn carried_items(app: &mut App) -> Vec<Item> {
        let mut query = app.world_mut().query_filtered::<&Cargo, With<Rover>>();
        query.iter(app.world()).map(|load| load.item).collect()
    }

    /// Whether every loaded rover on the map is holding a route rather than an order for one.
    ///
    /// A route it already drove is what a jammed rover waits on. One that let go of it is one the
    /// network is asked to route again on every tick of the jam.
    fn every_waiting_rover_still_holds_its_route(app: &mut App) -> bool {
        let mut query = app
            .world_mut()
            .query_filtered::<Option<&Route>, (With<Rover>, With<Cargo>)>();
        query.iter(app.world()).all(|route| route.is_some())
    }

    /// How much the building behind `port` has taken in over the run so far.
    fn taken_in(app: &App, port: Entity) -> u32 {
        app.world()
            .entity(port)
            .get::<Consuming>()
            .map_or(0, |consuming| consuming.0)
    }

    fn held_at(app: &App, entity: Entity) -> u32 {
        let entity = app.world().entity(entity);
        let carried = entity.get::<Cargo>().map_or(0, |load| load.quantity);
        let stood = entity.get::<Holding>().map_or(0, Holding::held);
        carried + stood
    }

    /// Everything anything in the world is holding, standing at a port or on the back of a rover.
    fn held_anywhere(app: &mut App) -> u32 {
        let mut carried = app.world_mut().query::<&Cargo>();
        let on_the_road: u32 = carried.iter(app.world()).map(|load| load.quantity).sum();
        let mut stood = app.world_mut().query::<&Holding>();
        let at_the_doors: u32 = stood.iter(app.world()).map(Holding::held).sum();
        on_the_road + at_the_doors
    }

    fn rovers_serving(app: &mut App, port: Entity) -> usize {
        let mut query = app.world_mut().query::<&Serving>();
        query
            .iter(app.world())
            .filter(|serving| serving.port == port)
            .count()
    }

    /// Where `port` is served from, which is where a rover of its fleet stands when it is at home.
    fn served(app: &App, port: Entity) -> ServedBy {
        app.world()
            .entity(port)
            .get::<RoadEndpoint>()
            .and_then(RoadEndpoint::served_by)
            .expect("a road reaches the port")
    }

    /// Whether every rover on the map is standing at `place`.
    fn all_the_rovers_stand_at(app: &mut App, place: ServedBy) -> bool {
        let mut query = app.world_mut().query::<&Rover>();
        query
            .iter(app.world())
            .all(|rover| rover.segment == place.segment && rover.along == place.along)
    }

    /// Whether some rover on the map is standing at `place`.
    fn a_rover_stands_at(app: &mut App, place: ServedBy) -> bool {
        !no_rover_stands_at(app, place)
    }

    /// Whether no rover on the map is standing at `place`.
    fn no_rover_stands_at(app: &mut App, place: ServedBy) -> bool {
        let mut query = app.world_mut().query::<&Rover>();
        query
            .iter(app.world())
            .all(|rover| rover.segment != place.segment || rover.along != place.along)
    }

    fn run(app: &mut App, ticks: u32) {
        for _ in 0..ticks {
            tick(app);
        }
    }

    /// Run until `ready` says so, giving up after `TICKS_ALLOWED`.
    fn run_until(app: &mut App, mut ready: impl FnMut(&mut App) -> bool) -> bool {
        for _ in 0..TICKS_ALLOWED {
            if ready(app) {
                return true;
            }
            tick(app);
        }
        ready(app)
    }

    /// Run frames carrying several ticks each until `ready` says so, giving up after
    /// `TICKS_ALLOWED`.
    fn run_busy_until(app: &mut App, mut ready: impl FnMut(&mut App) -> bool) -> bool {
        for _ in 0..TICKS_ALLOWED {
            if ready(app) {
                return true;
            }
            busy_frame(app);
        }
        ready(app)
    }

    /// An app holding the haulage road, a stocked source and a port to serve.
    fn haulage_app() -> (App, Entity, Entity) {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        (app, source, home)
    }

    #[test]
    fn a_fleet_puts_as_many_rovers_on_the_road_as_it_was_given() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn a_fleet_on_a_port_no_road_reaches_puts_no_rovers_on_the_road() {
        let mut app = fleet_app();
        outlet_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        assign(&mut app, home, A_FLEET);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), 0);
    }

    #[test]
    fn a_rover_brings_a_load_from_the_source_to_the_port_it_serves() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, 1);

        let delivered = run_until(&mut app, |app| held_at(app, home) > 0);

        assert!(delivered, "no load ever reached the port");
        assert_eq!(held_at(&app, home), ROVER_LOAD);
        assert_eq!(held_at(&app, source), A_STOCK - ROVER_LOAD);
    }

    #[test]
    fn a_rover_that_has_handed_over_sets_off_for_another_load() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, 1);

        let twice = run_until(&mut app, |app| held_at(app, home) >= 2 * ROVER_LOAD);

        assert!(twice, "the rover delivered once and stopped");
    }

    #[test]
    fn a_rover_carries_the_item_of_the_outlet_it_collected_from() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_of(&mut app, SOURCE, ANOTHER_ITEM);
        let home = intake_of(&mut app, HOME, ANOTHER_ITEM);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);

        let loaded = run_until(&mut app, |app| !carried_items(app).is_empty());

        assert!(loaded, "the rover never took anything on");
        assert_eq!(carried_items(&mut app), vec![ANOTHER_ITEM]);
    }

    #[test]
    fn a_fleet_collects_from_the_port_making_what_the_intake_it_serves_takes() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET);

        tick(&mut app);

        assert_eq!(source_of(&app, home), Some(source));
    }

    #[test]
    fn a_fleet_finds_no_supplier_where_nothing_makes_what_it_takes() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_of(&mut app, SOURCE, ANOTHER_ITEM);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, A_FLEET);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(source_of(&app, home), None);
        assert_eq!(
            held_at(&app, home),
            0,
            "a port took in an item it is no door for"
        );
        assert_eq!(held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn a_fleet_passes_over_a_producer_no_road_reaches() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let reached = outlet_at(&mut app, SOURCE);
        let stranded = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, stranded, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, A_FLEET);

        tick(&mut app);

        assert_eq!(source_of(&app, home), Some(reached));
    }

    #[test]
    fn a_fleet_collects_from_the_producer_that_is_quicker_by_road_than_nearer_on_the_grid() {
        let mut app = fleet_app();
        lay_road(&mut app, &THE_LONG_WAY_ROUND);
        let quick = outlet_at(&mut app, QUICK_BY_ROAD);
        let near = outlet_at(&mut app, NEAR_ON_THE_GRID);
        let home = intake_at(&mut app, BEND_HOME);
        stock(&mut app, quick, A_STOCK);
        stock(&mut app, near, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);

        tick(&mut app);

        assert_eq!(
            source_of(&app, home),
            Some(quick),
            "the fleet took the producer nearer across the grid over the one nearer by road"
        );
    }

    #[test]
    fn the_same_map_finds_the_same_producer_however_its_ports_were_spawned() {
        let one_way_round = {
            let mut app = fleet_app();
            lay_road(&mut app, &THE_LONG_WAY_ROUND);
            outlet_at(&mut app, QUICK_BY_ROAD);
            outlet_at(&mut app, NEAR_ON_THE_GRID);
            let home = intake_at(&mut app, BEND_HOME);
            tick(&mut app);
            assign(&mut app, home, 1);
            tick(&mut app);
            source_stands_on(&app, home)
        };
        let the_other = {
            let mut app = fleet_app();
            lay_road(&mut app, &THE_LONG_WAY_ROUND);
            let home = intake_at(&mut app, BEND_HOME);
            outlet_at(&mut app, NEAR_ON_THE_GRID);
            outlet_at(&mut app, QUICK_BY_ROAD);
            tick(&mut app);
            assign(&mut app, home, 1);
            tick(&mut app);
            source_stands_on(&app, home)
        };

        assert!(one_way_round.is_some(), "neither run found a producer");
        assert_eq!(
            one_way_round, the_other,
            "the order the ports were spawned in settled which one a fleet collects from"
        );
    }

    #[test]
    fn a_fleet_finds_a_producer_built_after_it_was_given_its_rovers() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let home = intake_at(&mut app, HOME);
        tick(&mut app);
        assign(&mut app, home, 1);
        tick(&mut app);
        assert_eq!(source_of(&app, home), None);

        let source = outlet_at(&mut app, SOURCE);
        stock(&mut app, source, A_STOCK);
        let delivered = run_until(&mut app, |app| held_at(app, home) > 0);

        assert_eq!(source_of(&app, home), Some(source));
        assert!(delivered, "the fleet never collected from the new producer");
    }

    #[test]
    fn a_fleet_given_a_quicker_producer_keeps_the_rovers_it_has() {
        let mut app = fleet_app();
        lay_road(&mut app, &THE_LONG_WAY_ROUND);
        let round_the_houses = outlet_at(&mut app, NEAR_ON_THE_GRID);
        let home = intake_at(&mut app, BEND_HOME);
        tick(&mut app);
        assign(&mut app, home, A_FLEET);
        tick(&mut app);
        assert_eq!(source_of(&app, home), Some(round_the_houses));
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        let quicker = outlet_at(&mut app, QUICK_BY_ROAD);
        tick(&mut app);

        assert_eq!(source_of(&app, home), Some(quicker));
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn a_settled_fleet_looks_no_further_while_the_world_stands_still() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        run(&mut app, TICKS_A_FEW);
        let settled = searches_run(&app);

        run(&mut app, TICKS_A_FEW);

        assert_eq!(
            searches_run(&app),
            settled,
            "a settled fleet asked the network again over {TICKS_A_FEW} quiet ticks"
        );
    }

    #[test]
    fn a_road_laid_has_every_fleet_look_again() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        run(&mut app, TICKS_A_FEW);
        let settled = searches_run(&app);

        lay_road(&mut app, &BRANCH);

        assert!(
            searches_run(&app) > settled,
            "a fleet took no notice of a road laid across the map"
        );
    }

    #[test]
    fn a_rover_collecting_from_an_empty_outlet_takes_on_nothing() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        tick(&mut app);
        assign(&mut app, home, 1);

        let reached = run_until(&mut app, |app| {
            let standing = served(app, source);
            a_rover_stands_at(app, standing)
        });

        assert!(reached, "the rover never reached the source");
        run(&mut app, TICKS_A_FEW);
        assert_eq!(
            held_anywhere(&mut app),
            0,
            "a load came out of an empty port"
        );
    }

    #[test]
    fn a_fleet_leaves_alone_what_was_delivered_to_another_intake() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, SOURCE);
        let delivered_to = intake_at(&mut app, OTHER_HOME);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        stock(&mut app, delivered_to, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, A_FLEET);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(
            held_at(&app, delivered_to),
            A_STOCK,
            "a rover drew on what was delivered to an intake"
        );
    }

    #[test]
    fn a_rover_that_cannot_hand_over_waits_at_the_door_it_serves() {
        let (mut app, source, home) = haulage_app();
        produce_at(&mut app, source);
        stock(&mut app, home, PORT_CAPACITY);
        assign(&mut app, home, 1);

        let back = run_until(&mut app, |app| {
            let standing = served(app, home);
            a_rover_stands_at(app, standing) && !carried_items(app).is_empty()
        });

        assert!(back, "no loaded rover ever got back to the port it serves");
        run(&mut app, TICKS_A_FEW);
        assert_eq!(held_at(&app, home), PORT_CAPACITY);
        assert_eq!(
            carried_items(&mut app),
            vec![HAULED],
            "the rover it could not unload gave up its load anyway"
        );
    }

    #[test]
    fn a_rover_held_at_a_full_door_keeps_the_route_it_drove_rather_than_asking_for_another() {
        let (mut app, source, home) = haulage_app();
        produce_at(&mut app, source);
        stock(&mut app, home, PORT_CAPACITY);
        assign(&mut app, home, 1);

        let back = run_until(&mut app, |app| {
            let standing = served(app, home);
            a_rover_stands_at(app, standing) && !carried_items(app).is_empty()
        });
        assert!(back, "no loaded rover ever got back to the port it serves");
        run(&mut app, TICKS_A_FEW);

        assert!(
            every_waiting_rover_still_holds_its_route(&mut app),
            "a rover jammed at a full door asks for a route every tick it waits"
        );
    }

    #[test]
    fn a_chain_run_for_a_stretch_of_ticks_loses_nothing_and_invents_nothing() {
        let (mut app, _, home) = haulage_app();
        consume_at(&mut app, home);
        assign(&mut app, home, A_FLEET);

        run(&mut app, TICKS_MEASURED);

        assert!(taken_in(&app, home) > 0, "nothing ever reached the port");
        assert_eq!(taken_in(&app, home) + held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn two_fleets_on_one_road_do_not_draw_on_each_others_rovers() {
        let (mut app, _, home) = haulage_app();
        let other = intake_at(&mut app, OTHER_HOME);
        tick(&mut app);
        assign(&mut app, home, A_FLEET);
        assign(&mut app, other, 1);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
        assert_eq!(rovers_serving(&mut app, other), 1);
    }

    #[test]
    fn raising_a_count_puts_another_rover_on_the_road_on_that_tick() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, 1);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), 1);

        assign(&mut app, home, A_FLEET);
        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn lowering_a_count_leaves_a_rover_that_is_away_on_the_road() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        let place = {
            tick(&mut app);
            served(&app, home)
        };
        let away = run_until(&mut app, |app| no_rover_stands_at(app, place));
        assert!(away, "the fleet never left the port it serves");

        assign(&mut app, home, 0);
        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn lowering_a_count_takes_the_rover_off_when_it_gets_back() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        tick(&mut app);

        assign(&mut app, home, 0);
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);

        assert!(gone, "a rover taken off never left the world");
    }

    #[test]
    fn a_fleet_asked_for_a_rover_back_runs_the_count_it_was_given() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        assign(&mut app, home, 1);
        let given_up = run_until(&mut app, |app| rovers_serving(app, home) == 1);
        assert!(given_up, "the fleet never gave up the rover it lost");

        assign(&mut app, home, A_FLEET);
        let back = run_busy_until(&mut app, |app| {
            rovers_serving(app, home) == A_FLEET as usize
        });

        assert!(back, "the fleet never made the rover it gave up back");
        run(&mut app, TICKS_A_FEW);
        assert_eq!(
            rovers_serving(&mut app, home),
            A_FLEET as usize,
            "the fleet ran a different number of rovers than it was given"
        );
    }

    #[test]
    fn a_rover_taken_off_hands_over_what_it_was_carrying_first() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        let carrying = run_until(&mut app, |app| held_at(app, source) < A_STOCK);
        assert!(carrying, "the fleet never took anything on");

        assign(&mut app, home, 0);
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);

        assert!(gone, "a rover taken off never left the world");
        assert!(held_at(&app, home) > 0, "nothing ever reached the port");
        assert_eq!(held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn a_fleet_with_no_producer_it_can_reach_keeps_its_rovers_at_the_port() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);
        tick(&mut app);
        let place = served(&app, home);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(rovers_serving(&mut app, home), 1);
        assert!(
            all_the_rovers_stand_at(&mut app, place),
            "a rover went out to strand rather than staying at the port"
        );
    }

    #[test]
    fn a_fleet_sets_off_once_a_road_reaches_a_producer() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);
        run(&mut app, TICKS_MEASURED);
        assert_eq!(held_at(&app, home), 0);

        lay_road(&mut app, &BRANCH);
        let delivered = run_until(&mut app, |app| held_at(app, home) > 0);

        assert!(delivered, "the fleet stayed put after a road reached it");
    }

    #[test]
    fn a_fleet_sets_off_once_a_road_reaches_a_producer_however_a_frame_divides() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);
        run(&mut app, TICKS_MEASURED);
        assert_eq!(held_at(&app, home), 0);

        lay_road_on_a_busy_frame(&mut app, &BRANCH);
        let delivered = run_busy_until(&mut app, |app| held_at(app, home) > 0);

        assert!(
            delivered,
            "a frame carrying several ticks parked the fleet for good after a road reached it"
        );
    }

    #[test]
    fn a_rover_takes_on_no_load_from_a_port_it_is_not_standing_at() {
        let mut app = fleet_app();
        lay_road(&mut app, &THE_LONG_WAY_ROUND);
        let empty = outlet_at(&mut app, QUICK_BY_ROAD);
        let stocked = outlet_at(&mut app, NEAR_ON_THE_GRID);
        let home = intake_at(&mut app, BEND_HOME);
        stock(&mut app, stocked, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1);
        let waiting = run_until(&mut app, |app| {
            let standing = served(app, empty);
            a_rover_stands_at(app, standing)
        });
        assert!(
            waiting,
            "the rover never reached the producer it was sent to"
        );

        run(&mut app, TICKS_A_FEW);

        assert_eq!(
            held_at(&app, stocked),
            A_STOCK,
            "a load left a port with no rover standing at it"
        );
    }

    #[test]
    fn a_fleet_puts_out_again_the_rovers_a_bulldozed_road_took_with_it() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        let road = the_road(&mut app);
        app.world_mut().entity_mut(road).despawn();
        let taken = run_until(&mut app, |app| rovers_serving(app, home) == 0);
        assert!(taken, "the bulldozed road left its rovers in the world");

        lay_road(&mut app, &HAULAGE);
        let back = run_until(&mut app, |app| {
            rovers_serving(app, home) == A_FLEET as usize
        });

        assert!(
            back,
            "the fleet never made up the rovers the bulldozed road took with it"
        );
    }

    #[test]
    fn a_port_given_a_fleet_again_puts_its_rovers_back_on_the_road() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        app.world_mut().entity_mut(home).remove::<Fleet>();
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);
        assert!(gone, "a rover outlived the fleet it belonged to");

        assign(&mut app, home, A_FLEET);
        let back = run_until(&mut app, |app| {
            rovers_serving(app, home) == A_FLEET as usize
        });

        assert!(
            back,
            "a port given a fleet again never put its rovers on the road"
        );
    }

    #[test]
    fn taking_a_port_off_the_map_takes_its_fleet_off_the_road() {
        let (mut app, _, home) = haulage_app();
        assign(&mut app, home, A_FLEET);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        app.world_mut().entity_mut(home).insert(Destroy);
        let gone = run_until(&mut app, |app| {
            let mut query = app.world_mut().query::<&Serving>();
            query.iter(app.world()).count() == 0
        });

        assert!(gone, "a rover outlived the port it served");
    }

    /// How much a fleet of `A_FLEET` delivers down `HAULAGE` over `TICKS_MEASURED`.
    ///
    /// Measured, not reasoned about (2.3). The same fixture delivered 72 before a rover carried a
    /// speed from one tick to the next: braking into the port it was sent to and pulling away from
    /// it again costs a rover the run those two take at full speed, twice a round trip, and this
    /// road is two tiles long so it pays that over forty world units rather than four hundred.
    /// What a longer haul loses to the same rate is a smaller share of it.
    ///
    /// It is written down so that the next thing to move it has to say so.
    const A_MEASURED_DELIVERY: u32 = 48;

    #[test]
    fn a_shuttle_delivers_what_a_measured_run_says_it_does() {
        let (mut app, source, home) = haulage_app();
        produce_at(&mut app, source);
        consume_at(&mut app, home);
        assign(&mut app, home, A_FLEET);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(taken_in(&app, home), A_MEASURED_DELIVERY);
    }

    #[test]
    fn two_rovers_deliver_more_over_the_same_ticks_than_one() {
        let delivered = |rovers: u32| {
            let (mut app, source, home) = haulage_app();
            produce_at(&mut app, source);
            consume_at(&mut app, home);
            assign(&mut app, home, rovers);
            run(&mut app, TICKS_MEASURED);
            taken_in(&app, home)
        };

        let one = delivered(1);
        let two = delivered(A_FLEET);

        assert!(
            one >= TRIPS_MEASURED * ROVER_LOAD,
            "one rover delivered {one} over {TICKS_MEASURED} ticks, which is short of the round \
             trips the comparison is made over"
        );
        assert!(
            two > one,
            "two rovers delivered {two} where one delivered {one}"
        );
    }

    /// The tile the building the player assigns stands on, in offset-row coordinates.
    const ASSIGNED: (i32, i32) = (0, 0);

    /// The tile the building it is pointed at stands on, in offset-row coordinates.
    const COLLECTED: (i32, i32) = (3, 0);

    /// How far through the catalogue the first assembler sits: one item in, one out.
    ///
    /// An extractor takes nothing in, so it stands no intake, and an intake is what takes a fleet.
    const MELTER: isize = 5;

    /// The corner a melter's intake stands on, which is `INTAKE_CORNERS[0]` unturned.
    const INTAKE_CORNER: TileCorner = TileCorner::SouthWest;

    /// The corner an extractor's outlet stands on, which is `OUTLET_CORNERS[0]` unturned.
    const OUTLET_CORNER: TileCorner = TileCorner::North;

    /// The tiles the road serving the assigned building's intake runs over.
    const REACHING: [(i32, i32); 2] = [(0, 0), (1, 0)];

    /// The tile a second building competing for the same rovers stands on, in offset-row
    /// coordinates. Far enough off `REACHING` that no road reaches it and no corner is shared.
    const ALSO_ASSIGNED: (i32, i32) = (0, 2);

    /// A pool one rover spends outright, so what any second port asks for is asked of nothing.
    const A_POOL_OF_ONE: u32 = 1;

    fn hold(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
        tick(app);
    }

    /// Lay a road through the `corner` of each of `offsets`, and let it take its tiles.
    fn lay_road_through(app: &mut App, corner: TileCorner, offsets: &[(i32, i32)]) {
        let nodes = offsets
            .iter()
            .map(|&offset| corner.node_of(tile(offset)))
            .collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
        tick(app);
    }

    /// Click over `tile` at `point`, then let the button go, so one frame is not two clicks.
    fn click_at(app: &mut App, tile: Entity, point: Vec3) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = Some(tile);
            input.world_cursor_position = Some(point);
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    /// Lay under `tile` the ground the type the tool is holding needs, an extractor standing
    /// nowhere but a deposit of what it draws.
    fn ground_for_the_chosen_type(app: &mut App, tile: Entity) {
        let BuildingType::Extractor(material) =
            app.world().resource::<ChosenBuildingType>().chosen()
        else {
            return;
        };
        app.world_mut().entity_mut(tile).insert(Deposit {
            material,
            richness: 1,
        });
    }

    /// Put the `steps`th type of the catalogue on `offsets`, answering with it and its tile.
    fn place(app: &mut App, offsets: (i32, i32), steps: isize) -> (Entity, Entity) {
        app.world_mut()
            .resource_mut::<ChosenBuildingType>()
            .step(steps);
        let ground = app
            .world_mut()
            .spawn(MapTile {
                coordinates: tile(offsets),
            })
            .id();
        ground_for_the_chosen_type(app, ground);
        click_at(app, ground, tile(offsets).world_position());
        app.world_mut()
            .resource_mut::<ChosenBuildingType>()
            .step(-steps);
        let building = app
            .world()
            .resource::<BuildingTiles>()
            .building_on(tile(offsets))
            .expect("a building stands there");
        (building, ground)
    }

    /// The port of `building` standing on `corner` of the tile at `offsets`.
    fn port_on(app: &App, building: Entity, corner: TileCorner, offsets: (i32, i32)) -> Entity {
        let node = corner.node_of(tile(offsets));
        app.world()
            .get::<Children>(building)
            .expect("a building has its ports")
            .iter()
            .find(|port| {
                app.world()
                    .get::<RoadEndpoint>(*port)
                    .is_some_and(|endpoint| endpoint.standing_on() == node)
            })
            .expect("a port stands on that corner")
    }

    /// Press and release `key`, so one press is not read on every frame after it.
    fn tap_key(app: &mut App, key: KeyCode) {
        press_key(app, key);
        tick(app);
        release_key(app, key);
        tick(app);
    }

    /// A melter to assign, an extractor to supply it, and a road reaching the melter's intake.
    ///
    /// Answers with the app, the melter's tile and its intake.
    fn assignment_app() -> (App, Entity, Entity) {
        let mut app = fleet_app();
        hold(&mut app, PlayerAction::EditBuildings);
        let (melter, ground) = place(&mut app, ASSIGNED, MELTER);
        place(&mut app, COLLECTED, 0);
        lay_road_through(&mut app, INTAKE_CORNER, &REACHING);
        hold(&mut app, PlayerAction::Select);

        let intake = port_on(&app, melter, INTAKE_CORNER, ASSIGNED);
        (app, ground, intake)
    }

    /// Pick out `corner` of the tile at `offsets`, which is a left click over that corner.
    fn pick_out(app: &mut App, ground: Entity, offsets: (i32, i32), corner: TileCorner) {
        click_at(app, ground, corner.node_of(tile(offsets)).world_position());
    }

    fn fleet_of(app: &App, port: Entity) -> Option<&Fleet> {
        app.world().entity(port).get::<Fleet>()
    }

    fn rovers_of(app: &App, port: Entity) -> u32 {
        fleet_of(app, port)
            .expect("the port was given a fleet")
            .rovers
    }

    /// Give the map a pool of `size` rovers, in place of the one the game ships with.
    fn pool_of(app: &mut App, size: u32) {
        app.insert_resource(RoverPool { size });
    }

    /// Stand a second melter on `ALSO_ASSIGNED`, answering with its intake and its tile.
    fn a_second_melter(app: &mut App) -> (Entity, Entity) {
        hold(app, PlayerAction::EditBuildings);
        let (melter, ground) = place(app, ALSO_ASSIGNED, MELTER);
        hold(app, PlayerAction::Select);
        (port_on(app, melter, INTAKE_CORNER, ALSO_ASSIGNED), ground)
    }

    #[test]
    fn asking_an_intake_for_a_rover_gives_it_a_fleet() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);

        tap_key(&mut app, FLEET_KEYS[1].0);

        let fleet = fleet_of(&app, intake).expect("the intake was given a fleet");
        assert_eq!(fleet.rovers, 1);
    }

    #[test]
    fn an_intake_gains_a_fleet_for_a_command_nobody_pressed_a_key_for() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);

        ask_for(&mut app, FLEET_KEYS[1].1);
        tick(&mut app);

        let fleet = fleet_of(&app, intake).expect("the intake was given a fleet");
        assert_eq!(fleet.rovers, 1);
    }

    #[test]
    fn asking_an_intake_for_a_rover_back_before_it_has_one_gives_it_no_fleet() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);

        tap_key(&mut app, FLEET_KEYS[0].0);

        assert!(fleet_of(&app, intake).is_none());
    }

    #[test]
    fn an_outlet_takes_no_fleet() {
        let (mut app, ground, _) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, OUTLET_CORNER);

        tap_key(&mut app, FLEET_KEYS[1].0);

        let melter = app
            .world()
            .resource::<BuildingTiles>()
            .building_on(tile(ASSIGNED))
            .expect("the melter stands there");
        let outlet = port_on(&app, melter, OUTLET_CORNER, ASSIGNED);
        assert!(fleet_of(&app, outlet).is_none());
    }

    #[test]
    fn raising_the_count_puts_another_rover_on_the_road() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);
        assign(&mut app, intake, 1);
        assert!(run_until(&mut app, |app| rovers_serving(app, intake) == 1));

        tap_key(&mut app, FLEET_KEYS[1].0);

        assert!(run_until(&mut app, |app| rovers_serving(app, intake) == 2));
    }

    #[test]
    fn lowering_the_count_takes_a_rover_off_the_road() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);
        assign(&mut app, intake, A_FLEET);
        assert!(run_until(&mut app, |app| rovers_serving(app, intake)
            == A_FLEET as usize));

        tap_key(&mut app, FLEET_KEYS[0].0);

        assert!(run_until(&mut app, |app| rovers_serving(app, intake)
            == A_FLEET as usize - 1));
    }

    #[test]
    fn the_count_does_not_fall_below_none() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);
        assign(&mut app, intake, 0);

        tap_key(&mut app, FLEET_KEYS[0].0);

        assert_eq!(
            fleet_of(&app, intake)
                .expect("the intake keeps its fleet")
                .rovers,
            0
        );
    }

    #[test]
    fn a_key_with_another_tool_held_assigns_nothing() {
        let (mut app, ground, intake) = assignment_app();
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);
        hold(&mut app, PlayerAction::EditRoads);

        tap_key(&mut app, FLEET_KEYS[1].0);

        assert!(fleet_of(&app, intake).is_none());
    }

    #[test]
    fn a_port_is_refused_a_rover_the_pool_cannot_supply() {
        let (mut app, ground, intake) = assignment_app();
        pool_of(&mut app, A_POOL_OF_ONE);
        assign(&mut app, intake, A_POOL_OF_ONE);
        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);

        tap_key(&mut app, FLEET_KEYS[1].0);

        assert_eq!(rovers_of(&app, intake), A_POOL_OF_ONE);
    }

    #[test]
    fn an_intake_asking_for_its_first_rover_is_refused_when_the_pool_is_spent() {
        let (mut app, _, intake) = assignment_app();
        pool_of(&mut app, A_POOL_OF_ONE);
        assign(&mut app, intake, A_POOL_OF_ONE);
        let (other, other_ground) = a_second_melter(&mut app);
        pick_out(&mut app, other_ground, ALSO_ASSIGNED, INTAKE_CORNER);

        tap_key(&mut app, FLEET_KEYS[1].0);

        assert!(
            fleet_of(&app, other).is_none(),
            "a port with no fleet took a rover the pool did not have"
        );
    }

    #[test]
    fn a_rover_taken_off_one_port_can_be_given_to_another() {
        let (mut app, ground, intake) = assignment_app();
        pool_of(&mut app, A_POOL_OF_ONE);
        let (other, other_ground) = a_second_melter(&mut app);
        assign(&mut app, intake, A_POOL_OF_ONE);
        assign(&mut app, other, 0);
        pick_out(&mut app, other_ground, ALSO_ASSIGNED, INTAKE_CORNER);
        tap_key(&mut app, FLEET_KEYS[1].0);
        assert_eq!(rovers_of(&app, other), 0, "the pool was spent already");

        pick_out(&mut app, ground, ASSIGNED, INTAKE_CORNER);
        tap_key(&mut app, FLEET_KEYS[0].0);
        pick_out(&mut app, other_ground, ALSO_ASSIGNED, INTAKE_CORNER);
        tap_key(&mut app, FLEET_KEYS[1].0);

        assert_eq!(rovers_of(&app, other), A_POOL_OF_ONE);
    }

    #[test]
    fn taking_a_port_off_the_map_gives_its_rovers_back_to_the_pool() {
        let (mut app, _, intake) = assignment_app();
        pool_of(&mut app, A_POOL_OF_ONE);
        let (other, other_ground) = a_second_melter(&mut app);
        assign(&mut app, intake, A_POOL_OF_ONE);
        assign(&mut app, other, 0);
        pick_out(&mut app, other_ground, ALSO_ASSIGNED, INTAKE_CORNER);
        tap_key(&mut app, FLEET_KEYS[1].0);
        assert_eq!(rovers_of(&app, other), 0, "the pool was spent already");

        app.world_mut().entity_mut(intake).insert(Destroy);
        tick(&mut app);
        tap_key(&mut app, FLEET_KEYS[1].0);

        assert_eq!(rovers_of(&app, other), A_POOL_OF_ONE);
    }

    #[test]
    fn a_fleet_too_large_for_its_road_delivers_less_than_one_that_fits() {
        let delivered = |rovers: u32| {
            let (mut app, source, home) = haulage_app();
            produce_at(&mut app, source);
            consume_at(&mut app, home);
            assign(&mut app, home, rovers);
            run(&mut app, TICKS_MEASURED);
            taken_in(&app, home)
        };

        let fits = delivered(A_FLEET_THAT_FITS);
        let crowded = delivered(A_FLEET_TOO_LARGE_FOR_ITS_ROAD);

        assert!(
            crowded < fits,
            "{A_FLEET_TOO_LARGE_FOR_ITS_ROAD} rovers delivered {crowded} over \
             {TICKS_MEASURED} ticks where {A_FLEET_THAT_FITS} delivered {fits}"
        );
    }
}
