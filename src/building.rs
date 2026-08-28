use crate::common::cleanup::Destroy;
use crate::common::cursor::CursorSurface;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{HexCoordinates, MapTile, MAP_TILE_INRADIUS, MAP_TILE_SIZE};
use crate::road::{RoadEndpoint, RoadTiles};
use crate::ui::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashMap;

const BUILDING_HEIGHT: f32 = 4.;
const BUILDING_WIDTH: f32 = MAP_TILE_SIZE / 2.;

/// How far the debug view lifts the mark on a tile, so it does not fight the tile it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a tile that will not take a building is crossed out in
const REFUSED_COLOUR: Color = Color::srgb(0.95, 0.3, 0.3);

/// How far the cross on a refused tile reaches, as a share of the tile's inradius.
const REFUSED_MARK: f32 = 0.5;

pub struct BuildingPlugin;

/// A building, standing on the tile whose coordinates it carries.
#[derive(Component)]
#[require(
    Transform,
    InheritedVisibility,
    NeedsInitialization,
    CursorSurface = building_surface()
)]
struct Building {
    coordinates: HexCoordinates,
}

/// Which building stands on each tile of the map.
///
/// Keyed by the tile, because both rules it answers are asked of one: a tile carrying a building
/// refuses another, and an arc the road tool proposes over that tile is refused too. That second
/// reader is why the record is a resource rather than a marker on the tile, and it is `RoadTiles`
/// seen from the other end.
#[derive(Resource, Default)]
pub struct BuildingTiles {
    on: HashMap<HexCoordinates, Entity>,
}

/// The roof a building offers the cursor, claiming the whole of the tile it stands on.
fn building_surface() -> CursorSurface {
    CursorSurface {
        radius: MAP_TILE_SIZE / 2.,
        height: BUILDING_HEIGHT,
    }
}

#[derive(SystemParam)]
struct BuildingInitializeParams<'w, 's> {
    query: Query<'w, 's, (&'static mut Transform, &'static mut Visibility), With<Building>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

impl BuildingTiles {
    /// The building standing on `tile`, of which there is at most one.
    pub fn building_on(&self, tile: HexCoordinates) -> Option<Entity> {
        self.on.get(&tile).copied()
    }

    fn claim(&mut self, tile: HexCoordinates, building: Entity) {
        self.on.insert(tile, building);
    }

