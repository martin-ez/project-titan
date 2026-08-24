use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::map::MAP_TILE_SIZE;
use crate::road::{NextSegment, RoadSegment};
use crate::simulation::Simulation;
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

/// The rovers on the map, and where on the road each of them stands.
///
/// A rover is where the road stops being scenery: everything a building receives arrives on one
/// (invariant 1). This is only the entity and its place — it sits on a segment and it is somewhere
/// along it. Nothing here moves it.
pub struct RoverPlugin;

/// A rover, standing somewhere along the segment it is driving.
///
/// Where it is is the fraction, and the `Vec3` it stands at is derived from that and the segment's
/// geometry every frame (invariant 3). Nothing reads the transform back to work out where the
/// rover got to, so the arc's `sin` and `cos` never reach the simulation and a chain jams the same
/// way on every machine (invariant 2).
#[derive(Component)]
#[require(Transform, Visibility = Visibility::Hidden, NeedsInitialization)]
pub struct Rover {
    /// The segment the rover is driving.
    pub segment: Entity,
    /// How far along that segment it has got, from `0` at the start to `1` at the end.
    pub along: f32,
}

/// What a rover is carrying, which is a quantity of nothing in particular.
///
/// Goods and recipes are #26's, and they come after traffic. Until then a load is opaque: enough
/// for something to change hands and for a jam to be worth watching, and not enough for a
/// production chain to be built on top of it. A rover carrying nothing has no `Cargo` at all.
#[derive(Component)]
pub struct Cargo {
    /// How much of it there is.
    pub quantity: u32,
}

#[derive(SystemParam)]
struct RoverInitializeParams<'w, 's> {
    query: Query<'w, 's, &'static mut Visibility, With<Rover>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

impl Plugin for RoverPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, initialize_system::<Rover, RoverInitializeParams>)
            .add_systems(FixedUpdate, drive_the_rovers.in_set(Simulation))
            .add_systems(
                Update,
                (stand_the_rovers_on_their_segments, draw_the_rovers).chain(),
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

/// Drive every rover along its lane, at whatever each segment it crosses allows.
///
/// A tick buys a rover an amount of time rather than an amount of ground, and it is spent segment
/// by segment: what is left of the tick when a rover reaches the end of one is carried onto the
/// next and spent at the next one's speed limit, so a rover joining a curve slows down on the
/// curve rather than a tick early. A rover that runs out of road stops at the end of it, and one
/// whose segment is gone is left where it is — what should become of that one is #102's.
fn drive_the_rovers(
    mut rovers: Query<&mut Rover>,
    segments: Query<(&RoadSegment, Option<&NextSegment>)>,
) {
    for mut rover in &mut rovers {
        let mut left = 1.;
        for _ in 0..HANDOVERS_PER_TICK {
            let Ok((segment, next)) = segments.get(rover.segment) else {
                break;
            };
            let length = segment.length();
            let crossing = (1. - rover.along) * length / segment.speed_limit();
            if crossing > left {
                rover.along += left * segment.speed_limit() / length;
                break;
            }

            left -= crossing;
            match next {
                Some(next) => {
                    rover.segment = next.0;
                    rover.along = 0.;
                }
                None => {
                    rover.along = 1.;
                    break;
                }
            }
        }
    }
}

/// Put every rover where its distance along its segment says it stands.
///
/// This is presentation and runs on the frame: it reads the simulation's fraction and writes a
/// transform, and nothing on the tick reads what it wrote. A rover whose segment is gone is left
/// where it was rather than taking the game down — what should become of it is #102's.
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
            segment.world_position(0.) + GIZMO_LIFT,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode};
    use crate::road::{Road, RoadPlugin};
    use crate::simulation::SimulationPlugin;
    use crate::testing::{advance, headless_app, tick};
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

    fn rover_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::Select)
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
            .find(|(_, segment)| segment.world_position(0.).distance(target) < TOLERANCE)
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
        (segment.world_position(0.), segment.world_position(1.))
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
                return driven + along * length;
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

    /// A road laid, a rover put on its first segment, and a frame for both to be seen.
    ///
    /// The frame carries no tick, so the rover is placed where it was put rather than where a
    /// tick of driving would have taken it: these are the tests of what a box on a lane shows.
    fn road_and_rover(along: f32) -> (App, Entity, Entity) {
        let mut app = road_app();
        let segment = segment_from(&mut app, tiles(&STRAIGHT)[0]);
        let rover = spawn_rover(&mut app, segment, along);
        advance(&mut app, SHORT_FRAME);
        (app, rover, segment)
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

        move_to(&mut app, rover, 0.75);
        advance(&mut app, SHORT_FRAME);

        assert!(standing_at(&app, rover).distance(from) > TOLERANCE);
    }

    #[test]
    fn a_rover_does_not_read_its_place_back_from_its_transform() {
        let (mut app, rover, segment) = road_and_rover(0.25);
        let (from, to) = ends_of(&mut app, segment);

        put_the_box_at(&mut app, rover, NOWHERE);
        advance(&mut app, SHORT_FRAME);

        let along = app
            .world()
            .entity(rover)
            .get::<Rover>()
            .expect("the rover is still there")
            .along;
        assert_eq!(along, 0.25);
        assert!(standing_at(&app, rover).distance(from.lerp(to, 0.25)) < TOLERANCE);
    }

    #[test]
    fn a_rover_is_placed_on_a_frame_that_carries_no_tick() {
        let (mut app, rover, segment) = road_and_rover(0.);
        let (from, to) = ends_of(&mut app, segment);

        move_to(&mut app, rover, 1.);
        advance(&mut app, SHORT_FRAME);

        assert!(standing_at(&app, rover).distance(to) < TOLERANCE);
        assert!(standing_at(&app, rover).distance(from) > TOLERANCE);
    }

    #[test]
    fn a_rover_whose_segment_is_gone_stays_where_it_was() {
        let (mut app, rover, _) = road_and_rover(0.5);
        let stood = standing_at(&app, rover);

        let road = app
            .world_mut()
            .query_filtered::<Entity, With<Road>>()
            .iter(app.world())
            .next()
            .expect("the road is still there");
        app.world_mut().entity_mut(road).despawn();
        tick(&mut app);

        assert_eq!(standing_at(&app, rover), stood);
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
        let (mut app, rover, _) = road_and_rover(0.);
        app.world_mut()
            .entity_mut(rover)
            .insert(Cargo { quantity: 3 });

        move_to(&mut app, rover, 0.5);
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
        let (mut app, rover, _) = road_and_rover(0.);

        tick(&mut app);

        assert!(place_of(&app, rover).1 > 0.);
    }

    #[test]
    fn a_rover_does_not_advance_on_a_frame_that_carries_no_tick() {
        let (mut app, rover, _) = road_and_rover(0.25);

        for _ in 0..FRAMES_WITHOUT_A_TICK {
            advance(&mut app, SHORT_FRAME);
        }

        assert_eq!(place_of(&app, rover).1, 0.25);
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
}
