use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::map::HexCoordinates;
use bevy::ecs::system::SystemParam;
use bevy::math::cubic_splines::InsufficientDataError;
use bevy::prelude::*;

/// How many straight pieces a segment's spline is drawn as.
const SEGMENT_SUBDIVISIONS: u32 = 8;

/// How far into a segment the arrow onto the next one reaches, at either end of the handover.
const HANDOVER_REACH: f32 = 0.1;

/// The colour a lane's spline is drawn in
const LANE_COLOUR: Color = Color::srgb(0.35, 0.75, 0.95);

/// The colour the step from one segment onto the next is drawn in
const HANDOVER_COLOUR: Color = Color::srgb(0.95, 0.8, 0.3);

/// The roads on the map, and the lanes a rover drives on them.
///
/// A road carries one lane in each direction, built together and removed together, and the two
/// join at each end so a dead-end spur is drivable. Nothing overtakes anywhere in the network:
/// there is no lane to move into, so a slow rover is everyone's problem and one badly placed
/// building is a queue you can watch form. One lane shared both ways was cheaper and made traffic
/// a decoration; several each way bought overtaking and spent it softening the jams the game is
/// for; making the player draw the return leg charged the saving to the first thing they build.
pub struct RoadPlugin;

/// A road the player drew: the tiles it runs through, in the order it crossed them.
///
/// The tiles are the road. Its spline, the lanes over it and every world position a rover ever
/// stands at are derived from them, so two roads drawn through the same tiles are the same shape.
#[derive(Component)]
#[require(NeedsInitialization)]
pub struct Road {
    /// The tiles the road was drawn through, from one end to the other.
    pub path: Vec<HexCoordinates>,
}

/// One direction of travel along a road, owning the segments that make it up.
#[derive(Component)]
struct Lane;

/// A stretch of one lane: the piece of the road's spline a rover drives in one go.
#[derive(Component)]
pub struct RoadSegment {
    curve: CubicSegment<Vec3>,
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
            .add_systems(Update, draw_the_lanes);
    }
}

impl RoadSegment {
    /// Where on the ground a rover `along` of the way down this segment stands.
    pub fn world_position(&self, along: f32) -> Vec3 {
        self.curve.position(along.clamp(0., 1.))
    }
}

impl Initialize<RoadInitializeParams<'_, '_>> for Road {
    fn initialize(&mut self, entity: &Entity, params: &mut RoadInitializeParams) -> Result {
        let along = spline(self.path.iter().copied())?;
        let back = spline(self.path.iter().copied().rev())?;

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

/// The curve running through the tiles of `path`, one cubic per step between two of them.
///
/// Catmull-Rom, so the road bends through a turn rather than cornering, and the tiles it was drawn
/// through are on it. Adjacent tiles all stand the same distance apart, which is what leaves the
/// cubics of one road roughly the same length without resampling them.
fn spline(
    path: impl Iterator<Item = HexCoordinates>,
) -> std::result::Result<CubicCurve<Vec3>, InsufficientDataError> {
    let points: Vec<Vec3> = path.map(|tile| tile.world_position()).collect();
    CubicCardinalSpline::new_catmull_rom(points).to_curve()
}

/// The two segments of a lane a road's other lane joins onto.
struct LaneEnds {
    first: Entity,
    last: Entity,
}

/// Put one direction of travel on `road`: a lane, and a segment of it per cubic of `curve`.
fn spawn_lane(commands: &mut Commands, road: Entity, curve: &CubicCurve<Vec3>) -> Result<LaneEnds> {
    let lane = commands.spawn((Lane, ChildOf(road))).id();
    let mut ends: Option<LaneEnds> = None;

    for &cubic in curve.segments() {
        let segment = commands
            .spawn((RoadSegment { curve: cubic }, ChildOf(lane)))
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

    ends.ok_or_else(|| "a lane of no segments".into())
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
            (0..=SEGMENT_SUBDIVISIONS)
                .map(|step| segment.world_position(step as f32 / SEGMENT_SUBDIVISIONS as f32)),
            LANE_COLOUR,
        );

        let Some(next) = next.and_then(|next| onward.get(next.0).ok()) else {
            continue;
        };
        gizmos.arrow(
            segment.world_position(1. - HANDOVER_REACH),
            next.world_position(HANDOVER_REACH),
            HANDOVER_COLOUR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::initialize::InitializationFailed;
    use crate::diagnostics::DebugGizmosPlugin;
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

    fn road_app() -> App {
        let mut app = headless_app();
        app.add_plugins((DebugGizmosPlugin, RoadPlugin));
        app
    }

    fn tiles(offsets: &[(i32, i32)]) -> Vec<HexCoordinates> {
        offsets
            .iter()
            .map(|&(col, row)| HexCoordinates::from_offset_row(col, row))
            .collect()
    }

    fn spawn_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        app.world_mut()
            .spawn(Road {
                path: tiles(offsets),
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

    /// How far it is along `segment`, measured by walking the spline in small steps.
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
    fn a_lane_is_split_into_a_segment_for_every_step_of_the_road() {
        let (app, road) = built_road(&STRAIGHT);

        for lane in lanes(&app, road) {
            assert_eq!(children_of(&app, lane).len(), STRAIGHT.len() - 1);
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
        let (app, road) = built_road(&TURNING);

        let mut joins = 0;
        for lane in lanes(&app, road) {
            for pair in children_of(&app, lane).windows(2) {
                let ends = position(&app, pair[0], 1.);
                let starts = position(&app, pair[1], 0.);
                assert!(ends.distance(starts) < TOLERANCE, "{ends} then {starts}");
                joins += 1;
            }
        }

        assert_eq!(joins, 2 * (TURNING.len() - 2));
    }

    #[test]
    fn the_segments_of_a_lane_are_roughly_the_same_length() {
        /// How much longer than the shortest segment of a lane the longest may be.
        ///
        /// A road that runs straight and then turns twice measures 3.4% across, the shortest
        /// being a straight run between two tiles and the longest the outside of a bend.
        const SPREAD: f32 = 1.05;

        let (app, road) = built_road(&WINDING);

        let lengths: Vec<f32> = lanes(&app, road)
            .into_iter()
            .flat_map(|lane| children_of(&app, lane))
            .map(|segment| length_of(&app, segment))
            .collect();
        let longest = lengths.iter().copied().fold(f32::MIN, f32::max);
        let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);

        assert_eq!(lengths.len(), 2 * (WINDING.len() - 1));
        assert!(longest <= shortest * SPREAD, "{lengths:?}");
    }

    #[test]
    fn a_position_along_a_segment_follows_the_spline_rather_than_the_line_between_its_ends() {
        /// How far off the straight line the middle of a curved segment has to be, as a share of
        /// the distance between its ends.
        const BEND: f32 = 0.01;

        let path = tiles(&TURNING);
        let (app, road) = built_road(&TURNING);

        let lane = lane_from(&app, road, path[0]);
        let segment = *lane.first().expect("the lane has segments");

        let (start, end) = (position(&app, segment, 0.), position(&app, segment, 1.));
        let strayed = position(&app, segment, 0.5).distance(start.midpoint(end));
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
}