    fn release(&mut self, tile: HexCoordinates) {
        self.on.remove(&tile);
    }
}

impl Plugin for BuildingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingTiles>()
            .declare_bindings([
                Binding {
                    input: BindingInput::Mouse(MouseButton::Left),
                    action: "Put a building on the tile",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
                Binding {
                    input: BindingInput::Mouse(MouseButton::Right),
                    action: "Take the building off the tile",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
            ])
            .add_observer(release_the_tile_of_a_removed_building)
            .add_systems(
                PreUpdate,
                initialize_system::<Building, BuildingInitializeParams>,
            )
            .add_systems(
                Update,
                (
                    (place_building_system, remove_building_system).chain(),
                    draw_the_refused_tile,
                ),
            );
    }
}

/// Whether `tile` will take a building, which is the whole of the rule and is asked in one place.
///
/// A road is read off the tile it runs over rather than measured out of its arcs, so the answer
/// costs a lookup however many roads are on the map.
fn takes_a_building(tile: HexCoordinates, buildings: &BuildingTiles, roads: &RoadTiles) -> bool {
    buildings.building_on(tile).is_none() && roads.roads_over(tile).is_empty()
}

/// Give up the tile a building held, whichever way it left the world.
fn release_the_tile_of_a_removed_building(
    removed: On<Remove, Building>,
    buildings: Query<&Building>,
    mut tiles: ResMut<BuildingTiles>,
) {
    if let Ok(building) = buildings.get(removed.entity) {
        tiles.release(building.coordinates);
    }
}

/// Put a building on the tile the cursor is over, when the player taps holding the building tool.
///
/// A tile takes one building and then refuses, so a tap on a tile that is already built on does
/// nothing rather than stacking a second on top of the first. A road running over the tile refuses
/// it the same way, whether the road stops there or only crosses it on the way somewhere else:
/// there is no room for a building on ground a rover drives over (invariant 1).
fn place_building_system(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Res<RoadTiles>,
    mut buildings: ResMut<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if !player_input.tap || *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(entity) = player_input.cursor_tile else {
        return;
    };
    let Ok(tile) = tiles.get(entity) else {
        return;
    };
    if !takes_a_building(tile.coordinates, &buildings, &roads) {
        return;
    }

    let building = commands
        .spawn((
            Building {
                coordinates: tile.coordinates,
            },
            RoadEndpoint::on(tile.coordinates),
            Visibility::Hidden,
        ))
        .id();
    buildings.claim(tile.coordinates, building);
}

/// Take the building off the tile the cursor is over, when the player clicks the secondary button
/// holding the building tool.
///
/// The tile is left as placeable as it was before anything stood on it: the record of what stands
/// there goes with the building, so nothing is left behind to refuse the next one.
fn remove_building_system(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    buildings: Res<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if !player_input.secondary_tap || *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(entity) = player_input.cursor_tile else {
        return;
    };
    let Ok(tile) = tiles.get(entity) else {
        return;
    };
    let Some(building) = buildings.building_on(tile.coordinates) else {
        return;
    };

    commands.entity(building).insert(Destroy);
}

/// Cross out the tile under the cursor when it will not take a building.
///
/// A refused tap is otherwise a click that does nothing, which reads as a game that missed it
/// rather than a tile that is taken. Marking it while the tool is held says so before the player
/// clicks, and says it the same way whether a building or a road is what is in the way.
fn draw_the_refused_tile(
    mut gizmos: Gizmos<DebugGizmos>,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Res<RoadTiles>,
    buildings: Res<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(tile) = player_input
        .cursor_tile
        .and_then(|entity| tiles.get(entity).ok())
    else {
        return;
    };
    if takes_a_building(tile.coordinates, &buildings, &roads) {
        return;
    }

    let centre = tile.coordinates.world_position() + GIZMO_LIFT;
    let reach = MAP_TILE_INRADIUS * REFUSED_MARK;
    for across in [Vec3::new(reach, 0., reach), Vec3::new(reach, 0., -reach)] {
        gizmos.line(centre - across, centre + across, REFUSED_COLOUR);
    }
}

impl Initialize<BuildingInitializeParams<'_, '_>> for Building {
    fn initialize(&mut self, entity: &Entity, params: &mut BuildingInitializeParams) -> Result {
        let (mut transform, mut visibility) = params.query.get_mut(*entity)?;
        transform.translation = self.coordinates.world_position();
        *visibility = Visibility::Visible;

        params.commands.entity(*entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(params.meshes.add(Cuboid::new(
                    BUILDING_WIDTH,
                    BUILDING_HEIGHT,
                    BUILDING_WIDTH,
                ))),
                MeshMaterial3d(params.materials.add(Color::srgb(0.55, 0.6, 0.72))),
                Transform::from_translation(Vec3::new(0., BUILDING_HEIGHT / 2., 0.)),
            ));
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::cleanup::CleanupPlugin;
    use crate::common::initialize::InitializationFailed;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::map::LatticeNode;
    use crate::road::{Road, RoadPlugin};
    use crate::testing::{headless_app, tick};

    /// A run of tiles whose nodes are neighbours, so the road takes those tiles and no others.
    const NEIGHBOURING: [(i32, i32); 3] = [(0, 0), (1, 0), (2, 0)];

    /// Two nodes far enough apart that the road runs over the tiles between them.
    ///
    /// A road drawn between neighbouring tile centres never leaves the tiles its nodes stand on,
    /// so a rule tested only against one of those is a rule tested only at the nodes.
    const SPANNING: [(i32, i32); 2] = [(0, 0), (5, 0)];

    fn building_app(action: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(action)
            .insert_resource(PlayerInput::default())
            .add_plugins((BuildingPlugin, CleanupPlugin, DebugGizmosPlugin, RoadPlugin));
        app
    }

    /// Lay a road through `offsets` and let it take its tiles.
    fn spawn_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        let nodes = offsets
            .iter()
            .map(|&(col, row)| LatticeNode::from_tile(HexCoordinates::from_offset_row(col, row)))
            .collect();
        let road = app
            .world_mut()
            .spawn(Road {
                nodes,
                leaving: None,
                one_way: false,
            })
            .id();
        tick(app);
        road
    }

    fn spawn_tile(app: &mut App, col: i32, row: i32) -> Entity {
        app.world_mut()
            .spawn(MapTile {
                coordinates: HexCoordinates::from_offset_row(col, row),
            })
            .id()
    }

