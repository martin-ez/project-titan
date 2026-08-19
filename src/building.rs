use crate::common::cursor::CursorSurface;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{HexCoordinates, MapTile, MAP_TILE_SIZE};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

const BUILDING_HEIGHT: f32 = 4.;
const BUILDING_WIDTH: f32 = MAP_TILE_SIZE / 2.;

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

/// Marks a tile as carrying a building, which is what makes it refuse another.
#[derive(Component)]
struct Occupied;

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

impl Plugin for BuildingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            initialize_system::<Building, BuildingInitializeParams>,
        )
        .add_systems(Update, place_building_system);
    }
}

/// Put a building on the tile the cursor is over, when the player taps holding the building tool.
///
/// A tile takes one building and then refuses, so a tap on a tile that is already built on does
/// nothing rather than stacking a second on top of the first.
fn place_building_system(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    tiles: Query<(&MapTile, Has<Occupied>)>,
) {
    if !player_input.tap || *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(entity) = player_input.cursor_tile else {
        return;
    };
    let Ok((tile, occupied)) = tiles.get(entity) else {
        return;
    };
    if occupied {
        return;
    }

    commands.entity(entity).insert(Occupied);
    commands.spawn((
        Building {
            coordinates: tile.coordinates,
        },
        Visibility::Hidden,
    ));
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
    use crate::common::initialize::InitializationFailed;
    use crate::testing::{headless_app, tick};

    fn building_app(action: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(action)
            .insert_resource(PlayerInput::default())
            .add_plugins(BuildingPlugin);
        app
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
}
