use crate::common::cleanup::Destroy;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::map::MAP_TILE_SIZE;
use crate::road::{
    EndsAtJunction, JunctionLegs, JunctionPolicy, NextSegment, PlaceOnTheRoad, RoadEndpoint,
    RoadNetwork, RoadSegment, RoadsLaid, SegmentCut,
};
use crate::simulation::{Simulation, Ticks};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// How long and wide the box standing in for a rover is.
const ROVER_SIZE: f32 = MAP_TILE_SIZE / 5.;

/// How tall the box standing in for a rover is.
const ROVER_HEIGHT: f32 = MAP_TILE_SIZE / 10.;

/// How many segments a rover may be handed onto in one tick.
///
/// Nothing the road tool lays comes near it: a rover crosses a segment in tens of ticks, not the
/// other way round. It is here so that a lane of segments too short to spend a whole tick on
/// cannot spin the driver, rather than to cap how fast anything goes.
const HANDOVERS_PER_TICK: usize = 8;

/// How far the debug view lifts a rover's marks off the road, so they do not fight the lane.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.2, 0.);

/// How tall the mark over a loaded rover stands for each unit it carries.
const LOAD_MARK: f32 = 0.5;

/// The colour the stretch of segment a rover has covered is drawn in
const PROGRESS_COLOUR: Color = Color::srgb(0.95, 0.55, 0.35);

/// The colour the load a rover is carrying is drawn in
const LOAD_COLOUR: Color = Color::srgb(0.9, 0.9, 0.4);

/// The colour a rover a junction is holding is marked in
const HELD_COLOUR: Color = Color::srgb(0.95, 0.3, 0.3);

/// How tall the mark over a rover a junction is holding stands.
const HELD_MARK: f32 = 1.5;

/// How far along the way out of a junction the arrow onto it reaches, in world units.
const WAY_OUT_REACH: f32 = 1.;

/// The colour the way to a rover's destination is drawn in
const ROUTE_COLOUR: Color = Color::srgb(0.4, 0.85, 0.6);

/// The colour a rover that cannot drive the route it holds is marked in
const STRANDED_COLOUR: Color = Color::srgb(0.95, 0.25, 0.6);

/// How tall the mark over a rover that cannot drive the route it holds stands.
const STRANDED_MARK: f32 = 2.;

/// The rovers on the map, and where on the road each of them stands.
///
/// A rover is where the road stops being scenery: everything a building receives arrives on one
/// (invariant 1). This is only the entity and its place — it sits on a segment and it is somewhere
/// along it. Nothing here moves it.
pub struct RoverPlugin;

/// A rover, standing somewhere along the segment it is driving.
///
/// Where it is is the distance, measured along the segment's own arc rather than as a fraction of
/// the stretch it covers, so a road cut under a rover leaves the rover reading the same point off
/// the same arc and moves it by nothing at all (invariant 6). The `Vec3` it stands at is derived
/// from that geometry every frame (invariant 3), and nothing reads the transform back to work out
/// where the rover got to, so the arc's `sin` and `cos` never reach the simulation and a chain
/// jams the same way on every machine (invariant 2).
#[derive(Component)]
#[require(Transform, Visibility = Visibility::Hidden, NeedsInitialization)]
pub struct Rover {
    /// The segment the rover is driving.
    pub segment: Entity,
    /// How far along that segment's arc it has got, between the ends of the stretch it covers.
    pub along: f32,
}

/// A rover a junction is holding at the end of its segment until its policy lets it through.
///
/// Standing still at a junction is a fact about the rover rather than one the junction stores, so
/// the tick finds the few rovers waiting by the component they carry rather than by reading every
/// rover on the map.
#[derive(Component)]
struct WaitingAtJunction {
    /// The tick the rover reached the junction on.
    since: u64,
}

/// A load, on the rover carrying it or standing at the endpoint it was left at.
///
/// Goods and recipes are #26's, and they come after traffic. Until then a load is opaque: enough
/// for something to change hands and for a jam to be worth watching, and not enough for a
/// production chain to be built on top of it. Whatever holds nothing has no `Cargo` at all.
#[derive(Component)]
pub struct Cargo {
    /// How much of it there is.
    pub quantity: u32,
}

/// A delivery a rover was handed: where it takes its load, and how it gets there.
///
/// The ways out are the whole of the route, because the lane decides everything else: a rover
/// follows the segments ahead of it until one runs into a junction, and only there is there
/// anything to choose. What is left of the list is what it has yet to take, so a road with no
/// junction on it is driven on an empty route.
///
/// They are segments rather than turns because a segment is what survives a road built across it:
/// the cut leaves the stretch before it holding the entity and the distance it started at, so a
/// route stays the route it was (invariant 6).
#[derive(Component)]
pub struct Route {
    /// The endpoint the rover drives to, stops at, and leaves its load at.
    pub destination: Entity,
    /// The segments it has yet to leave a junction by, in the order it reaches them.
    pub ways_out: Vec<Entity>,
}

/// A rover that has been sent somewhere, which is an order for a route rather than a route.
///
/// Where it is going is the whole of it: how it gets there is worked out from the network the
/// tick it is asked for, and the order is spent doing so. A rover already driving one is holding
/// a `Route` and nothing else, so a fleet under way asks for nothing and the search runs once a
/// delivery rather than once a rover a tick.
#[derive(Component)]
pub struct SentTo(pub Entity);

/// A rover that cannot get where it was sent, standing wherever it ran out of route.
///
/// A destination no road reaches, one no road joins it to, a junction that refuses the turn the
/// route names and a lane that stops short of the destination are the four ways that happens, and
/// the alternatives to saying so are a rover driving off down whatever road it can reach and one
/// quietly forgetting where it was going. Both look like a delivery still under way to anything
/// waiting on it.
#[derive(Component)]
pub struct Stranded;

/// A rover whose stretch of road was removed, holding the place it was standing at.
///
/// A place is a distance along an arc, and it is the only part of where a rover stood that
/// outlives the segment covering it. Removing an arc takes the whole road apart and lays the
/// stretches either side of it again on the next frame, so a rover on ground the removal did not
/// touch is off the road until then and back on the same ground afterwards.
#[derive(Component)]
struct OffTheRoad(PlaceOnTheRoad);

#[derive(SystemParam)]
struct RoverInitializeParams<'w, 's> {
    query: Query<'w, 's, &'static mut Visibility, With<Rover>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

impl Plugin for RoverPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(hand_the_rovers_beyond_a_cut_the_stretch_beyond_it)
            .add_observer(lift_the_rovers_off_a_removed_segment)
            .add_systems(
                PreUpdate,
                (
                    initialize_system::<Rover, RoverInitializeParams>,
                    put_the_rovers_back_on_the_road_that_survived.after(RoadsLaid),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    find_the_route_a_rover_was_sent_on,
                    let_the_rovers_through,
                    drive_the_rovers,
                    hand_the_load_to_the_endpoint_it_was_driven_to,
                )
                    .chain()
                    .in_set(Simulation),
            )
            .add_systems(
                Update,
                (
                    stand_the_rovers_on_their_segments,
                    (
                        draw_the_rovers,
                        draw_the_rovers_a_junction_holds,
                        draw_where_the_rovers_are_going,
                        draw_the_loads_standing_at_the_endpoints,
                    ),
                )
                    .chain(),
            );
    }
}

impl Initialize<RoverInitializeParams<'_, '_>> for Rover {
    fn initialize(&mut self, entity: &Entity, params: &mut RoverInitializeParams) -> Result {
        let mut visibility = params.query.get_mut(*entity)?;
        *visibility = Visibility::Visible;

        params.commands.entity(*entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(
                    params
                        .meshes
                        .add(Cuboid::new(ROVER_SIZE, ROVER_HEIGHT, ROVER_SIZE)),
                ),
                MeshMaterial3d(params.materials.add(Color::srgb(0.85, 0.5, 0.35))),
                Transform::from_translation(Vec3::new(0., ROVER_HEIGHT / 2., 0.)),
            ));
        });
        Ok(())
    }
}

/// Work out the route every rover that has been sent somewhere drives, and spend the order.
///
/// A search runs when a rover is sent rather than while it drives, so what it costs is one search
/// a delivery however long the journey and however large the fleet already under way. What it
/// finds is the quickest way through an empty network: nothing here reads how many rovers a
/// segment is carrying, and a route that answers to congestion is #8's.
fn find_the_route_a_rover_was_sent_on(
    mut commands: Commands,
    mut network: RoadNetwork,
    sent: Query<(Entity, &Rover, &SentTo)>,
) {
    for (entity, rover, sent_to) in &sent {
        let found = network.fastest_way(rover.segment, rover.along, sent_to.0);
        let mut rover = commands.entity(entity);
        rover.remove::<SentTo>();
        match found {
            Some(ways_out) => {
                rover.insert(Route {
                    destination: sent_to.0,
                    ways_out,
                });
            }
            None => {
                rover.insert_if_new(Stranded);
            }
        }
    }
}

/// Let one rover through each junction that has any waiting, by the policy the junction holds.
///
/// One a tick, so a junction is a place where traffic has to take its turn rather than a point
/// rovers pass through together. Which leg goes is the policy's answer and the tick's rotation,
/// never the order the world stores its rovers in (invariant 2); which ways out are open is the
/// junction's to say and which is taken is the route's. The ones not let through keep their place
/// and their arrival, so the longest wait on a leg is served first when its turn comes.
///
/// A rover already stranded is not offered a turn: it is going nowhere either way, and a leg whose
/// turn is spent every tick on the same rover is a leg no other rover ever leaves by.
fn let_the_rovers_through(
    mut commands: Commands,
    ticks: Res<Ticks>,
    junctions: Query<(&JunctionLegs, &JunctionPolicy)>,
    segments: Query<(&RoadSegment, Option<&EndsAtJunction>)>,
    mut rovers: Query<
        (Entity, &mut Rover, &WaitingAtJunction, Option<&mut Route>),
        Without<Stranded>,
    >,
    mut held: Local<Vec<(Entity, usize, u64, Entity)>>,
    mut legs_waiting: Local<Vec<usize>>,
) {
    held.clear();
    for (entity, rover, wait, _) in &rovers {
        let Ok(ends) = segments.get(rover.segment).map(|(_, ends)| ends) else {
            continue;
        };
        let Some(ends) = ends else {
            commands.entity(entity).remove::<WaitingAtJunction>();
            continue;
        };
        held.push((ends.junction, ends.leg, wait.since, entity));
    }
    held.sort_unstable();

    let mut from = 0;
    while from < held.len() {
        let junction = held[from].0;
        let queue = held[from..].partition_point(|waiting| waiting.0 == junction);
        let queue = &held[from..from + queue];
        from += queue.len();

        let Ok((legs, policy)) = junctions.get(junction) else {
            continue;
        };
        legs_waiting.clear();
        legs_waiting.extend(queue.iter().map(|&(_, leg, ..)| leg));
        legs_waiting.dedup();

        let Some(leg) = policy.who_goes_next(legs, &legs_waiting, ticks.0) else {
            continue;
        };
        let Some(&(.., rover)) = queue.iter().find(|&&(_, waiting, ..)| waiting == leg) else {
            continue;
        };
        let open = legs.exits_from(leg);
        let Ok((_, mut let_through, _, route)) = rovers.get_mut(rover) else {
            continue;
        };
        let Some(out) = the_way_out_taken(route, &open) else {
            commands.entity(rover).insert_if_new(Stranded);
            continue;
        };

        let Ok((exit, _)) = segments.get(out) else {
            continue;
        };
        let_through.segment = out;
        let_through.along = exit.starts_at();
        commands.entity(rover).remove::<WaitingAtJunction>();
    }
}

/// Which of the ways `open` to a rover it leaves the junction by, spending its route if it has one.
///
/// A route names every junction it passes, so one with nothing left to say at a junction is as
/// undrivable as one naming a turn the junction refuses, and neither is a route to carry on down.
fn the_way_out_taken(route: Option<Mut<Route>>, open: &[Entity]) -> Option<Entity> {
    let Some(mut route) = route else {
        return open.first().copied();
    };
    let taken = route
        .ways_out
        .first()
        .copied()
        .filter(|way| open.contains(way))?;
    route.ways_out.remove(0);
    Some(taken)
}

/// Hand every rover standing past a cut onto the stretch of road beyond it.
///
/// Only which segment it is on changes. How far it has driven is measured along the arc and the
/// cut left the arc alone, so a rover crossed by a new road stands where it stood to the bit,
/// however often the road under it is crossed again (invariant 6). A rover short of the cut is on
/// the stretch it was already on and is not touched at all.
fn hand_the_rovers_beyond_a_cut_the_stretch_beyond_it(
    cut: On<SegmentCut>,
    mut rovers: Query<&mut Rover>,
    segments: Query<&RoadSegment>,
) {
    let Ok(head) = segments.get(cut.segment) else {
        return;
    };
    let cut_at = head.ends_at();
    for mut rover in &mut rovers {
        if rover.segment == cut.segment && rover.along > cut_at {
            rover.segment = cut.beyond;
        }
    }
}

