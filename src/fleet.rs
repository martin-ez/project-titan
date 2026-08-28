//! The rovers a port has been given, and the shuttle each of them runs.
//!
//! This is where a rover stops being something a test spawns and becomes something a building
//! has. An input port is given a number of rovers and a port to collect from, and every rover
//! assigned to it drives to that source, takes on a load, drives back, hands it over and sets
//! off again. The lever is how many rovers serve an input, never which road they take: routing
//! is [`crate::road`]'s answer and the road itself is the player's, so the only way to move more
//! is to put more rovers on the road they built and live with what that does to it.
//!
//! The source is named rather than found, and only until #132 — a rover looking for the port
//! that makes what an input takes needs recipes, and those come after this. What crosses between
//! the two ends is [`crate::rover::Cargo`], which is opaque until #26 gives a load a kind.

use crate::common::cleanup::Destroy;
use crate::diagnostics::DebugGizmos;
use crate::road::{RoadEndpoint, RoadTiles};
use crate::rover::{Cargo, Route, Rover, RoversDriven, SentTo, Stranded};
use crate::simulation::Simulation;
use bevy::prelude::*;

/// How much a rover takes on in one trip.
///
/// A load is opaque until #26 gives it a kind, so what this is worth in goods is not answerable
/// yet. What it has to be is more than one, so a source holding less than a full load hands over
/// what it has rather than nothing at all.
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

impl Plugin for FleetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_the_way_a_fleet_collects_along);
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
