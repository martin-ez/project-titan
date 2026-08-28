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
//! [`crate::rover::Cargo`], an opaque load whose kind is #26's.

use crate::common::cleanup::Destroy;
use crate::diagnostics::DebugGizmos;
use crate::road::{RoadEndpoint, RoadTiles};
use crate::rover::{Cargo, Route, Rover, RoversDriven, SentTo, Stranded};
use crate::simulation::Simulation;
use bevy::prelude::*;

/// How much a rover takes on in one trip.
///
/// A load has no kind to weigh it by, that being #26's, so what this is worth in goods is not a
/// question the game can answer. What it has to be is more than one, so a source holding less
/// than a full load hands over what it has rather than nothing at all.
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

/// A rover with nothing to do: no route, no order for one, and not stopped for want of one.
type StandingIdle = (Without<Route>, Without<SentTo>, Without<Stranded>);

impl Plugin for FleetPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(take_the_rovers_of_a_fleet_that_is_gone_off_the_road)
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
/// A route is spent the moment it is driven, and a rover holding one it has finished is a rover
/// standing still: dropping it here is what leaves the next leg of the shuttle to be decided from
/// what the rover is carrying rather than from a record of the trip it just made. At the port it
/// serves there is nothing to do but let go, the load having already changed hands on the way in.
///
/// A source with nothing to give leaves the rover waiting there holding its route, which is what
/// keeps an empty producer from costing a search every tick it stays empty.
fn turn_the_rovers_round_at_the_port_they_reached(
    mut commands: Commands,
    fleets: Query<&Fleet>,
    mut ports: Query<(&RoadEndpoint, Option<&mut Cargo>)>,
    rovers: Query<(Entity, &Rover, &Route, &Serving, Has<Cargo>)>,
) {
    for (entity, standing, route, serving, carrying) in &rovers {
        let arrived = ports
            .get(route.destination)
            .is_ok_and(|(endpoint, _)| standing.standing_at(endpoint));
        if !arrived {
            continue;
        }
        if carrying || route.destination == serving.port {
            commands.entity(entity).remove::<Route>();
            continue;
        }

        let Ok(fleet) = fleets.get(serving.port) else {
            continue;
        };
        let Ok((_, Some(mut stood))) = ports.get_mut(fleet.source) else {
            continue;
        };
        let taken = stood.quantity.min(ROVER_LOAD);
        if taken == 0 {
            continue;
        }
        stood.quantity -= taken;
        if stood.quantity == 0 {
            commands.entity(fleet.source).remove::<Cargo>();
        }
        commands
            .entity(entity)
            .insert(Cargo { quantity: taken })
            .remove::<Route>();
    }
}

/// Take off the road the rovers of every fleet given fewer than it has out.
///
/// A rover leaves at the port it serves and nowhere else, so one taken away finishes the trip it
/// is on and hands over what it is carrying before it goes — the handover having already run this
/// tick. Anything else is a load that stops existing halfway down a road, which is the free
/// transfer invariant 1 is there to forbid.
///
/// The tally comes down here rather than when the rover leaves the world, so a second rover
/// standing at the same port on the same tick is measured against what the fleet will have rather
/// than against what it had.
fn retire_the_rovers_a_fleet_no_longer_wants(
    mut commands: Commands,
    mut fleets: Query<(&Fleet, &RoadEndpoint, &mut OnTheRoad)>,
    rovers: Query<(Entity, &Rover, &Serving)>,
) {
    for (entity, standing, serving) in &rovers {
        let Ok((fleet, home, mut out)) = fleets.get_mut(serving.port) else {
            continue;
        };
        if out.0 <= fleet.rovers || !standing.standing_at(home) {
            continue;
        }
        out.0 -= 1;
        commands.entity(entity).insert(Destroy);
    }
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
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode, TileCorner};
    use crate::road::{Road, RoadPlugin, ServedBy};
    use crate::rover::RoverPlugin;
    use crate::simulation::SimulationPlugin;
    use crate::testing::{headless_app, tick};

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

    /// How much a source under test holds, far more than a run of trips can take away.
    const A_STOCK: u32 = 10_000;

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
            ));
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

    /// Stand a port on the tile at `offset`, which a road reaches when one runs through its corner.
    fn port_at(app: &mut App, offset: (i32, i32)) -> Entity {
        app.world_mut()
            .spawn(RoadEndpoint::at(node_at(offset)))
            .id()
    }

    /// Give `port` a fleet of `rovers` collecting from `source`.
    fn assign(app: &mut App, port: Entity, rovers: u32, source: Entity) {
        app.world_mut()
            .entity_mut(port)
            .insert(Fleet { rovers, source });
    }

    /// Put `quantity` at `port` for a fleet to collect.
    fn stock(app: &mut App, port: Entity, quantity: u32) {
        app.world_mut().entity_mut(port).insert(Cargo { quantity });
    }

    fn held_at(app: &App, entity: Entity) -> u32 {
        app.world()
            .entity(entity)
            .get::<Cargo>()
            .map_or(0, |load| load.quantity)
    }

    /// Everything anything in the world is holding, standing at a port or on the back of a rover.
    fn held_anywhere(app: &mut App) -> u32 {
        let mut query = app.world_mut().query::<&Cargo>();
        query.iter(app.world()).map(|load| load.quantity).sum()
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

    /// An app holding the haulage road, a stocked source and a port to serve.
    fn haulage_app() -> (App, Entity, Entity) {
        let mut app = fleet_app();
        lay_road(&mut app, &HAULAGE);
        let source = port_at(&mut app, SOURCE);
        let home = port_at(&mut app, HOME);
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
        let source = port_at(&mut app, SOURCE);
        let home = port_at(&mut app, HOME);
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
    fn two_fleets_on_one_road_do_not_draw_on_each_others_rovers() {
        let (mut app, source, home) = haulage_app();
        let other = port_at(&mut app, OTHER_HOME);
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
        let source = port_at(&mut app, UP_THE_BRANCH);
        let home = port_at(&mut app, HOME);
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
        let source = port_at(&mut app, UP_THE_BRANCH);
        let home = port_at(&mut app, HOME);
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
            assign(&mut app, home, rovers, source);
            run(&mut app, TICKS_MEASURED);
            held_at(&app, home)
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