/// Lift every rover standing on a removed segment off the road, keeping the place it stood at.
///
/// The segment goes and the place does not, which is what leaves a rover something to be put back
/// down on. It is taken while the segment is still there to be read, because a distance along an
/// arc says nothing on its own once the arc it was measured against has gone.
fn lift_the_rovers_off_a_removed_segment(
    removed: On<Remove, RoadSegment>,
    rovers: Query<(Entity, &Rover)>,
    segments: Query<&RoadSegment>,
    mut commands: Commands,
) {
    let Ok(going) = segments.get(removed.entity) else {
        return;
    };
    for (rover, standing) in &rovers {
        if standing.segment == removed.entity {
            commands
                .entity(rover)
                .insert(OffTheRoad(going.place_at(standing.along)));
        }
    }
}

/// Put every rover the road went out from under back on the stretch of it that survived.
///
/// A road bulldozed under traffic takes the traffic with it, load and all, so clearing a jam by
/// removing the road under it is paid for in rovers rather than free (invariant 1). What it must
/// not take is the traffic on the stretches it left standing: a removal lays those again exactly
/// as they were, so the ground a rover held is either back to the bit or gone with the arc, and
/// which of the two it is is the question of whether any segment covers it (invariant 6).
///
/// It runs once the frame's roads are laid, which is what separates a stretch that has gone from
/// one not laid yet, and before the tick, so a rover put back loses none of its journey.
fn put_the_rovers_back_on_the_road_that_survived(
    mut commands: Commands,
    mut lifted: Query<(Entity, &mut Rover, &OffTheRoad)>,
    segments: Query<(Entity, &RoadSegment)>,
) {
    for (entity, mut rover, stood) in &mut lifted {
        let standing = segments
            .iter()
            .find(|(_, segment)| segment.covers(&stood.0))
            .map(|(segment, _)| segment);
        let Some(segment) = standing else {
            commands.entity(entity).insert(Destroy);
            continue;
        };

        rover.segment = segment;
        rover.along = stood.0.along();
        commands.entity(entity).remove::<OffTheRoad>();
    }
}

/// Drive every rover along its lane, at whatever each segment it crosses allows.
///
/// A tick buys a rover an amount of time rather than an amount of ground, and it is spent segment
/// by segment: what is left when it reaches the end of one is carried onto the next and spent at
/// that one's speed limit, so a rover joining a curve slows down on the curve, not a tick early.
///
/// Four things stop it short of the road ahead: the lane running out, which is where a rover whose
/// road was removed stands; a junction, whose way on is its own to give; its destination, where
/// the route it still holds parks it; and a route it cannot drive, which strands it where it is.
fn drive_the_rovers(
    mut commands: Commands,
    ticks: Res<Ticks>,
    mut rovers: Query<(Entity, &mut Rover, Option<&Route>), Without<Stranded>>,
    segments: Query<(&RoadSegment, Option<&NextSegment>, Option<&EndsAtJunction>)>,
    endpoints: Query<&RoadEndpoint>,
) {
    for (entity, mut rover, route) in &mut rovers {
        let stops_at = match route {
            None => None,
            Some(route) => {
                let bound_for = endpoints
                    .get(route.destination)
                    .ok()
                    .and_then(RoadEndpoint::served_by);
                let Some(served) = bound_for else {
                    commands.entity(entity).insert_if_new(Stranded);
                    continue;
                };
                Some(served)
            }
        };

        let mut left = 1.;
        for _ in 0..HANDOVERS_PER_TICK {
            let Ok((segment, next, junction)) = segments.get(rover.segment) else {
                break;
            };
            let arriving = stops_at
                .filter(|served| served.segment == rover.segment && served.along >= rover.along)
                .map(|served| served.along);
            let ends_at = arriving.unwrap_or_else(|| segment.ends_at());
            let crossing = (ends_at - rover.along) / segment.speed_limit();
            if crossing > left {
                rover.along += left * segment.speed_limit();
                break;
            }

            left -= crossing;
            rover.along = ends_at;
            if arriving.is_some() {
                break;
            }
            if junction.is_some() {
                commands
                    .entity(entity)
                    .insert_if_new(WaitingAtJunction { since: ticks.0 });
                break;
            }
            match next.and_then(|next| Some((next.0, segments.get(next.0).ok()?.0.starts_at()))) {
                Some((onward, from)) => {
                    rover.segment = onward;
                    rover.along = from;
                }
                None => {
                    if stops_at.is_some() {
                        commands.entity(entity).insert_if_new(Stranded);
                    }
                    break;
                }
            }
        }
    }
}

/// Hand the load of every rover standing at its destination to whatever is built there.
///
/// This is invariant 1 coming due: what a building receives it receives because a rover drove a
/// road to it, and here is where the load stops being the rover's. It is instant, and it happens
/// once — the whole load goes, so a rover parked at the endpoint it delivered to is carrying
/// nothing and has nothing left to hand over. Holding it there for a number of ticks nothing in
/// the game measures would be a balance claim with no measurement behind it (2.3).
fn hand_the_load_to_the_endpoint_it_was_driven_to(
    mut commands: Commands,
    rovers: Query<(Entity, &Rover, &Route, &Cargo)>,
    endpoints: Query<&RoadEndpoint>,
) {
    for (entity, standing, route, load) in &rovers {
        let arrived = endpoints
            .get(route.destination)
            .ok()
            .and_then(RoadEndpoint::served_by)
            .is_some_and(|served| {
                served.segment == standing.segment && served.along == standing.along
            });
        if !arrived {
            continue;
        }

        let quantity = load.quantity;
        commands
            .entity(route.destination)
            .entry::<Cargo>()
            .and_modify(move |mut held| held.quantity += quantity)
            .or_insert(Cargo { quantity });
        commands.entity(entity).remove::<Cargo>();
    }
}

/// Put every rover where its distance along its segment says it stands.
///
/// This is presentation and runs on the frame: it reads the simulation's distance and writes a
/// transform, and nothing on the tick reads what it wrote. A rover whose segment is gone is left
/// where it was rather than taking the game down, for the frame it takes to leave the world with
/// the road under it.
fn stand_the_rovers_on_their_segments(
    mut rovers: Query<(&Rover, &mut Transform)>,
    segments: Query<&RoadSegment>,
) {
    for (rover, mut transform) in &mut rovers {
        let Ok(segment) = segments.get(rover.segment) else {
            continue;
        };
        transform.translation = segment.world_position(rover.along);
    }
}

/// Draw how far along its segment each rover has got, and what it is carrying.
///
/// A box on a lane says nothing about either: a rover stuck at a junction and one crawling look
/// the same from one frame, and a loaded rover looks like an empty one. The arrow grows out of the
/// segment's start as the rover covers it, and the mark over a rover stands for its load.
fn draw_the_rovers(
    mut gizmos: Gizmos<DebugGizmos>,
    rovers: Query<(&Rover, Option<&Cargo>)>,
    segments: Query<&RoadSegment>,
) {
    for (rover, cargo) in &rovers {
        let Ok(segment) = segments.get(rover.segment) else {
            continue;
        };
        let standing = segment.world_position(rover.along) + GIZMO_LIFT;
        gizmos.arrow(
            segment.world_position(segment.starts_at()) + GIZMO_LIFT,
            standing,
            PROGRESS_COLOUR,
        );

        let Some(cargo) = cargo else {
            continue;
        };
        gizmos.line(
            standing,
            standing + Vec3::Y * LOAD_MARK * cargo.quantity as f32,
            LOAD_COLOUR,
        );
    }
}

/// Mark every rover a junction is holding, and the way out it is waiting to be let onto.
///
/// A rover stopped at a junction and one crawling towards it stand the same way round in the same
/// place, and nothing about a box on a lane says which leg it is bound for. The mark says it is
/// being held and the arrow says which way it goes when it is let through.
fn draw_the_rovers_a_junction_holds(
    mut gizmos: Gizmos<DebugGizmos>,
    held: Query<&Rover, With<WaitingAtJunction>>,
    arriving: Query<&EndsAtJunction>,
    junctions: Query<&JunctionLegs>,
    segments: Query<&RoadSegment>,
) {
    for rover in &held {
        let Ok(segment) = segments.get(rover.segment) else {
            continue;
        };
        let standing = segment.world_position(rover.along) + GIZMO_LIFT;
        gizmos.line(standing, standing + Vec3::Y * HELD_MARK, HELD_COLOUR);

        let Some(out) = arriving
            .get(rover.segment)
            .ok()
            .and_then(|ends| {
                junctions
                    .get(ends.junction)
                    .ok()
                    .map(|legs| (legs, ends.leg))
            })
            .and_then(|(legs, leg)| legs.exits_from(leg).first().copied())
            .and_then(|out| segments.get(out).ok())
        else {
            continue;
        };
        gizmos.arrow(
            standing,
            out.world_position(out.starts_at() + WAY_OUT_REACH) + GIZMO_LIFT,
            HELD_COLOUR,
        );
    }
}

/// Draw where every rover on a route is bound, and mark the ones that will not get there.
///
/// A rover carrying out a delivery looks like a rover driving about, and a stranded one looks like
/// a rover that has merely stopped. The arrow says what it was sent to do and its colour says
/// whether it still can (invariant 5).
fn draw_where_the_rovers_are_going(
    mut gizmos: Gizmos<DebugGizmos>,
    rovers: Query<(&Rover, &Route, Option<&Stranded>)>,
    endpoints: Query<&RoadEndpoint>,
    segments: Query<&RoadSegment>,
) {
    for (rover, route, stranded) in &rovers {
        let Ok(segment) = segments.get(rover.segment) else {
            continue;
        };
        let standing = segment.world_position(rover.along) + GIZMO_LIFT;
        let Some(bound_for) = endpoints
            .get(route.destination)
            .ok()
            .and_then(RoadEndpoint::served_by)
            .and_then(|served| standing_place(served.segment, served.along, &segments))
        else {
            gizmos.line(
                standing,
                standing + Vec3::Y * STRANDED_MARK,
                STRANDED_COLOUR,
            );
            continue;
        };

        let colour = if stranded.is_some() {
            STRANDED_COLOUR
        } else {
            ROUTE_COLOUR
        };
        gizmos.arrow(standing, bound_for + GIZMO_LIFT, colour);
    }
}

/// Draw what is standing at each endpoint, waiting to be taken or just delivered.
///
/// A load that changed hands is otherwise invisible: what it was left at looks the same holding a
/// hundred as holding none, and a delivery landing is the one thing a road full of rovers is for
/// (invariant 5).
fn draw_the_loads_standing_at_the_endpoints(
    mut gizmos: Gizmos<DebugGizmos>,
    endpoints: Query<(&RoadEndpoint, &Cargo)>,
    segments: Query<&RoadSegment>,
) {
    for (endpoint, load) in &endpoints {
        let Some(standing) = endpoint
            .served_by()
            .and_then(|served| standing_place(served.segment, served.along, &segments))
        else {
            continue;
        };

        let standing = standing + GIZMO_LIFT;
        gizmos.line(
            standing,
            standing + Vec3::Y * LOAD_MARK * load.quantity as f32,
            LOAD_COLOUR,
        );
    }
}

