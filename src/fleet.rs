//! The rovers a port has been given, and the shuttle each of them runs.
//!
//! This is where a rover stops being something a test spawns and becomes something a building
//! has. An input port is given a number of rovers and a port to collect from, and every rover
//! assigned to it drives to that source, takes on a load, drives back, hands it over and sets
//! off again. The lever is how many rovers serve an input, never which road they take: routing
//! is [`crate::road`]'s answer and the road itself is the player's, so the only way to move more
//! is to put more rovers on the road they built and live with what that does to it.
//!
//! The source is named by the player rather than found by the rover, the finding being #132's
//! and needing recipes this comes before. What crosses between the two ends is
//! [`crate::rover::Cargo`], carrying the item of the outlet it was collected from.

use crate::building::{Flow, Holding, Port};
use crate::common::cleanup::Destroy;
use crate::diagnostics::DebugGizmos;
use crate::road::{RoadEndpoint, RoadTiles};
use crate::rover::{Cargo, Route, Rover, RoversDriven, SentTo, Stranded};
use crate::simulation::Simulation;
use bevy::prelude::*;

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

/// The rovers each port has been given, and the shuttle they run.
pub struct FleetPlugin;

/// The rovers a port has been given, and the port they collect from.
///
/// The count is the port's own record rather than a fact about a frame, which is what leaves a
/// player free to write it on the frame their click arrives on (invariant 2): the next tick reads
/// the number they asked for and puts that many rovers on the road. Whatever carries this needs a
/// [`RoadEndpoint`] for a rover to stand at, and a fleet on a port no road reaches is idle rather
/// than illegal — it starts running when a road arrives.
#[derive(Component)]
#[require(OnTheRoad)]
pub struct Fleet {
    /// How many rovers serve this port.
    pub rovers: u32,
    /// The port they collect their load from.
    pub source: Entity,
}

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

impl Plugin for FleetPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(take_the_rovers_of_a_fleet_that_is_gone_off_the_road)
            .add_observer(give_back_the_place_of_a_rover_that_left_the_world)
            .add_systems(
                FixedUpdate,
                (
                    turn_the_rovers_round_at_the_port_they_reached,
                    retire_the_rovers_a_fleet_no_longer_wants,
                    put_the_rovers_a_fleet_is_owed_on_the_road,
                    let_the_parked_rovers_try_again_when_the_roads_change,
                    set_the_idle_rovers_off_again,
                )
                    .chain()
                    .after(RoversDriven)
                    .in_set(Simulation),
            )
            .add_systems(Update, draw_the_way_a_fleet_collects_along);
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
        let collecting =
            !carrying && route.destination == fleet.source && fleet.source != serving.port;
        if !collecting {
            commands.entity(entity).remove::<Route>();
            continue;
        }

        let Ok((_, source, mut stood)) = ports.get_mut(fleet.source) else {
            continue;
        };
        if source.flow != Flow::Outlet {
            continue;
        }
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
                },
                Serving { port },
            ));
            out.0 += 1;
        }
    }
}

/// Offer every parked rover another go at reaching its source, once the roads have changed.
///
/// A standing assignment outlives the road it was made over, so a fleet whose source is bulldozed
/// away has to set off again when one comes back rather than staying parked for good. What it
/// must not do is pay for that with a search a tick: `RoadTiles` moves when a road is laid and
/// when one is removed and at no other time, which makes it the whole of the question. Reading
/// that it changed is a fact about ticks rather than frames — it says the roads moved since this
/// last ran, so a tick cannot miss the change and two ticks in one frame cannot both take it.
fn let_the_parked_rovers_try_again_when_the_roads_change(
    mut commands: Commands,
    roads: Res<RoadTiles>,
    parked: Query<Entity, (With<Serving>, With<Stranded>)>,
) {
    if !roads.is_changed() {
        return;
    }
    for entity in &parked {
        commands.entity(entity).remove::<Stranded>();
    }
}