    /// Click on `tile`, then let the tap go, so a second frame is not a second click.
    fn tap_on(app: &mut App, tile: Option<Entity>) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = tile;
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    /// Right-click on `tile`, then let the button go, so a second frame is not a second click.
    fn secondary_tap_on(app: &mut App, tile: Option<Entity>) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = true;
            input.cursor_tile = tile;
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().secondary_tap = false;
    }

    fn still_there(app: &App, entity: Entity) -> bool {
        app.world().entities().contains(entity)
    }

    fn buildings(app: &mut App) -> Vec<HexCoordinates> {
        let mut query = app.world_mut().query::<&Building>();
        query
            .iter(app.world())
            .map(|building| building.coordinates)
            .collect()
    }

    fn building_entity(app: &mut App) -> Option<Entity> {
        let mut query = app.world_mut().query_filtered::<Entity, With<Building>>();
        query.iter(app.world()).next()
    }

    #[test]
    fn a_tap_with_the_building_tool_puts_a_building_on_the_tile_under_the_cursor() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 2, 3);

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(2, 3)]);
    }

    #[test]
    fn a_placed_building_gets_an_endpoint_to_meet_the_road_network_on() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 2, 3);

        tap_on(&mut app, Some(tile));

        let building = building_entity(&mut app).expect("the tap placed a building");
        assert!(app.world().entity(building).contains::<RoadEndpoint>());
    }

    #[test]
    fn a_placed_building_stands_at_the_world_position_of_its_tile() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 2, 3);

        tap_on(&mut app, Some(tile));
        tick(&mut app);

        let building = building_entity(&mut app).expect("the tap placed a building");
        assert_eq!(
            app.world()
                .entity(building)
                .get::<Transform>()
                .map(|t| t.translation),
            Some(HexCoordinates::from_offset_row(2, 3).world_position())
        );
    }

    #[test]
    fn a_placed_building_is_there_to_see_once_it_is_initialized() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));
        tick(&mut app);

        let building = building_entity(&mut app).expect("the tap placed a building");
        let world = app.world();
        assert_eq!(
            world.entity(building).get::<Visibility>(),
            Some(&Visibility::Visible)
        );
        assert!(!world.entity(building).contains::<InitializationFailed>());
        assert!(world
            .entity(building)
            .get::<Children>()
            .is_some_and(|children| !children.is_empty()));
    }

    #[test]
    fn a_secondary_tap_takes_the_building_off_the_tile() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));

        secondary_tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tile_whose_building_was_removed_takes_another_one() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        let first = building_entity(&mut app).expect("the tap placed a building");
        secondary_tap_on(&mut app, Some(tile));

        tap_on(&mut app, Some(tile));

        let second = building_entity(&mut app).expect("the tile took another building");
        assert!(!still_there(&app, first));
        assert_ne!(second, first);
    }

    #[test]
    fn the_mesh_of_a_removed_building_goes_with_it() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        tick(&mut app);
        let building = building_entity(&mut app).expect("the tap placed a building");
        let mesh = app
            .world()
            .entity(building)
            .get::<Children>()
            .and_then(|children| children.iter().next())
            .expect("the building was given a mesh");

        secondary_tap_on(&mut app, Some(tile));

        assert!(!still_there(&app, building));
        assert!(!still_there(&app, mesh));
    }

    #[test]
    fn a_secondary_tap_while_selecting_leaves_the_building_alone() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::Select);
        tick(&mut app);

        secondary_tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_while_editing_roads_leaves_the_building_alone() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::EditRoads);
        tick(&mut app);

        secondary_tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_on_an_empty_tile_does_nothing() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let built = spawn_tile(&mut app, 0, 0);
        let empty = spawn_tile(&mut app, 1, 0);
        tap_on(&mut app, Some(built));

        secondary_tap_on(&mut app, Some(empty));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_over_no_tile_does_nothing() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));

        secondary_tap_on(&mut app, None);

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_tap_while_selecting_puts_nothing_down() {
        let mut app = building_app(PlayerAction::Select);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_while_editing_roads_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditRoads);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_cursor_over_no_tile_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, None);

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn moving_over_a_tile_without_tapping_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        app.world_mut().resource_mut::<PlayerInput>().cursor_tile = Some(tile);

        tick(&mut app);

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_second_tap_on_an_occupied_tile_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));
        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_tap_on_a_free_tile_beside_an_occupied_one_still_places() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let occupied = spawn_tile(&mut app, 0, 0);
        let free = spawn_tile(&mut app, 1, 0);

        tap_on(&mut app, Some(occupied));
        tap_on(&mut app, Some(free));

        assert_eq!(buildings(&mut app).len(), 2);
    }

    #[test]
    fn a_building_offers_the_cursor_a_roof_to_climb_onto() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        let building = building_entity(&mut app).expect("the tap placed a building");
        assert_eq!(
            app.world()
                .entity(building)
                .get::<CursorSurface>()
                .map(|s| s.height),
            Some(BUILDING_HEIGHT)
        );
    }

    #[test]
    fn a_tap_on_a_tile_a_road_stands_on_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_on_a_tile_a_road_only_crosses_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &SPANNING);
        let tile = spawn_tile(&mut app, 3, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_on_a_tile_beside_a_road_still_places_a_building() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 1);

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(1, 1)]);
    }

    #[test]
    fn a_tile_whose_road_was_taken_away_takes_a_building() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let road = spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 0);
        app.world_mut().entity_mut(road).despawn();

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(1, 0)]);
    }
}