/// Where on the ground a distance along a segment puts whatever holds it.
fn standing_place(segment: Entity, along: f32, segments: &Query<&RoadSegment>) -> Option<Vec3> {
    Some(segments.get(segment).ok()?.world_position(along))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode, MAP_TILE_INRADIUS};
    use crate::road::{EndsAtJunction, JunctionLegs, Road, RoadEndpoint, RoadPlugin, ServedBy};
    use crate::simulation::{SimulationPlugin, Ticks};
    use crate::testing::{advance, headless_app, tick, trace};
    use std::time::Duration;

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// A straight run of tiles, in offset-row coordinates.
    ///
    /// It sets off away from the origin, so a rover that was never placed at all stands somewhere
    /// its segment does not run through rather than at the first tile by coincidence.
    const STRAIGHT: [(i32, i32); 4] = [(1, 0), (2, 0), (3, 0), (4, 0)];

    /// A run of tiles that turns back on itself twice, in offset-row coordinates.
    ///
    /// The same number of tiles as `STRAIGHT` and the same distance between each, so a rover has
    /// as much road ahead of it here as there and only the curves tell the two apart.
    const WINDING: [(i32, i32); 4] = [(1, 6), (2, 6), (1, 7), (2, 7)];

    /// A run of tiles crossing `STRAIGHT` at its second tile, in offset-row coordinates.
    const CROSSING: [(i32, i32); 3] = [(2, -1), (2, 0), (2, 1)];

    /// A run of tiles ending on `STRAIGHT`'s second tile, in offset-row coordinates.
    const SPUR: [(i32, i32); 2] = [(2, 1), (2, 0)];

    /// A run of tiles reaching the tile another road crosses, in offset-row coordinates.
    const APPROACH: [(i32, i32); 4] = [(0, 0), (1, 0), (2, 0), (3, 0)];

    /// A straight run of tiles crossing `APPROACH`'s last one, in offset-row coordinates.
    ///
    /// A rover reaching the junction may leave by either arm and takes the straighter of the two,
    /// which is the arm turning sixty degrees rather than the one turning a hundred and twenty.
    const ACROSS: [(i32, i32); 3] = [(2, 1), (3, 0), (3, -1)];

    /// The same four steps of the lattice with no turn in them, in offset-row coordinates.
    const STRAIGHT_ON: [(i32, i32); 5] = [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)];

    /// Where both routes through the corner come out, in offset-row coordinates.
    const PAST_THE_TURN: (i32, i32) = (3, -1);

    /// Where the route with no turn in it comes out, in offset-row coordinates.
    const PAST_THE_STRAIGHT: (i32, i32) = (4, 0);

    /// How many ticks the wait at a junction costs a rover that nothing else is holding up.
    ///
    /// The handover is one tick and the tick it lands on is where the rounding goes, so a rover
    /// carried straight through has lost this much of its journey and no more. What a rover that
    /// turns loses on top is the arc it drives, which is the junction's whole cost.
    const TICKS_LOST_AT_A_JUNCTION: u32 = 2;

    /// How near the middle of its last tile a rover has to stand to have arrived there.
    const ARRIVED_WITHIN: f32 = 0.3;

    /// How many legs two two-way roads crossing each other make.
    const LEGS_OF_A_CROSSROADS: usize = 4;

    /// How far along a segment, as a share of it, a rover stands to reach the end in one tick.
    const ABOUT_TO_ARRIVE: f32 = 0.99;

    /// A frame far too short to carry a tick of the fixed clock.
    const SHORT_FRAME: Duration = Duration::from_micros(100);

    /// How many segments a lane may hold before a walk along it has plainly lost its way.
    const LAP_SEGMENTS: usize = 1024;

    /// How many ticks a rover is given to cross one segment before the test gives up on it.
    const TICKS_ALLOWED: u32 = 4096;

    /// How many ticks the two roads are driven for before what each delivered is compared.
    ///
    /// Well short of what either rover needs to reach the end of its road, so both were offered
    /// the same stretch of it and only their speed limits decide how much went under them.
    const TICKS_MEASURED: u32 = 100;

    /// How many frames carrying no tick it takes for a rover driven by the frame to have moved.
    const FRAMES_WITHOUT_A_TICK: u32 = 32;

    /// Somewhere no segment of the road under test runs through.
    const NOWHERE: Vec3 = Vec3::new(999., 999., 999.);

    /// A direction from a tile's middle far enough towards a corner of it to settle on that corner.
    const TOWARDS_A_CORNER: Vec3 = Vec3::new(0., 0., MAP_TILE_INRADIUS);

    /// A direction from a tile's middle far enough towards the corner two round from that one.
    const TOWARDS_THE_CORNER_TWO_ROUND: Vec3 =
        Vec3::new(MAP_TILE_INRADIUS, 0., -MAP_TILE_INRADIUS / 2.);

    /// How far along a segment the rover set down early on it stands.
    const EARLY_ALONG: f32 = 0.25;

    /// How far along a segment the rover set down late on it stands.
    const LATE_ALONG: f32 = 0.8;

    /// How many ticks a rover is driven for once the road ahead of it has gone.
    ///
    /// More than the two segments between it and the gap take to cross, so a rover still holding a
    /// route through the gap has had every chance to drive into it.
    const TICKS_PAST_THE_GAP: u32 = 128;

    /// Which of `STRAIGHT`'s three arcs the removal under test takes off the road.
    ///
    /// The middle one, so what the removal leaves is a road either side of it: one running on from
    /// the end the road was begun at, and one setting off along the tangent the arc that went
    /// ended on. A rover on either of them is standing on ground the removal did not touch.
    const MIDDLE_ARC: usize = 1;

    /// The tile a delivery sets off from, in offset-row coordinates.
    const COLLECTION: (i32, i32) = (0, 0);

    /// The tile a delivery straight down one road is bound for, in offset-row coordinates.
    const DELIVERY: (i32, i32) = (0, 6);

    /// The tile the road crossing that one sets off from, in offset-row coordinates.
    const ACROSS_FROM: (i32, i32) = (-2, 3);

    /// The tile a delivery has to turn at the junction to reach, in offset-row coordinates.
    const ACROSS_TO: (i32, i32) = (2, 3);

    /// A tile far enough from either road that nothing serves it, in offset-row coordinates.
    const OFF_THE_NETWORK: (i32, i32) = (8, 8);

    /// A tile on a road of its own, in offset-row coordinates.
    ///
    /// Far enough from the road under test that no arc of either reaches the other, so a rover on
    /// one is served by a network the other is no part of rather than merely a long way off.
    const ELSEWHERE_FROM: (i32, i32) = (12, 12);

    /// The far end of that road, in offset-row coordinates.
    const ELSEWHERE_TO: (i32, i32) = (12, 16);

    /// The road a delivery through the fork sets off down, in offset-row coordinates.
    const THE_STEM: [(i32, i32); 2] = [(-1, 0), (0, 0)];

    /// The arm of the fork that covers less road and costs more time, in offset-row coordinates.
    ///
    /// Three tiles of corners tight enough that driving them costs half as much time again as the
    /// longer way round, which is what leaves the shortest route and the quickest route two
    /// different answers to ask for.
    const THE_SHORT_ARM: [(i32, i32); 5] = [(0, 0), (1, -1), (2, -1), (3, -1), (4, 0)];

    /// The arm that covers more road and costs less time, in offset-row coordinates.
    const THE_LONG_ARM: [(i32, i32); 4] = [(0, 0), (1, 2), (3, 2), (4, 0)];

    /// That same arm as the player would have drawn it the other way about.
    const THE_LONG_ARM_REVERSED: [(i32, i32); 4] = [(4, 0), (3, 2), (1, 2), (0, 0)];

    /// The road running on from where the arms come back together, in offset-row coordinates.
    const THE_RUN_OUT: [(i32, i32); 2] = [(4, 0), (5, 0)];

    /// The tile a delivery through the fork sets off from, in offset-row coordinates.
    const FORK_FROM: (i32, i32) = (-2, 0);

    /// The tile a delivery through the fork is bound for, in offset-row coordinates.
    const FORK_TO: (i32, i32) = (6, 0);

    /// The tile the shorter arm of the fork heads for, in offset-row coordinates.
    const THE_SHORT_WAY: (i32, i32) = (1, -1);

    /// The tile the longer arm of the fork heads for, in offset-row coordinates.
    const THE_LONG_WAY: (i32, i32) = (1, 2);

    /// How many roads run each way across the network a route is looked for over.
    const ROADS_EACH_WAY: i32 = 11;

    /// How many tiles apart those roads run.
    const ROAD_SPACING: i32 = 3;

    /// The tile each of them begins on, in offset-row coordinates.
    ///
    /// Short of the first road crossing it, so that every crossing of the network is one road
    /// drawn across the middle of another rather than one begun on the edge of it.
    const ROAD_BEGINS_AT: i32 = -2;

    /// The tile each of them ends on, in offset-row coordinates.
    const ROAD_ENDS_AT: i32 = 33;

    /// How many segments a network has to hold to be worth looking for a route across.
    const A_LARGE_NETWORK: usize = 2000;

    /// How much a rover carries on a delivery under test.
    const LOAD: u32 = 3;

    /// How many ticks a delivery is given to land before the test gives up on it.
    const TICKS_TO_DELIVER: u32 = 4096;

    /// How many ticks a rover that will not arrive is driven for before it is asked where it got to.
    const TICKS_GOING_NOWHERE: u32 = 256;

    /// A frame carrying exactly one tick, which is the rate every other run is compared against.
    ///
    /// The timestep is 15625 µs, and 15625 is five to the sixth, so each frame length below
    /// divides it exactly and hundreds of them accumulate no drift for a comparison to trip on.
    const A_TICK_A_FRAME: [Duration; 1] = [Duration::from_micros(15_625)];

    /// Five frames to the tick, four of them carrying none.
    const FIVE_FRAMES_A_TICK: [Duration; 1] = [Duration::from_micros(3_125)];

    /// One frame to four ticks, which is a machine too slow to draw the world every tick.
    const FOUR_TICKS_A_FRAME: [Duration; 1] = [Duration::from_micros(62_500)];

    /// Frame lengths no clock hands out twice, one of them too short to carry a tick at all.
    const RAGGED_FRAMES: [Duration; 5] = [
        Duration::from_micros(3_125),
        Duration::from_micros(100),
        Duration::from_micros(46_875),
        Duration::from_micros(15_625),
        Duration::from_micros(625),
    ];

    /// How many ticks two traces are compared over.
    ///
    /// Long enough for a rover to reach the junction, be let through it and drive on down the far
    /// road, which is the stretch of a journey where the tick has anything to decide.
    const TICKS_TRACED: usize = 400;

    /// How much of its segment a rover waiting at the junction ahead of it has covered.
    const AT_THE_JUNCTION: f32 = 1.;

    fn rover_app() -> App {
        rover_app_holding(PlayerAction::Select)
    }

    fn rover_app_holding(tool: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(tool)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                SimulationPlugin,
                DebugGizmosPlugin,
                CleanupPlugin,
                RoadPlugin,
                RoverPlugin,
                BuildingPlugin,
            ));
        app
    }

    fn tiles(offsets: &[(i32, i32)]) -> Vec<HexCoordinates> {
        offsets
            .iter()
            .map(|&(col, row)| HexCoordinates::from_offset_row(col, row))
            .collect()
    }

    /// An app holding a straight road, laid and cut into segments.
    fn road_app() -> App {
        let mut app = rover_app();
        let nodes = tiles(&STRAIGHT)
            .into_iter()
            .map(LatticeNode::from_tile)
            .collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
        tick(&mut app);
        app
    }

    /// The segment that sets off from `tile`, of which the road has exactly one.
    fn segment_from(app: &mut App, tile: HexCoordinates) -> Entity {
        let target = tile.world_position();
        app.world_mut()
            .query::<(Entity, &RoadSegment)>()
            .iter(app.world())
            .find(|(_, segment)| {
                segment.world_position(segment.starts_at()).distance(target) < TOLERANCE
            })
            .map(|(entity, _)| entity)
            .expect("the road has a segment setting off from the tile")
    }

    /// The two ends of `segment`, which on a straight road are two ends of a straight line.
    fn ends_of(app: &mut App, segment: Entity) -> (Vec3, Vec3) {
        let segment = app
            .world()
            .entity(segment)
            .get::<RoadSegment>()
            .expect("the segment is still there");
        (
            segment.world_position(segment.starts_at()),
            segment.world_position(segment.ends_at()),
        )
    }

    fn spawn_rover(app: &mut App, segment: Entity, along: f32) -> Entity {
        app.world_mut().spawn(Rover { segment, along }).id()
    }

    /// The rover's segment and how far along it, which together are where it is.
    fn place_of(app: &App, rover: Entity) -> (Entity, f32) {
        let rover = app
            .world()
            .entity(rover)
            .get::<Rover>()
            .expect("the rover is still there");
        (rover.segment, rover.along)
    }

    /// How far `rover` has driven since it set off from the start of `from`.
    ///
    /// Walked along the lane rather than measured between two world positions: a rover round a
    /// bend covers more ground than the straight line it ends up displaced by, and how much road
    /// went under it is the whole of what a speed limit decides.
    fn driven_from(app: &App, rover: Entity, from: Entity) -> f32 {
        let (standing, along) = place_of(app, rover);
        let mut segment = from;
        let mut driven = 0.;
        for _ in 0..LAP_SEGMENTS {
            let length = length_of(app, segment);
            if segment == standing {
                return driven + along - place_along(app, segment, 0.);
            }
            driven += length;
            segment = app
                .world()
                .entity(segment)
                .get::<NextSegment>()
                .expect("the lane runs on")
                .0;
        }
        f32::NAN
    }

    fn length_of(app: &App, segment: Entity) -> f32 {
        app.world()
            .entity(segment)
            .get::<RoadSegment>()
            .expect("the segment is still there")
            .length()
    }

    fn speed_limit_of(app: &App, segment: Entity) -> f32 {
        app.world()
            .entity(segment)
            .get::<RoadSegment>()
            .expect("the segment is still there")
            .speed_limit()
    }

    /// How many ticks it takes `rover` to leave the segment it is standing on.
    fn ticks_to_cross(app: &mut App, rover: Entity) -> u32 {
        let (setting_off, _) = place_of(app, rover);
        for taken in 1..=TICKS_ALLOWED {
            tick(app);
            if place_of(app, rover).0 != setting_off {
                return taken;
            }
        }
        TICKS_ALLOWED
    }

    /// How many ticks a rover takes to drive from the start of the road through `from` to `to`.
    ///
    /// The `roads` are laid in an app of their own, so no other traffic is contending for a
    /// junction on the way and the count is the route's own rather than the map's.
    fn ticks_along(roads: &[&[(i32, i32)]], from: &[(i32, i32)], to: (i32, i32)) -> u32 {
        let mut app = rover_app();
        for road in roads {
            lay_road(&mut app, road);
        }
        tick(&mut app);
        tick(&mut app);

        let arrived = tiles(&[to])[0].world_position();
        let (rover, _) = set_off_along(&mut app, from);
        for taken in 1..=TICKS_ALLOWED {
            tick(&mut app);
            if standing_at(&app, rover).distance(arrived) < ARRIVED_WITHIN {
                return taken;
            }
        }
        TICKS_ALLOWED
    }

    /// Lay a road through `offsets` on `app`, which takes a tick to become segments.
    fn lay_road(app: &mut App, offsets: &[(i32, i32)]) {
        let nodes = tiles(offsets)
            .into_iter()
            .map(LatticeNode::from_tile)
            .collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
    }

    /// Set a rover off from the start of the first segment of the road through `offsets`.
    fn set_off_along(app: &mut App, offsets: &[(i32, i32)]) -> (Entity, Entity) {
        let segment = segment_from(app, tiles(offsets)[0]);
        (spawn_rover(app, segment, 0.), segment)
    }

    /// The segment of the one road in `app` that holds a rover to the lowest speed.
    fn slowest_segment_in(app: &mut App) -> Entity {
        app.world_mut()
            .query::<(Entity, &RoadSegment)>()
            .iter(app.world())
            .min_by(|(_, one), (_, other)| one.speed_limit().total_cmp(&other.speed_limit()))
            .map(|(segment, _)| segment)
            .expect("the road has segments")
    }

    /// How fast a segment with no curve in it allows, which is the fastest a segment gets.
    fn the_open_road() -> f32 {
        let (app, _, straight) = road_with_a_driver(&STRAIGHT);
        speed_limit_of(&app, straight)
    }

    /// An app of its own holding one road and one rover at the start of it.
    ///
    /// Its own, because a tick drives every rover in the world: two rovers measured in one app
    /// would each be carried along by the other's measurement.
    fn road_with_a_driver(offsets: &[(i32, i32)]) -> (App, Entity, Entity) {
        let mut app = rover_app();
        lay_road(&mut app, offsets);
        tick(&mut app);
        let (rover, segment) = set_off_along(&mut app, offsets);
        (app, rover, segment)
    }

    fn standing_at(app: &App, rover: Entity) -> Vec3 {
        app.world()
            .entity(rover)
            .get::<Transform>()
            .expect("a rover has a transform")
            .translation
    }

    fn move_to(app: &mut App, rover: Entity, along: f32) {
        app.world_mut()
            .entity_mut(rover)
            .get_mut::<Rover>()
            .expect("the rover is still there")
            .along = along;
    }

    fn put_the_box_at(app: &mut App, rover: Entity, place: Vec3) {
        app.world_mut()
            .entity_mut(rover)
            .get_mut::<Transform>()
            .expect("a rover has a transform")
            .translation = place;
    }

    /// A road laid, a rover put `along` of the way down its first segment, and a frame to be seen.
    ///
    /// The frame carries no tick, so the rover is placed where it was put rather than where a
    /// tick of driving would have taken it: these are the tests of what a box on a lane shows.
    fn road_and_rover(along: f32) -> (App, Entity, Entity) {
        let mut app = road_app();
        let segment = segment_from(&mut app, tiles(&STRAIGHT)[0]);
        let rover = set_down_on(&mut app, segment, along);
        advance(&mut app, SHORT_FRAME);
        (app, rover, segment)
    }

    fn corner_of(offset: (i32, i32), towards: Vec3) -> LatticeNode {
        let tile = HexCoordinates::from_offset_row(offset.0, offset.1);
        LatticeNode::nearest_on(tile, tile.world_position() + towards)
    }

    /// The two corners of the tile at `offset` a road drawn between crosses `STRAIGHT` at.
    ///
    /// Neither corner stands on `STRAIGHT`, and the straight between them meets it between two of
    /// its nodes and inside a segment rather than at an end of one, so what it cuts is a stretch a
    /// rover can be standing on.
    fn crossing_arm(offset: (i32, i32)) -> Vec<LatticeNode> {
        vec![
            corner_of(offset, TOWARDS_A_CORNER),
            corner_of(offset, TOWARDS_THE_CORNER_TWO_ROUND),
        ]
    }

    /// Lay a road across the middle of `STRAIGHT`, on a frame that carries no tick.
    ///
    /// No tick, so where a rover stands afterwards is where the cut left it rather than where it
    /// drove to next.
    fn cut_the_road_across(app: &mut App) {
        app.world_mut().spawn(Road {
            nodes: crossing_arm(STRAIGHT[1]),
            leaving: None,
            one_way: false,
        });
        advance(app, SHORT_FRAME);
    }

    fn segments_in(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<RoadSegment>>()
            .iter(app.world())
            .collect()
    }

    fn next_of(app: &App, segment: Entity) -> Option<Entity> {
        app.world()
            .get_entity(segment)
            .ok()?
            .get::<NextSegment>()
            .map(|next| next.0)
    }

    fn the_road_in(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .next()
            .expect("the road is still there")
    }

    /// Every segment of the road, with a rover set down early on it and another set down late.
    ///
    /// Two on each, so wherever a crossing road cuts one there is a rover either side of the cut
    /// and the test is not left asserting about a stretch nobody was standing on.
    fn rovers_all_along(app: &mut App) -> Vec<(Entity, Entity, Entity)> {
        segments_in(app)
            .into_iter()
            .map(|segment| {
                let early = set_down_on(app, segment, EARLY_ALONG);
                let late = set_down_on(app, segment, LATE_ALONG);
                (segment, early, late)
            })
            .collect()
    }

    /// `STRAIGHT` laid, with the road tool in hand to take an arc of it off again.
    fn a_road_to_edit() -> App {
        let mut app = rover_app_holding(PlayerAction::EditRoads);
        lay_road(&mut app, &STRAIGHT);
        tick(&mut app);
        app
    }

    /// A rover early and late on every segment of the road, and where each of them stands.
    ///
    /// Every segment of both lanes, so a removal has rovers on the ground either side of the arc
    /// it takes as well as on the ground that goes. Where each stands is read before the removal,
    /// because that is what a removal that moved nothing has to leave alone.
    fn rovers_all_along_and_where_they_stand(app: &mut App) -> Vec<(Entity, Vec3)> {
        let rovers = rovers_all_along(app);
        advance(app, SHORT_FRAME);
        rovers
            .iter()
            .flat_map(|&(_, early, late)| [early, late])
            .map(|rover| (rover, standing_at(app, rover)))
            .collect()
    }

    /// Take the arc between `STRAIGHT`'s middle two tiles off the road, with the road tool.
    ///
    /// A right click is a secondary tap and a finish at once, which is what the mouse reports. The
    /// frame after it is the one that lays the roads the removal left, and neither carries a tick,
    /// so where a rover stands afterwards is where the removal left it rather than where it drove.
    fn remove_the_middle_arc(app: &mut App) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.ground_cursor_position = Some(middle_of_the_middle_arc());
            input.secondary_tap = true;
            input.finish = true;
        }
        advance(app, SHORT_FRAME);
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = false;
            input.finish = false;
        }
        advance(app, SHORT_FRAME);
    }

    /// Half way between `STRAIGHT`'s middle two tiles, which is the middle of the arc joining them.
    fn middle_of_the_middle_arc() -> Vec3 {
        let run = tiles(&STRAIGHT);
        (run[MIDDLE_ARC].world_position() + run[MIDDLE_ARC + 1].world_position()) / 2.
    }

    /// Whether `place` stands on the arc the removal under test takes.
    ///
    /// `STRAIGHT` runs along one row of the grid, so which of its arcs a place is on is which pair
    /// of tile centres it stands between, and nothing has to be measured against a curve.
    fn stands_on_the_middle_arc(place: Vec3) -> bool {
        let run = tiles(&STRAIGHT);
        let from = run[MIDDLE_ARC].world_position().x;
        let to = run[MIDDLE_ARC + 1].world_position().x;
        (from..=to).contains(&place.x)
    }

    /// The rovers of `stood` standing on the ground the removal under test leaves alone.
    fn off_the_arc_that_goes(stood: &[(Entity, Vec3)]) -> Vec<(Entity, Vec3)> {
        stood
            .iter()
            .copied()
            .filter(|&(_, was)| !stands_on_the_middle_arc(was))
            .collect()
    }

    /// The rovers of `stood` standing on the ground the removal under test takes away.
    fn on_the_arc_that_goes(stood: &[(Entity, Vec3)]) -> Vec<(Entity, Vec3)> {
        stood
            .iter()
            .copied()
            .filter(|&(_, was)| stands_on_the_middle_arc(was))
            .collect()
    }

    #[test]
    fn a_rover_the_removal_left_alone_stands_exactly_where_it_stood() {
        let mut app = a_road_to_edit();
        let stood = rovers_all_along_and_where_they_stand(&mut app);
        let left_alone = off_the_arc_that_goes(&stood);
        assert!(!left_alone.is_empty(), "no rover stood clear of the arc");

        remove_the_middle_arc(&mut app);

        for (rover, was) in left_alone {
            assert!(
                app.world().entities().contains(rover),
                "a rover went with an arc it was not standing on"
            );
            assert_eq!(
                standing_at(&app, rover),
                was,
                "a rover moved when an arc elsewhere on its road was removed"
            );
        }
    }

    #[test]
    fn a_rover_on_a_stretch_a_junction_cut_stands_exactly_where_it_stood() {
        let mut app = a_road_to_edit();
        cut_the_road_across(&mut app);
        let stood = rovers_all_along_and_where_they_stand(&mut app);
        let left_alone = off_the_arc_that_goes(&stood);
        assert!(!left_alone.is_empty(), "no rover stood clear of the arc");

        remove_the_middle_arc(&mut app);

        for (rover, was) in left_alone {
            assert!(
                app.world().entities().contains(rover),
                "a rover went with an arc it was not standing on"
            );
            assert_eq!(
                standing_at(&app, rover),
                was,
                "a rover moved when the road it stood on was laid again and cut again"
            );
        }
    }

    #[test]
    fn a_rover_the_removal_left_alone_is_still_on_a_stretch_of_road() {
        let mut app = a_road_to_edit();
        let stood = rovers_all_along_and_where_they_stand(&mut app);
        let left_alone = off_the_arc_that_goes(&stood);
        assert!(!left_alone.is_empty(), "no rover stood clear of the arc");

        remove_the_middle_arc(&mut app);

        for (rover, _) in left_alone {
            assert!(
                stands_on_its_segment(&app, rover),
                "a rover stands where no segment runs"
            );
        }
    }

    #[test]
    fn a_rover_the_removal_left_alone_drives_on() {
        let mut app = a_road_to_edit();
        let stood = rovers_all_along_and_where_they_stand(&mut app);
        let (driving, was) = *off_the_arc_that_goes(&stood)
            .first()
            .expect("no rover stood clear of the arc");

        remove_the_middle_arc(&mut app);
        tick(&mut app);

        assert_ne!(
            standing_at(&app, driving),
            was,
            "a rover the removal left alone stopped driving"
        );
    }

    #[test]
    fn a_rover_on_the_arc_a_removal_took_leaves_the_world() {
        let mut app = a_road_to_edit();
        let stood = rovers_all_along_and_where_they_stand(&mut app);
        let going = on_the_arc_that_goes(&stood);
        assert!(!going.is_empty(), "no rover stood on the arc that goes");

        remove_the_middle_arc(&mut app);

        for (rover, _) in going {
            assert!(
                !app.world().entities().contains(rover),
                "a rover outlived the arc it was standing on"
            );
        }
    }

    /// Put a rover `fraction` of the way along `segment`.
    fn set_down_on(app: &mut App, segment: Entity, fraction: f32) -> Entity {
        let along = place_along(app, segment, fraction);
        spawn_rover(app, segment, along)
    }

    /// How far along its arc a rover `fraction` of the way down `segment` has got.
    fn place_along(app: &App, segment: Entity, fraction: f32) -> f32 {
        let segment = app
            .world()
            .entity(segment)
            .get::<RoadSegment>()
            .expect("the segment is still there");
        segment.starts_at() + fraction * segment.length()
    }

    /// Whether `rover` stands on a stretch of road the segment it is on owns.
    fn stands_on_its_segment(app: &App, rover: Entity) -> bool {
        let (segment, along) = place_of(app, rover);
        (place_along(app, segment, 0.)..=place_along(app, segment, 1.)).contains(&along)
    }

    /// The rovers of `all` that were handed a different segment than the one they were set down on.
    fn handed_on(app: &App, all: &[(Entity, Entity, Entity)]) -> Vec<(Entity, Entity, Entity)> {
        all.iter()
            .filter(|&&(segment, _, late)| place_of(app, late).0 != segment)
            .copied()
            .collect()
    }

    #[test]
    fn a_rover_at_the_start_of_its_segment_stands_where_the_segment_starts() {
        let (app, rover, _) = road_and_rover(0.);
        let first = tiles(&STRAIGHT)[0].world_position();

        assert!(
            standing_at(&app, rover).distance(first) < TOLERANCE,
            "{} against the tile at {first}",
            standing_at(&app, rover)
        );
    }

    #[test]
    fn a_rover_half_way_along_a_straight_segment_stands_at_its_middle() {
        let (mut app, rover, segment) = road_and_rover(0.5);
        let (from, to) = ends_of(&mut app, segment);

        assert!(standing_at(&app, rover).distance(from.midpoint(to)) < TOLERANCE);
    }

    #[test]
    fn a_rover_moved_along_its_segment_moves_with_it() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let (from, _) = ends_of(&mut app, segment);

        let along = place_along(&app, segment, 0.75);
        move_to(&mut app, rover, along);
        advance(&mut app, SHORT_FRAME);

        assert!(standing_at(&app, rover).distance(from) > TOLERANCE);
    }

    #[test]
    fn a_rover_does_not_read_its_place_back_from_its_transform() {
        let (mut app, rover, segment) = road_and_rover(0.25);
        let (from, to) = ends_of(&mut app, segment);

        put_the_box_at(&mut app, rover, NOWHERE);
        advance(&mut app, SHORT_FRAME);

        assert_eq!(place_of(&app, rover).1, place_along(&app, segment, 0.25));
        assert!(standing_at(&app, rover).distance(from.lerp(to, 0.25)) < TOLERANCE);
    }

    #[test]
    fn a_rover_is_placed_on_a_frame_that_carries_no_tick() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let (from, to) = ends_of(&mut app, segment);

        let along = place_along(&app, segment, 1.);
        move_to(&mut app, rover, along);
        advance(&mut app, SHORT_FRAME);

        assert!(standing_at(&app, rover).distance(to) < TOLERANCE);
        assert!(standing_at(&app, rover).distance(from) > TOLERANCE);
    }

    #[test]
    fn a_rover_goes_with_the_road_removed_under_it() {
        let (mut app, rover, _) = road_and_rover(0.5);
        let road = the_road_in(&mut app);

        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert!(
            !app.world().entities().contains(rover),
            "a rover outlived the road it was standing on"
        );
    }

    #[test]
    fn a_rover_whose_road_ahead_is_removed_stops_where_the_road_stops() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let onward = next_of(&app, segment).expect("the lane runs on");
        let beyond = next_of(&app, onward).expect("the lane runs on");

        app.world_mut().entity_mut(beyond).despawn();
        for _ in 0..TICKS_PAST_THE_GAP {
            tick(&mut app);
        }

        assert_eq!(
            place_of(&app, rover),
            (onward, place_along(&app, onward, 1.))
        );
        assert_eq!(
            next_of(&app, onward),
            None,
            "a lane still runs into the gap"
        );
    }

    #[test]
    fn a_road_cut_across_leaves_every_rover_on_it_where_it_stood() {
        let mut app = road_app();
        let rovers = rovers_all_along(&mut app);
        advance(&mut app, SHORT_FRAME);
        let stood: Vec<(Entity, Vec3)> = rovers
            .iter()
            .flat_map(|&(_, early, late)| [early, late])
            .map(|rover| (rover, standing_at(&app, rover)))
            .collect();

        cut_the_road_across(&mut app);

        for (rover, was) in stood {
            assert_eq!(
                standing_at(&app, rover),
                was,
                "a rover moved when the road under it was cut"
            );
        }
    }

    #[test]
    fn a_rover_beyond_the_cut_is_handed_the_stretch_beyond_it() {
        let mut app = road_app();
        let rovers = rovers_all_along(&mut app);
        advance(&mut app, SHORT_FRAME);

        cut_the_road_across(&mut app);

        assert!(
            !handed_on(&app, &rovers).is_empty(),
            "the cut left every rover on the stretch it was standing on"
        );
    }

    #[test]
    fn a_rover_is_never_handed_a_stretch_it_has_not_reached() {
        let mut app = road_app();
        let rovers = rovers_all_along(&mut app);
        advance(&mut app, SHORT_FRAME);
        let stood: Vec<(Entity, Entity, f32)> = rovers
            .iter()
            .flat_map(|&(segment, early, late)| [(segment, early), (segment, late)])
            .map(|(segment, rover)| (segment, rover, place_along(&app, segment, 0.)))
            .collect();

        cut_the_road_across(&mut app);

        for (segment, rover, began) in stood {
            let (now, _) = place_of(&app, rover);
            if now == segment {
                continue;
            }
            assert!(
                place_along(&app, now, 0.) > began,
                "a rover was handed a stretch beginning behind the one it was on"
            );
        }
    }

    #[test]
    fn no_rover_stands_off_the_stretch_it_is_on_once_the_road_is_cut() {
        let mut app = road_app();
        let rovers = rovers_all_along(&mut app);
        advance(&mut app, SHORT_FRAME);

        cut_the_road_across(&mut app);

        for rover in rovers.iter().flat_map(|&(_, early, late)| [early, late]) {
            assert!(
                stands_on_its_segment(&app, rover),
                "a rover stands where no segment runs"
            );
        }
    }

    #[test]
    fn a_rover_is_given_a_box_to_be_seen_as() {
        let (app, rover, _) = road_and_rover(0.);

        let boxes = app
            .world()
            .entity(rover)
            .get::<Children>()
            .map(|children| {
                children
                    .iter()
                    .filter(|&child| app.world().entity(child).contains::<Mesh3d>())
                    .count()
            })
            .unwrap_or_default();

        assert_eq!(boxes, 1);
    }

    #[test]
    fn a_rovers_cargo_is_untouched_by_the_frames_that_place_it() {
        let (mut app, rover, segment) = road_and_rover(0.);
        app.world_mut()
            .entity_mut(rover)
            .insert(Cargo { quantity: 3 });

        let along = place_along(&app, segment, 0.5);
        move_to(&mut app, rover, along);
        advance(&mut app, SHORT_FRAME);
        advance(&mut app, SHORT_FRAME);

        let carried = app
            .world()
            .entity(rover)
            .get::<Cargo>()
            .map(|cargo| cargo.quantity);
        assert_eq!(carried, Some(3));
    }
    #[test]
    fn a_rover_advances_along_its_lane_on_the_tick() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let set_off = place_along(&app, segment, 0.);

        tick(&mut app);

        assert!(place_of(&app, rover).1 > set_off);
    }

    #[test]
    fn a_rover_does_not_advance_on_a_frame_that_carries_no_tick() {
        let (mut app, rover, segment) = road_and_rover(0.25);

        for _ in 0..FRAMES_WITHOUT_A_TICK {
            advance(&mut app, SHORT_FRAME);
        }

        assert_eq!(place_of(&app, rover).1, place_along(&app, segment, 0.25));
    }

    #[test]
    fn a_rover_crosses_a_segment_in_ticks_its_length_over_its_speed_limit() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let expected = length_of(&app, segment) / speed_limit_of(&app, segment);

        let taken = ticks_to_cross(&mut app, rover) as f32;

        assert!(
            (taken - expected).abs() <= 1.,
            "{taken} ticks against the {expected} its length over its limit asks for"
        );
    }

    #[test]
    fn a_rover_crosses_a_slower_segment_in_more_ticks() {
        let mut app = rover_app();
        lay_road(&mut app, &WINDING);
        tick(&mut app);
        let bend = slowest_segment_in(&mut app);
        let round_the_bend = spawn_rover(&mut app, bend, 0.);
        let ground = length_of(&app, bend);
        let open_road = the_open_road();

        let slowly = speed_limit_of(&app, bend);
        let taken = ticks_to_cross(&mut app, round_the_bend) as f32;

        assert!(slowly < open_road, "{slowly} round the bend is no slower");
        assert!(
            taken > ground / open_road,
            "{taken} ticks round the bend against the {} the same {ground} of straight takes",
            ground / open_road
        );
    }

    #[test]
    fn a_rover_hands_over_to_the_next_segment_at_the_end_of_this_one() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let onward = app
            .world()
            .entity(segment)
            .get::<NextSegment>()
            .expect("the lane runs on")
            .0;

        ticks_to_cross(&mut app, rover);

        assert_eq!(place_of(&app, rover).0, onward);
    }

    /// A straight road, and a road laid across the middle of it a frame later.
    fn a_crossed_road() -> App {
        let mut app = rover_app();
        lay_road(&mut app, &STRAIGHT);
        tick(&mut app);
        lay_road(&mut app, &CROSSING);
        tick(&mut app);
        app
    }

    /// A straight road and a road across it, laid on one frame, so neither gives way to the other.
    fn a_crossroads_taking_turns() -> App {
        let mut app = rover_app();
        lay_road(&mut app, &STRAIGHT);
        lay_road(&mut app, &CROSSING);
        tick(&mut app);
        app
    }

    /// A straight road, and a road ending on it a frame later.
    fn a_road_ending_on_another() -> App {
        let mut app = rover_app();
        lay_road(&mut app, &STRAIGHT);
        tick(&mut app);
        lay_road(&mut app, &SPUR);
        tick(&mut app);
        app
    }

    /// The segment that reaches the junction from `tile`, of which there is one.
    fn arriving_from(app: &mut App, offset: (i32, i32)) -> Entity {
        let from = HexCoordinates::from_offset_row(offset.0, offset.1).world_position();
        app.world_mut()
            .query::<(Entity, &RoadSegment, &EndsAtJunction)>()
            .iter(app.world())
            .min_by(|(_, one, _), (_, other, _)| {
                one.world_position(one.starts_at())
                    .distance(from)
                    .total_cmp(&other.world_position(other.starts_at()).distance(from))
            })
            .map(|(entity, ..)| entity)
            .expect("a segment reaching the junction")
    }

    /// Which leg of its junction `segment` arrives on.
    fn leg_of(app: &App, segment: Entity) -> usize {
        app.world()
            .entity(segment)
            .get::<EndsAtJunction>()
            .expect("the segment reaches a junction")
            .leg
    }

    /// The ways out of the junction `segment` reaches, for a rover arriving down it.
    fn ways_out_of(app: &App, segment: Entity) -> Vec<Entity> {
        let ends = app
            .world()
            .entity(segment)
            .get::<EndsAtJunction>()
            .expect("the segment reaches a junction");
        app.world()
            .entity(ends.junction)
            .get::<JunctionLegs>()
            .expect("the junction has legs")
            .exits_from(ends.leg)
    }

    /// Put a rover at the far end of `segment`, a tick short of the junction it reaches.
    fn waiting_on(app: &mut App, segment: Entity) -> Entity {
        let along = place_along(app, segment, ABOUT_TO_ARRIVE);
        spawn_rover(app, segment, along)
    }

    /// Every segment of the road laid through `offsets`, on either of its lanes.
    fn segments_of(app: &mut App, offsets: &[(i32, i32)]) -> Vec<Entity> {
        let wanted = tiles(offsets)[0].world_position();
        let roads: Vec<Vec<Entity>> = app
            .world_mut()
            .query_filtered::<&Children, With<Road>>()
            .iter(app.world())
            .map(|lanes| lanes.iter().collect())
            .collect();
        let pieces: Vec<Vec<Entity>> = roads
            .into_iter()
            .map(|lanes| {
                lanes
                    .into_iter()
                    .flat_map(|lane| children_of(app, lane))
                    .collect()
            })
            .collect();

        pieces
            .into_iter()
            .min_by(|one, other| {
                nearest_of(app, one, wanted).total_cmp(&nearest_of(app, other, wanted))
            })
            .expect("a road was laid")
    }

    fn children_of(app: &App, entity: Entity) -> Vec<Entity> {
        app.world()
            .entity(entity)
            .get::<Children>()
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    }

    /// How close to `wanted` the nearest of `pieces` sets off from.
    fn nearest_of(app: &App, pieces: &[Entity], wanted: Vec3) -> f32 {
        pieces
            .iter()
            .filter_map(|&piece| app.world().entity(piece).get::<RoadSegment>())
            .map(|segment| segment.world_position(segment.starts_at()).distance(wanted))
            .fold(f32::INFINITY, f32::min)
    }

    /// Set the world's tick so that the next one asks `leg` of a junction of `legs` first.
    fn ask_first(app: &mut App, leg: usize, legs: usize) {
        let wanted = (leg + legs - 1) % legs;
        app.world_mut().resource_mut::<Ticks>().0 = wanted as u64;
    }

    #[test]
    fn a_rover_reaching_a_junction_waits_a_tick_before_it_crosses() {
        let mut app = a_crossed_road();
        let arriving = arriving_from(&mut app, STRAIGHT[0]);
        let rover = waiting_on(&mut app, arriving);

        tick(&mut app);

        assert_eq!(
            place_of(&app, rover),
            (arriving, place_along(&app, arriving, 1.))
        );
    }

    #[test]
    fn a_rover_let_through_a_junction_leaves_by_a_way_out_it_allows() {
        let mut app = a_crossed_road();
        let arriving = arriving_from(&mut app, STRAIGHT[0]);
        let out = ways_out_of(&app, arriving);
        let rover = waiting_on(&mut app, arriving);

        tick(&mut app);
        tick(&mut app);

        let (standing, _) = place_of(&app, rover);
        assert!(
            out.contains(&standing),
            "a way out the junction does not allow"
        );
    }

    #[test]
    fn a_rover_on_a_road_that_ends_on_another_turns_onto_it_rather_than_back() {
        let mut app = a_road_ending_on_another();
        let arriving = arriving_from(&mut app, SPUR[0]);
        let spur = segments_of(&mut app, &SPUR);
        let rover = waiting_on(&mut app, arriving);

        tick(&mut app);
        tick(&mut app);

        let (standing, _) = place_of(&app, rover);
        assert!(
            !spur.contains(&standing),
            "a rover that turned back down its own road"
        );
    }

    #[test]
    fn two_rovers_reaching_a_junction_at_once_cross_on_different_ticks() {
        let mut app = a_crossed_road();
        let along = arriving_from(&mut app, STRAIGHT[0]);
        let across = arriving_from(&mut app, CROSSING[0]);
        let (one, other) = (waiting_on(&mut app, along), waiting_on(&mut app, across));

        tick(&mut app);
        tick(&mut app);

        assert_eq!(crossed(&app, &[(one, along), (other, across)]), 1);
        tick(&mut app);
        assert_eq!(crossed(&app, &[(one, along), (other, across)]), 2);
    }

    /// How many of the rovers have left the segment they were waiting on.
    fn crossed(app: &App, waiting: &[(Entity, Entity)]) -> usize {
        waiting
            .iter()
            .filter(|&&(rover, segment)| place_of(app, rover).0 != segment)
            .count()
    }

    #[test]
    fn a_rover_a_junction_holds_stays_at_the_end_of_its_segment() {
        let mut app = a_crossed_road();
        let along = arriving_from(&mut app, STRAIGHT[0]);
        let across = arriving_from(&mut app, CROSSING[0]);
        let (one, other) = (waiting_on(&mut app, along), waiting_on(&mut app, across));

        tick(&mut app);
        tick(&mut app);

        let held = [(one, along), (other, across)]
            .into_iter()
            .find(|&(rover, segment)| place_of(&app, rover).0 == segment)
            .expect("a rover was held");
        let end = place_along(&app, held.1, 1.);
        assert_eq!(place_of(&app, held.0), (held.1, end));
    }

    #[test]
    fn a_rover_gives_way_to_the_road_that_was_there_before_its_own() {
        let mut app = a_crossed_road();
        let along = arriving_from(&mut app, STRAIGHT[0]);
        let across = arriving_from(&mut app, CROSSING[0]);
        let (one, other) = (waiting_on(&mut app, along), waiting_on(&mut app, across));
        tick(&mut app);
        let asked = leg_of(&app, across);
        ask_first(&mut app, asked, LEGS_OF_A_CROSSROADS);

        tick(&mut app);

        assert_ne!(
            place_of(&app, one).0,
            along,
            "the road already there waited"
        );
        assert_eq!(
            place_of(&app, other).0,
            across,
            "the road drawn across went first"
        );
    }

    #[test]
    fn two_roads_laid_at_once_take_turns_at_the_junction_they_make() {
        let mut app = a_crossroads_taking_turns();
        let along = arriving_from(&mut app, STRAIGHT[0]);
        let across = arriving_from(&mut app, CROSSING[0]);
        let (one, other) = (waiting_on(&mut app, along), waiting_on(&mut app, across));
        tick(&mut app);
        let asked = leg_of(&app, across);
        ask_first(&mut app, asked, LEGS_OF_A_CROSSROADS);

        tick(&mut app);

        assert_ne!(
            place_of(&app, other).0,
            across,
            "the leg the tick asked waited"
        );
        assert_eq!(
            place_of(&app, one).0,
            along,
            "a leg the tick did not ask went first"
        );
    }

    #[test]
    fn which_rover_crosses_first_does_not_depend_on_which_was_spawned_first() {
        assert_eq!(first_leg_through(true), first_leg_through(false));
    }

    /// Which leg of a crossroads is let through first, the rover along the road spawned first or
    /// second according to `along_first`.
    ///
    /// The junction takes turns rather than giving way, because a leg with priority is served
    /// first whatever order the rovers reached the world in and would say nothing about this.
    fn first_leg_through(along_first: bool) -> usize {
        let mut app = a_crossroads_taking_turns();
        let along = arriving_from(&mut app, STRAIGHT[0]);
        let across = arriving_from(&mut app, CROSSING[0]);
        let (first, second) = if along_first {
            (along, across)
        } else {
            (across, along)
        };
        assert_ne!(along, across, "the two legs are the same segment");
        let (one, other) = (waiting_on(&mut app, first), waiting_on(&mut app, second));

        tick(&mut app);
        tick(&mut app);

        let (_, crossed) = [(one, first), (other, second)]
            .into_iter()
            .find(|&(rover, segment)| place_of(&app, rover).0 != segment)
            .expect("a rover crossed");
        leg_of(&app, crossed)
    }

    #[test]
    fn a_rover_held_at_a_junction_that_is_taken_away_drives_on() {
        let mut app = a_crossed_road();
        let arriving = arriving_from(&mut app, STRAIGHT[0]);
        let rover = waiting_on(&mut app, arriving);
        let across = crossing_road(&mut app);
        tick(&mut app);

        app.world_mut().entity_mut(across).despawn();
        tick(&mut app);
        tick(&mut app);

        assert_ne!(place_of(&app, rover).0, arriving, "a rover held at nothing");
    }

    /// The road laid across `STRAIGHT`, of which there is one.
    fn crossing_road(app: &mut App) -> Entity {
        let wanted = tiles(&CROSSING)[0].world_position();
        let roads: Vec<(Entity, Vec<Entity>)> = app
            .world_mut()
            .query_filtered::<(Entity, &Children), With<Road>>()
            .iter(app.world())
            .map(|(entity, lanes)| (entity, lanes.iter().collect()))
            .collect();

        roads
            .into_iter()
            .map(|(road, lanes)| {
                let pieces: Vec<Entity> = lanes
                    .into_iter()
                    .flat_map(|lane| children_of(app, lane))
                    .collect();
                (road, nearest_of(app, &pieces, wanted))
            })
            .min_by(|(_, one), (_, other)| one.total_cmp(other))
            .map(|(road, _)| road)
            .expect("a road was laid")
    }

    #[test]
    fn a_straight_road_delivers_more_than_a_winding_one_beside_it() {
        let mut app = rover_app();
        lay_road(&mut app, &STRAIGHT);
        lay_road(&mut app, &WINDING);
        tick(&mut app);
        let (on_the_straight, straight) = set_off_along(&mut app, &STRAIGHT);
        let (round_the_bend, winding) = set_off_along(&mut app, &WINDING);

        for _ in 0..TICKS_MEASURED {
            tick(&mut app);
        }

        let quick = driven_from(&app, on_the_straight, straight);
        let slow = driven_from(&app, round_the_bend, winding);
        assert!(
            quick > slow,
            "{quick} covered on the straight against {slow} round the bends"
        );
    }

    #[test]
    fn a_turn_through_a_junction_costs_the_arc_it_drives() {
        let turning = ticks_along(&[&APPROACH, &ACROSS], &APPROACH, PAST_THE_TURN);
        let straight_on = ticks_along(&[&STRAIGHT_ON], &STRAIGHT_ON, PAST_THE_STRAIGHT);

        assert!(
            turning > straight_on + TICKS_LOST_AT_A_JUNCTION,
            "{turning} ticks round the turn against {straight_on} straight on, over the same steps"
        );
    }

    #[test]
    fn carrying_straight_on_through_a_junction_costs_the_tick_it_waits_and_nothing_more() {
        let crossed = ticks_along(&[&STRAIGHT_ON, &CROSSING], &STRAIGHT_ON, PAST_THE_STRAIGHT);
        let clear = ticks_along(&[&STRAIGHT_ON], &STRAIGHT_ON, PAST_THE_STRAIGHT);

        assert!(
            crossed <= clear + TICKS_LOST_AT_A_JUNCTION,
            "{crossed} ticks across the junction against {clear} down the road nothing crosses"
        );
    }

    #[test]
    fn a_rover_put_where_an_endpoint_is_served_stands_on_the_road_node_serving_it() {
        let mut app = rover_app();
        let built_on = HexCoordinates::from_offset_row(0, 0);
        let corner = LatticeNode::nearest_on(
            built_on,
            built_on.world_position() + Vec3::Z * MAP_TILE_SIZE,
        );
        let endpoint = app.world_mut().spawn(RoadEndpoint::on(built_on)).id();
        app.world_mut().spawn(Road {
            nodes: vec![
                LatticeNode::from_tile(HexCoordinates::from_offset_row(0, 1)),
                corner,
            ],
            leaving: None,
            one_way: false,
        });
        tick(&mut app);

        let served = app
            .world()
            .entity(endpoint)
            .get::<RoadEndpoint>()
            .and_then(RoadEndpoint::served_by)
            .expect("the endpoint is served");
        let rover = app
            .world_mut()
            .spawn(Rover {
                segment: served.segment,
                along: served.along,
            })
            .id();
        advance(&mut app, SHORT_FRAME);

        let standing = app
            .world()
            .entity(rover)
            .get::<Transform>()
            .map(|transform| transform.translation)
            .expect("the rover stands somewhere");
        assert!(
            standing.distance(corner.world_position()) < TOLERANCE,
            "the rover stands at {standing}, not on the node {corner:?} serving the endpoint"
        );
    }

    /// The tile at `offset` of an offset-row layout.
    fn tile(offset: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offset.0, offset.1)
    }

    /// Lay a road between the corner of each of two tiles that faces the other.
    ///
    /// A road drawn through tile middles serves no tile at all, a middle never being a corner, so
    /// every road a delivery is tested on runs corner to corner and puts both tiles on the
    /// network.
    fn lay_road_between(app: &mut App, from: (i32, i32), to: (i32, i32)) {
        let (from, to) = (tile(from), tile(to));
        app.world_mut().spawn(Road {
            nodes: vec![
                LatticeNode::nearest_on(from, to.world_position()),
                LatticeNode::nearest_on(to, from.world_position()),
            ],
            leaving: None,
            one_way: false,
        });
    }

    /// An endpoint on `offset`'s tile, standing in for whatever is built there.
    fn endpoint_on(app: &mut App, offset: (i32, i32)) -> Entity {
        app.world_mut().spawn(RoadEndpoint::on(tile(offset))).id()
    }

    /// One road, and an endpoint at either end of it.
    fn a_road_between_endpoints() -> (App, Entity, Entity) {
        let mut app = rover_app();
        lay_road_between(&mut app, COLLECTION, DELIVERY);
        let collection = endpoint_on(&mut app, COLLECTION);
        let delivery = endpoint_on(&mut app, DELIVERY);
        tick(&mut app);
        (app, collection, delivery)
    }

    /// That road, crossed in its middle by a road to a third endpoint.
    fn a_crossroads_between_endpoints() -> (App, Entity, Entity) {
        let mut app = rover_app();
        lay_road_between(&mut app, COLLECTION, DELIVERY);
        lay_road_between(&mut app, ACROSS_FROM, ACROSS_TO);
        let collection = endpoint_on(&mut app, COLLECTION);
        let across = endpoint_on(&mut app, ACROSS_TO);
        tick(&mut app);
        (app, collection, across)
    }

    /// Where on the road `endpoint` is served, which is where a rover bound for it stops.
    fn served_place(app: &App, endpoint: Entity) -> ServedBy {
        app.world()
            .entity(endpoint)
            .get::<RoadEndpoint>()
            .and_then(RoadEndpoint::served_by)
            .expect("the endpoint is served")
    }

    /// Put a loaded rover where `collection` is served, routed to `delivery`.
    fn set_off_from(
        app: &mut App,
        collection: Entity,
        delivery: Entity,
        ways_out: Vec<Entity>,
    ) -> Entity {
        let from = served_place(app, collection);
        app.world_mut()
            .spawn((
                Rover {
                    segment: from.segment,
                    along: from.along,
                },
                Cargo { quantity: LOAD },
                Route {
                    destination: delivery,
                    ways_out,
                },
            ))
            .id()
    }

    /// The node of `tile` facing `towards`, which is a corner a road can serve the tile from.
    fn corner_facing(tile: (i32, i32), towards: (i32, i32)) -> LatticeNode {
        LatticeNode::nearest_on(
            HexCoordinates::from_offset_row(tile.0, tile.1),
            HexCoordinates::from_offset_row(towards.0, towards.1).world_position(),
        )
    }

    /// Lay one road of the fork, through the middle of each of `drawn`'s tiles.
    fn lay_a_road_of_the_fork(app: &mut App, drawn: &[LatticeNode], one_way: bool) {
        app.world_mut().spawn(Road {
            nodes: drawn.to_vec(),
            leaving: None,
            one_way,
        });
    }

    /// The road a delivery sets off down, from the collection tile's corner to where it forks.
    fn the_stem() -> Vec<LatticeNode> {
        let mut nodes = vec![corner_facing(FORK_FROM, THE_STEM[0])];
        nodes.extend(tiles(&THE_STEM).into_iter().map(LatticeNode::from_tile));
        nodes
    }

    /// The road running on from where the arms come together to the delivery tile's corner.
    fn the_run_out() -> Vec<LatticeNode> {
        let mut nodes: Vec<LatticeNode> = tiles(&THE_RUN_OUT)
            .into_iter()
            .map(LatticeNode::from_tile)
            .collect();
        nodes.push(corner_facing(FORK_TO, THE_RUN_OUT[THE_RUN_OUT.len() - 1]));
        nodes
    }

    /// One arm of the fork, as the player drew it.
    fn an_arm(drawn: &[(i32, i32)]) -> Vec<LatticeNode> {
        tiles(drawn)
            .into_iter()
            .map(LatticeNode::from_tile)
            .collect()
    }

    /// An endpoint at either end of the fork, once its roads are laid.
    fn endpoints_of_the_fork(app: &mut App) -> (Entity, Entity) {
        let collection = endpoint_on(app, FORK_FROM);
        let delivery = endpoint_on(app, FORK_TO);
        tick(app);
        (collection, delivery)
    }

    /// The fork, with both of its arms drivable both ways.
    fn a_fork_between_endpoints() -> (App, Entity, Entity) {
        let mut app = rover_app();
        lay_a_road_of_the_fork(&mut app, &the_stem(), false);
        lay_a_road_of_the_fork(&mut app, &an_arm(&THE_SHORT_ARM), false);
        lay_a_road_of_the_fork(&mut app, &an_arm(&THE_LONG_ARM), false);
        lay_a_road_of_the_fork(&mut app, &the_run_out(), false);
        let (collection, delivery) = endpoints_of_the_fork(&mut app);
        (app, collection, delivery)
    }

    /// How long the stretch of lane from `way_out` to the next junction is, and how long it takes.
    fn arm_from(app: &App, way_out: Entity) -> (f32, f32) {
        let (mut length, mut time) = (0., 0.);
        let mut at = way_out;
        for _ in 0..LAP_SEGMENTS {
            let segment = app
                .world()
                .entity(at)
                .get::<RoadSegment>()
                .expect("the arm is still there");
            length += segment.length();
            time += segment.length() / segment.speed_limit();
            if app.world().entity(at).contains::<EndsAtJunction>() {
                break;
            }
            match next_of(app, at) {
                Some(onward) => at = onward,
                None => break,
            }
        }
        (length, time)
    }

    /// Where each way out of the route `rover` holds comes out, which is the shape of the route.
    ///
    /// Where rather than which, because two networks built from the same roads in a different
    /// order hold different entities standing in the same places, and it is the places that say
    /// whether the same way through was taken.
    fn shape_of_the_route(app: &App, rover: Entity) -> Vec<Vec3> {
        route_of(app, rover)
            .expect("the rover was given a route")
            .into_iter()
            .map(|way_out| reach_of(app, way_out))
            .collect()
    }

    /// A network of roads crossing each other, and an endpoint at opposite corners of it.
    fn a_large_network() -> (App, Entity, Entity) {
        let mut app = rover_app();
        let far = (ROADS_EACH_WAY - 1) * ROAD_SPACING;
        for step in 0..ROADS_EACH_WAY {
            let at = step * ROAD_SPACING;
            lay_road_between(&mut app, (ROAD_BEGINS_AT, at), (ROAD_ENDS_AT, at));
            lay_road_between(&mut app, (at, ROAD_BEGINS_AT), (at, ROAD_ENDS_AT));
        }
        let collection = endpoint_on(&mut app, (ROAD_BEGINS_AT, 0));
        let delivery = endpoint_on(&mut app, (ROAD_ENDS_AT, far));
        tick(&mut app);
        (app, collection, delivery)
    }

    /// How many stretches of road the world holds.
    fn segments_in_the_world(app: &mut App) -> usize {
        app.world_mut()
            .query::<&RoadSegment>()
            .iter(app.world())
            .count()
    }

    /// Put a loaded rover where `collection` is served, sent to `delivery` to find its own way.
    fn send_from(app: &mut App, collection: Entity, delivery: Entity) -> Entity {
        let from = served_place(app, collection);
        app.world_mut()
            .spawn((
                Rover {
                    segment: from.segment,
                    along: from.along,
                },
                Cargo { quantity: LOAD },
                SentTo(delivery),
            ))
            .id()
    }

    /// The ways out `rover` is holding, or nothing at all where it was given no route.
    fn route_of(app: &App, rover: Entity) -> Option<Vec<Entity>> {
        app.world()
            .entity(rover)
            .get::<Route>()
            .map(|route| route.ways_out.clone())
    }

    /// Whether `rover` is still carrying an order for a route.
    fn is_still_asking(app: &App, rover: Entity) -> bool {
        app.world().entity(rover).contains::<SentTo>()
    }

    /// How much `entity` is carrying or holding, which is nothing while it holds no load at all.
    fn load_of(app: &App, entity: Entity) -> u32 {
        app.world()
            .entity(entity)
            .get::<Cargo>()
            .map_or(0, |cargo| cargo.quantity)
    }

    /// The tick a load reached `endpoint` on, or nothing if none ever did.
    fn tick_delivered_on(app: &mut App, endpoint: Entity) -> Option<u64> {
        for _ in 0..TICKS_TO_DELIVER {
            tick(app);
            if load_of(app, endpoint) > 0 {
                return Some(app.world().resource::<Ticks>().0);
            }
        }
        None
    }

    fn is_stranded(app: &App, rover: Entity) -> bool {
        app.world().entity(rover).contains::<Stranded>()
    }

    /// Where the far end of `segment` stands.
    fn reach_of(app: &App, segment: Entity) -> Vec3 {
        let segment = app
            .world()
            .entity(segment)
            .get::<RoadSegment>()
            .expect("the segment is still there");
        segment.world_position(segment.ends_at())
    }

    /// The way out of the junction `arriving` reaches that sets off towards `towards`.
    fn way_out_towards(app: &App, arriving: Entity, towards: (i32, i32)) -> Entity {
        let wanted = tile(towards).world_position();
        ways_out_of(app, arriving)
            .into_iter()
            .min_by(|one, other| {
                reach_of(app, *one)
                    .distance(wanted)
                    .total_cmp(&reach_of(app, *other).distance(wanted))
            })
            .expect("the junction has a way out")
    }

    #[test]
    fn a_rover_on_a_route_stops_where_its_destination_endpoint_stands() {
        let (mut app, collection, delivery) = a_road_between_endpoints();
        let rover = set_off_from(&mut app, collection, delivery, Vec::new());
        let stops = served_place(&app, delivery);

        tick_delivered_on(&mut app, delivery).expect("the delivery lands");
        for _ in 0..TICKS_GOING_NOWHERE {
            tick(&mut app);
        }

        assert_eq!(place_of(&app, rover), (stops.segment, stops.along));
    }

    #[test]
    fn a_rover_leaves_its_load_at_the_endpoint_it_was_routed_to() {
        let (mut app, collection, delivery) = a_road_between_endpoints();
        set_off_from(&mut app, collection, delivery, Vec::new());

        tick_delivered_on(&mut app, delivery).expect("the delivery lands");

        assert_eq!(load_of(&app, delivery), LOAD);
    }

    #[test]
    fn a_rover_that_has_delivered_carries_nothing_on() {
        let (mut app, collection, delivery) = a_road_between_endpoints();
        let rover = set_off_from(&mut app, collection, delivery, Vec::new());

        tick_delivered_on(&mut app, delivery).expect("the delivery lands");

        assert_eq!(load_of(&app, rover), 0);
    }

    #[test]
    fn a_delivery_does_not_advance_on_frames_that_carry_no_tick() {
        let (mut app, collection, delivery) = a_road_between_endpoints();
        let rover = set_off_from(&mut app, collection, delivery, Vec::new());
        let set_off = place_of(&app, rover);

        for _ in 0..FRAMES_WITHOUT_A_TICK {
            advance(&mut app, SHORT_FRAME);
        }

        assert_eq!(place_of(&app, rover), set_off);
        assert_eq!(load_of(&app, delivery), 0);
    }

    #[test]
    fn a_rover_routed_through_a_junction_delivers_down_the_road_its_route_names() {
        let (mut app, collection, across) = a_crossroads_between_endpoints();
        let arriving = arriving_from(&mut app, COLLECTION);
        let turn = way_out_towards(&app, arriving, ACROSS_TO);
        set_off_from(&mut app, collection, across, vec![turn]);

        tick_delivered_on(&mut app, across).expect("the delivery lands");

        assert_eq!(load_of(&app, across), LOAD);
    }

    #[test]
    fn a_rover_whose_route_names_a_way_out_the_junction_refuses_is_stranded() {
        let (mut app, collection, across) = a_crossroads_between_endpoints();
        let arriving = arriving_from(&mut app, COLLECTION);
        let rover = set_off_from(&mut app, collection, across, vec![arriving]);

        for _ in 0..TICKS_GOING_NOWHERE {
            tick(&mut app);
        }

        assert!(
            is_stranded(&app, rover),
            "a rover the junction let take a turn it does not allow"
        );
        assert_eq!(load_of(&app, across), 0);
    }

    #[test]
    fn a_rover_routed_to_an_endpoint_no_road_reaches_is_stranded() {
        let (mut app, collection, _) = a_road_between_endpoints();
        let nowhere = endpoint_on(&mut app, OFF_THE_NETWORK);
        let rover = set_off_from(&mut app, collection, nowhere, Vec::new());
        let set_off = place_of(&app, rover);

        for _ in 0..TICKS_GOING_NOWHERE {
            tick(&mut app);
        }

        assert!(is_stranded(&app, rover));
        assert_eq!(
            place_of(&app, rover),
            set_off,
            "a rover that set off down a route it has not got"
        );
    }

    #[test]
    fn a_rover_whose_road_runs_out_before_its_destination_is_stranded() {
        let (mut app, collection, delivery) = a_road_between_endpoints();
        let rover = set_off_from(&mut app, collection, delivery, Vec::new());
        let (setting_off, _) = place_of(&app, rover);
        let onward = next_of(&app, setting_off).expect("the lane runs on");
        let beyond = next_of(&app, onward).expect("the lane runs on");

        app.world_mut().entity_mut(beyond).despawn();
        for _ in 0..TICKS_PAST_THE_GAP {
            tick(&mut app);
        }

        assert!(is_stranded(&app, rover));
        assert_eq!(load_of(&app, delivery), 0);
    }

    #[test]
    fn a_rover_sent_to_an_endpoint_finds_its_own_way_there() {
        let (mut app, collection, across) = a_crossroads_between_endpoints();
        send_from(&mut app, collection, across);

        tick_delivered_on(&mut app, across).expect("the delivery lands");

        assert_eq!(load_of(&app, across), LOAD);
    }

    #[test]
    fn a_route_is_found_once_rather_than_on_every_tick_of_the_journey() {
        let (mut app, collection, across) = a_crossroads_between_endpoints();
        let rover = send_from(&mut app, collection, across);

        tick(&mut app);
        assert!(
            route_of(&app, rover).is_some(),
            "the rover set off without a route"
        );

        for _ in 0..TICKS_TO_DELIVER {
            assert!(
                !is_still_asking(&app, rover),
                "a rover asking again for the route it is already driving"
            );
            if load_of(&app, across) > 0 {
                return;
            }
            tick(&mut app);
        }
        panic!("the delivery never landed");
    }

    #[test]
    fn a_rover_sent_to_an_endpoint_no_road_reaches_is_stranded_rather_than_routed() {
        let (mut app, collection, _) = a_road_between_endpoints();
        let nowhere = endpoint_on(&mut app, OFF_THE_NETWORK);
        let rover = send_from(&mut app, collection, nowhere);

        tick(&mut app);

        assert!(is_stranded(&app, rover));
        assert!(
            route_of(&app, rover).is_none(),
            "a rover handed part of a route to somewhere it cannot go"
        );
    }

    #[test]
    fn a_rover_sent_across_a_gap_in_the_network_is_stranded_rather_than_half_routed() {
        let (mut app, collection, _) = a_road_between_endpoints();
        lay_road_between(&mut app, ELSEWHERE_FROM, ELSEWHERE_TO);
        let elsewhere = endpoint_on(&mut app, ELSEWHERE_TO);
        tick(&mut app);
        let rover = send_from(&mut app, collection, elsewhere);

        tick(&mut app);

        assert!(is_stranded(&app, rover));
        assert!(
            route_of(&app, rover).is_none(),
            "a rover handed a route that stops at the gap"
        );
    }

    #[test]
    fn a_rover_sent_where_it_already_stands_is_given_a_route_with_nothing_to_choose() {
        let (mut app, _, delivery) = a_road_between_endpoints();
        let stops = served_place(&app, delivery);
        let rover = app
            .world_mut()
            .spawn((
                Rover {
                    segment: stops.segment,
                    along: stops.along,
                },
                Cargo { quantity: LOAD },
                SentTo(delivery),
            ))
            .id();

        tick(&mut app);

        assert_eq!(route_of(&app, rover), Some(Vec::new()));
        assert_eq!(load_of(&app, delivery), LOAD);
    }

    #[test]
    fn the_route_found_is_the_quickest_way_rather_than_the_shortest() {
        let (mut app, collection, delivery) = a_fork_between_endpoints();
        let arriving = arriving_from(&mut app, FORK_FROM);
        let short_way = way_out_towards(&app, arriving, THE_SHORT_WAY);
        let long_way = way_out_towards(&app, arriving, THE_LONG_WAY);
        let (short, slowly) = arm_from(&app, short_way);
        let (long, quickly) = arm_from(&app, long_way);
        assert!(short < long, "the fork has no shorter arm to be tempted by");
        assert!(quickly < slowly, "the longer arm is not the quicker one");

        let rover = send_from(&mut app, collection, delivery);
        tick(&mut app);

        assert_eq!(
            route_of(&app, rover).as_deref().and_then(<[Entity]>::first),
            Some(&long_way),
            "the rover took the shorter arm rather than the quicker one"
        );
        tick_delivered_on(&mut app, delivery).expect("the delivery lands");
    }

    #[test]
    fn a_route_does_not_run_against_a_one_way_road() {
        let mut app = rover_app();
        lay_a_road_of_the_fork(&mut app, &the_stem(), false);
        lay_a_road_of_the_fork(&mut app, &an_arm(&THE_SHORT_ARM), false);
        lay_a_road_of_the_fork(&mut app, &an_arm(&THE_LONG_ARM_REVERSED), true);
        lay_a_road_of_the_fork(&mut app, &the_run_out(), false);
        let (collection, delivery) = endpoints_of_the_fork(&mut app);
        let arriving = arriving_from(&mut app, FORK_FROM);
        let short_way = way_out_towards(&app, arriving, THE_SHORT_WAY);

        let rover = send_from(&mut app, collection, delivery);
        tick(&mut app);

        assert_eq!(
            route_of(&app, rover).as_deref().and_then(<[Entity]>::first),
            Some(&short_way),
            "the rover set off the wrong way down a one-way arm"
        );
        tick_delivered_on(&mut app, delivery).expect("the delivery lands");
    }

    #[test]
    fn the_route_found_is_the_same_however_the_network_was_laid() {
        let (mut drawn_in_order, collection, delivery) = a_fork_between_endpoints();
        let rover = send_from(&mut drawn_in_order, collection, delivery);
        tick(&mut drawn_in_order);
        let taken = shape_of_the_route(&drawn_in_order, rover);

        let mut drawn_the_other_way = rover_app();
        lay_a_road_of_the_fork(&mut drawn_the_other_way, &the_run_out(), false);
        lay_a_road_of_the_fork(&mut drawn_the_other_way, &an_arm(&THE_LONG_ARM), false);
        lay_a_road_of_the_fork(&mut drawn_the_other_way, &an_arm(&THE_SHORT_ARM), false);
        lay_a_road_of_the_fork(&mut drawn_the_other_way, &the_stem(), false);
        let (collection, delivery) = endpoints_of_the_fork(&mut drawn_the_other_way);
        let rover = send_from(&mut drawn_the_other_way, collection, delivery);
        tick(&mut drawn_the_other_way);

        let also_taken = shape_of_the_route(&drawn_the_other_way, rover);
        assert_eq!(
            also_taken.len(),
            taken.len(),
            "the two networks were routed through a different number of junctions"
        );
        for (one, other) in also_taken.iter().zip(&taken) {
            assert!(
                one.distance(*other) < TOLERANCE,
                "the two networks were routed down different ways out: {one} and {other}"
            );
        }
    }

    #[test]
    fn a_route_is_found_across_a_network_of_thousands_of_segments() {
        let (mut app, collection, delivery) = a_large_network();
        assert!(
            segments_in_the_world(&mut app) > A_LARGE_NETWORK,
            "the network is too small to say anything about a large one"
        );
        let rover = send_from(&mut app, collection, delivery);

        tick(&mut app);

        let route = route_of(&app, rover).expect("the rover was given a route");
        assert!(
            !route.is_empty(),
            "a route across the network with nothing to choose in it"
        );
        assert!(!is_stranded(&app, rover));
    }

    #[test]
    fn the_same_delivery_arrives_on_the_same_tick_twice() {
        let (mut once, from, to) = a_road_between_endpoints();
        set_off_from(&mut once, from, to, Vec::new());
        let first = tick_delivered_on(&mut once, to).expect("the delivery lands");

        let (mut again, from, to) = a_road_between_endpoints();
        set_off_from(&mut again, from, to, Vec::new());
        let second = tick_delivered_on(&mut again, to).expect("the delivery lands");

        assert_eq!(first, second);
    }

    #[test]
    fn a_delivery_arrives_on_the_same_tick_at_half_the_frame_rate() {
        let (mut steady, from, to) = a_road_between_endpoints();
        set_off_from(&mut steady, from, to, Vec::new());
        let on_the_tick = tick_delivered_on(&mut steady, to).expect("the delivery lands");

        let (mut halved, from, to) = a_road_between_endpoints();
        set_off_from(&mut halved, from, to, Vec::new());
        let half_a_tick = halved.world().resource::<Time<Fixed>>().timestep() / 2;
        let mut delivered = None;
        for _ in 0..TICKS_TO_DELIVER * 2 {
            advance(&mut halved, half_a_tick);
            if load_of(&halved, to) > 0 {
                delivered = Some(halved.world().resource::<Ticks>().0);
                break;
            }
        }

        assert_eq!(delivered, Some(on_the_tick));
    }

    /// Where one rover stands and what it is doing, as the tick left it.
    #[derive(Clone, Debug, PartialEq)]
    struct Standing {
        segment: Entity,
        along: f32,
        waiting: bool,
        stranded: bool,
        load: u32,
    }

    /// The traffic on the map, in an order nothing about how the world stores it can decide.
    ///
    /// Sorted by where a rover is rather than by which entity it is, so two runs that spawned the
    /// same rovers in a different order still compare entry for entry, and a rover picking up a
    /// component cannot reorder the reading by moving to another archetype.
    fn traffic(world: &World) -> Vec<Standing> {
        let mut standing: Vec<Standing> = world
            .iter_entities()
            .filter_map(|entity| {
                let rover = entity.get::<Rover>()?;
                Some(Standing {
                    segment: rover.segment,
                    along: rover.along,
                    waiting: entity.contains::<WaitingAtJunction>(),
                    stranded: entity.contains::<Stranded>(),
                    load: entity.get::<Cargo>().map_or(0, |cargo| cargo.quantity),
                })
            })
            .collect();
        standing.sort_by(|one, other| {
            one.segment
                .cmp(&other.segment)
                .then(one.along.total_cmp(&other.along))
        });
        standing
    }

    /// A crossroads with a delivery running through it and a rover coming the other way.
    ///
    /// Both roads carry a rover into the junction, so the tick has a choice to make, and the one
    /// on a route has a load to hand over on the far side of that choice.
    fn a_crossroads_under_traffic() -> App {
        let (mut app, collection, across) = a_crossroads_between_endpoints();
        let arriving = arriving_from(&mut app, COLLECTION);
        let turn = way_out_towards(&app, arriving, ACROSS_TO);
        set_off_from(&mut app, collection, across, vec![turn]);

        let crossing = arriving_from(&mut app, ACROSS_FROM);
        let sets_off = place_along(&app, crossing, 0.);
        spawn_rover(&mut app, crossing, sets_off);
        app
    }

    /// A straight road with a rover set down `along` the segment it sets off from.
    fn a_rover_set_down_at(along: f32) -> App {
        let mut app = road_app();
        let segment = segment_from(&mut app, tiles(&STRAIGHT)[0]);
        spawn_rover(&mut app, segment, along);
        app
    }

    /// A crossroads holding a rover at the end of the leg reaching it from each tile in turn.
    ///
    /// Both are set down where their segment ends, so both are marked as waiting on the same tick
    /// with the same arrival and neither has waited longer than the other. Which of them goes is
    /// a tie, and the order they are spawned in is the only thing two runs of this differ by.
    fn two_rovers_held_at_a_junction(first: (i32, i32), second: (i32, i32)) -> App {
        let (mut app, ..) = a_crossroads_between_endpoints();
        for approach in [first, second] {
            let leg = arriving_from(&mut app, approach);
            let waits_at = place_along(&app, leg, AT_THE_JUNCTION);
            spawn_rover(&mut app, leg, waits_at);
        }
        app
    }

    #[test]
    fn a_trace_tells_two_worlds_that_set_off_differently_apart() {
        let early = a_rover_set_down_at(EARLY_ALONG);
        let late = a_rover_set_down_at(LATE_ALONG);

        let early = trace(early, &A_TICK_A_FRAME, TICKS_TRACED, traffic);

        assert_ne!(early, trace(late, &A_TICK_A_FRAME, TICKS_TRACED, traffic));
    }

    #[test]
    fn the_same_world_run_twice_gives_the_same_traffic() {
        let once = trace(
            a_crossroads_under_traffic(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        let again = trace(
            a_crossroads_under_traffic(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        assert_eq!(once, again);
    }

    #[test]
    fn a_world_drawn_five_times_a_tick_gives_the_same_traffic() {
        let steady = trace(
            a_crossroads_under_traffic(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        let often = trace(
            a_crossroads_under_traffic(),
            &FIVE_FRAMES_A_TICK,
            TICKS_TRACED,
            traffic,
        );

        assert_eq!(steady, often);
    }

    #[test]
    fn a_world_drawn_once_in_four_ticks_gives_the_same_traffic() {
        let steady = trace(
            a_crossroads_under_traffic(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        let seldom = trace(
            a_crossroads_under_traffic(),
            &FOUR_TICKS_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        assert_eq!(steady, seldom);
    }

    #[test]
    fn a_world_drawn_on_ragged_frames_gives_the_same_traffic() {
        let steady = trace(
            a_crossroads_under_traffic(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            traffic,
        );

        let ragged = trace(
            a_crossroads_under_traffic(),
            &RAGGED_FRAMES,
            TICKS_TRACED,
            traffic,
        );

        assert_eq!(steady, ragged);
    }

    #[test]
    fn a_tie_at_a_junction_outlives_the_order_the_rovers_were_spawned_in() {
        let one_way_round = two_rovers_held_at_a_junction(COLLECTION, ACROSS_FROM);
        let the_other = two_rovers_held_at_a_junction(ACROSS_FROM, COLLECTION);

        let one_way_round = trace(one_way_round, &A_TICK_A_FRAME, TICKS_TRACED, traffic);

        assert_eq!(
            one_way_round,
            trace(the_other, &A_TICK_A_FRAME, TICKS_TRACED, traffic)
        );
    }
}