/// Send every rover standing idle on to wherever the next leg of its shuttle takes it.
///
/// What it is carrying is what decides: an empty rover is going for a load and a loaded one is
/// bringing it home. A rover that cannot be routed is left standing where it is rather than sent
/// out to run out of road — which, since a shuttle is only ever sent from a port, is the port it
/// serves or the one it came to collect from.
fn set_the_idle_rovers_off_again(
    mut commands: Commands,
    fleets: Query<&Fleet>,
    idle: Query<(Entity, &Serving, Has<Cargo>), StandingIdle>,
) {
    for (entity, serving, carrying) in &idle {
        let Ok(fleet) = fleets.get(serving.port) else {
            continue;
        };
        let bound_for = if carrying { serving.port } else { fleet.source };
        commands.entity(entity).insert(SentTo(bound_for));
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
        let Ok(source) = endpoints.get(fleet.source) else {
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
    use crate::building::{BuildingPlugin, Item, PORT_CAPACITY};
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode, TileCorner};
    use crate::road::{Road, RoadPlugin, ServedBy};
    use crate::rover::RoverPlugin;
    use crate::simulation::SimulationPlugin;
    use crate::testing::{advance, headless_app, tick};

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

    /// How many rovers a fleet under test is given.
    const A_FLEET: u32 = 2;

    /// How much a source under test is stocked with, which is everything a port will hold.
    ///
    /// A finite figure now that a port is bounded, and the whole of what the world holds in the
    /// tests that count what came out the other end.
    const A_STOCK: u32 = PORT_CAPACITY;

    /// The one item these tests haul, every port standing here being a door for it.
    const HAULED: Item = Item::Water;

    /// An item no port standing in these tests is a door for, so nothing takes a delivery of it.
    const MISPLUMBED: Item = Item::Hydrogen;

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
            ))
            .add_systems(
                FixedUpdate,
                (
                    keep_the_producing_ports_full,
                    take_in_what_reached_the_consuming_ports,
                )
                    .before(RoversDriven),
            );
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

    /// Give `port` a fleet of `rovers` collecting from `source`.
    fn assign(app: &mut App, port: Entity, rovers: u32, source: Entity) {
        app.world_mut()
            .entity_mut(port)
            .insert(Fleet { rovers, source });
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
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn a_fleet_on_a_port_no_road_reaches_puts_no_rovers_on_the_road() {
        let mut app = fleet_app();
        let source = outlet_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        assign(&mut app, home, A_FLEET, source);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), 0);
    }

    #[test]
    fn a_rover_brings_a_load_from_the_source_to_the_port_it_serves() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, 1, source);

        let delivered = run_until(&mut app, |app| held_at(app, home) > 0);

        assert!(delivered, "no load ever reached the port");
        assert_eq!(held_at(&app, home), ROVER_LOAD);
        assert_eq!(held_at(&app, source), A_STOCK - ROVER_LOAD);
    }

    #[test]
    fn a_rover_that_has_handed_over_sets_off_for_another_load() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, 1, source);

        let twice = run_until(&mut app, |app| held_at(app, home) >= 2 * ROVER_LOAD);

        assert!(twice, "the rover delivered once and stopped");
    }

    #[test]
    fn a_rover_carries_the_item_of_the_outlet_it_collected_from() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_of(&mut app, SOURCE, MISPLUMBED);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1, source);

        let loaded = run_until(&mut app, |app| !carried_items(app).is_empty());

        assert!(loaded, "the rover never took anything on");
        assert_eq!(carried_items(&mut app), vec![MISPLUMBED]);
    }

    #[test]
    fn a_fleet_pointed_at_an_outlet_of_another_item_delivers_nothing() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_of(&mut app, SOURCE, MISPLUMBED);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, A_FLEET, source);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(
            held_at(&app, home),
            0,
            "a port took in an item it is no door for"
        );
        assert_eq!(held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn a_rover_collecting_from_an_empty_outlet_takes_on_nothing() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        tick(&mut app);
        assign(&mut app, home, 1, source);

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
    fn a_fleet_collecting_from_an_intake_takes_nothing_out_of_it() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = intake_at(&mut app, SOURCE);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, A_FLEET, source);

        run(&mut app, TICKS_MEASURED);

        assert_eq!(
            held_at(&app, source),
            A_STOCK,
            "a rover drew on what was delivered to an intake"
        );
    }

    #[test]
    fn a_rover_that_cannot_hand_over_waits_at_the_door_it_serves() {
        let (mut app, source, home) = haulage_app();
        produce_at(&mut app, source);
        stock(&mut app, home, PORT_CAPACITY);
        assign(&mut app, home, 1, source);

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
        assign(&mut app, home, 1, source);

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
        let (mut app, source, home) = haulage_app();
        consume_at(&mut app, home);
        assign(&mut app, home, A_FLEET, source);

        run(&mut app, TICKS_MEASURED);

        assert!(taken_in(&app, home) > 0, "nothing ever reached the port");
        assert_eq!(taken_in(&app, home) + held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn two_fleets_on_one_road_do_not_draw_on_each_others_rovers() {
        let (mut app, source, home) = haulage_app();
        let other = intake_at(&mut app, OTHER_HOME);
        tick(&mut app);
        assign(&mut app, home, A_FLEET, source);
        assign(&mut app, other, 1, source);

        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
        assert_eq!(rovers_serving(&mut app, other), 1);
    }

    #[test]
    fn raising_a_count_puts_another_rover_on_the_road_on_that_tick() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, 1, source);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), 1);

        assign(&mut app, home, A_FLEET, source);
        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn lowering_a_count_leaves_a_rover_that_is_away_on_the_road() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
        let place = {
            tick(&mut app);
            served(&app, home)
        };
        let away = run_until(&mut app, |app| no_rover_stands_at(app, place));
        assert!(away, "the fleet never left the port it serves");

        assign(&mut app, home, 0, source);
        tick(&mut app);

        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);
    }

    #[test]
    fn lowering_a_count_takes_the_rover_off_when_it_gets_back() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
        tick(&mut app);

        assign(&mut app, home, 0, source);
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);

        assert!(gone, "a rover taken off never left the world");
    }

    #[test]
    fn a_fleet_asked_for_a_rover_back_runs_the_count_it_was_given() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        assign(&mut app, home, 1, source);
        let given_up = run_until(&mut app, |app| rovers_serving(app, home) == 1);
        assert!(given_up, "the fleet never gave up the rover it lost");

        assign(&mut app, home, A_FLEET, source);
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
        assign(&mut app, home, A_FLEET, source);
        let carrying = run_until(&mut app, |app| held_at(app, source) < A_STOCK);
        assert!(carrying, "the fleet never took anything on");

        assign(&mut app, home, 0, source);
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);

        assert!(gone, "a rover taken off never left the world");
        assert!(held_at(&app, home) > 0, "nothing ever reached the port");
        assert_eq!(held_anywhere(&mut app), A_STOCK);
    }

    #[test]
    fn a_fleet_whose_source_no_road_reaches_keeps_its_rovers_at_the_port() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1, source);
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
    fn a_fleet_sets_off_once_a_road_reaches_its_source() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1, source);
        run(&mut app, TICKS_MEASURED);
        assert_eq!(held_at(&app, home), 0);

        lay_road(&mut app, &BRANCH);
        let delivered = run_until(&mut app, |app| held_at(app, home) > 0);

        assert!(delivered, "the fleet stayed put after a road reached it");
    }

    #[test]
    fn a_fleet_sets_off_once_a_road_reaches_its_source_however_a_frame_divides() {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = outlet_at(&mut app, UP_THE_BRANCH);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, source, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1, source);
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
        lay_road(&mut app, &HAULAGE);
        let empty = outlet_at(&mut app, SOURCE);
        let stocked = outlet_at(&mut app, OTHER_HOME);
        let home = intake_at(&mut app, HOME);
        stock(&mut app, stocked, A_STOCK);
        tick(&mut app);
        assign(&mut app, home, 1, empty);
        let waiting = run_until(&mut app, |app| {
            let standing = served(app, empty);
            a_rover_stands_at(app, standing)
        });
        assert!(waiting, "the rover never reached the source it was sent to");

        assign(&mut app, home, 1, stocked);
        tick(&mut app);

        assert_eq!(
            held_at(&app, stocked),
            A_STOCK,
            "a load left a port with no rover standing at it"
        );
    }

    #[test]
    fn a_fleet_puts_out_again_the_rovers_a_bulldozed_road_took_with_it() {
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
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
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        app.world_mut().entity_mut(home).remove::<Fleet>();
        let gone = run_until(&mut app, |app| rovers_serving(app, home) == 0);
        assert!(gone, "a rover outlived the fleet it belonged to");

        assign(&mut app, home, A_FLEET, source);
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
        let (mut app, source, home) = haulage_app();
        assign(&mut app, home, A_FLEET, source);
        tick(&mut app);
        assert_eq!(rovers_serving(&mut app, home), A_FLEET as usize);

        app.world_mut().entity_mut(home).insert(Destroy);
        let gone = run_until(&mut app, |app| {
            let mut query = app.world_mut().query::<&Serving>();
            query.iter(app.world()).count() == 0
        });

        assert!(gone, "a rover outlived the port it served");
    }

    #[test]
    fn two_rovers_deliver_more_over_the_same_ticks_than_one() {
        let delivered = |rovers: u32| {
            let (mut app, source, home) = haulage_app();
            produce_at(&mut app, source);
            consume_at(&mut app, home);
            assign(&mut app, home, rovers, source);
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
}
