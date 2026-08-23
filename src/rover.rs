use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::map::MAP_TILE_SIZE;
use crate::road::RoadSegment;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// How long and wide the box standing in for a rover is.
const ROVER_SIZE: f32 = MAP_TILE_SIZE / 5.;

/// How tall the box standing in for a rover is.
const ROVER_HEIGHT: f32 = MAP_TILE_SIZE / 10.;

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
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode};
    use crate::road::{Road, RoadPlugin};
    use crate::testing::{advance, headless_app, tick};
    use std::time::Duration;

    /// How closely two world positions have to agree to be the same place.
    const TOLERANCE: f32 = 1e-3;

    /// A straight run of tiles, in offset-row coordinates.
    ///
    /// It sets off away from the origin, so a rover that was never placed at all stands somewhere
    /// its segment does not run through rather than at the first tile by coincidence.
    const STRAIGHT: [(i32, i32); 4] = [(1, 0), (2, 0), (3, 0), (4, 0)];

    /// A frame far too short to carry a tick of the fixed clock.
    const SHORT_FRAME: Duration = Duration::from_micros(100);

    /// Somewhere no segment of the road under test runs through.
    const NOWHERE: Vec3 = Vec3::new(999., 999., 999.);

    fn rover_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::Select)
            .insert_resource(PlayerInput::default())
            .add_plugins((DebugGizmosPlugin, CleanupPlugin, RoadPlugin, RoverPlugin));
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
    fn road_and_rover(along: f32) -> (App, Entity, Entity) {
        let mut app = road_app();
        let segment = segment_from(&mut app, tiles(&STRAIGHT)[0]);
        let rover = spawn_rover(&mut app, segment, along);
        tick(&mut app);
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
        tick(&mut app);

        assert!(standing_at(&app, rover).distance(from) > TOLERANCE);
    }

    #[test]
    fn a_rover_does_not_read_its_place_back_from_its_transform() {
        let (mut app, rover, segment) = road_and_rover(0.25);
        let (from, to) = ends_of(&mut app, segment);

        put_the_box_at(&mut app, rover, NOWHERE);
        tick(&mut app);

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
        tick(&mut app);
        tick(&mut app);

        let carried = app
            .world()
            .entity(rover)
            .get::<Cargo>()
            .map(|cargo| cargo.quantity);
        assert_eq!(carried, Some(3));
    }
}
