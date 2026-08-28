use crate::common::cursor::{CursorHit, CursorRayCast};
use crate::map::{LatticeNode, MapTile};
use crate::ui::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use bevy::ecs::system::SystemParam;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

/// The key that picks up each tool, and the tool it picks up
const TOOL_KEYS: [(KeyCode, PlayerAction); 3] = [
    (KeyCode::Digit1, PlayerAction::Select),
    (KeyCode::Digit2, PlayerAction::EditRoads),
    (KeyCode::Digit3, PlayerAction::EditBuildings),
];

/// What the player holds to move the camera, most specific first, and the movement it asks for
const CAMERA_MODIFIERS: [(BindingInput, CameraMovement); 4] = [
    (BindingInput::Key(KeyCode::ShiftLeft), CameraMovement::Orbit),
    (
        BindingInput::Key(KeyCode::ControlLeft),
        CameraMovement::Zoom,
    ),
    (BindingInput::Key(KeyCode::Space), CameraMovement::Pan),
    (
        BindingInput::Mouse(MouseButton::Middle),
        CameraMovement::Pan,
    ),
];

/// The keys that drive the camera over the surface, the way each drives it, and what to call that
const MOVEMENT_KEYS: [(KeyCode, Vec3, &str); 4] = [
    (KeyCode::KeyW, Vec3::NEG_Z, "Move the camera forward"),
    (KeyCode::KeyS, Vec3::Z, "Move the camera back"),
    (KeyCode::KeyA, Vec3::NEG_X, "Move the camera left"),
    (KeyCode::KeyD, Vec3::X, "Move the camera right"),
];

/// Key binding for finishing what the player is placing
const FINISH_KEY: KeyCode = KeyCode::Escape;

pub struct PlayerInputPlugin;

/// The current desired action of the player, controlled by the UI or keyboard shortcuts
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerAction {
    Select,
    EditRoads,
    EditBuildings,
}

impl PlayerAction {
    /// What the legend calls this tool.
    pub fn label(&self) -> &'static str {
        match self {
            PlayerAction::Select => "Select tool",
            PlayerAction::EditRoads => "Road tool",
            PlayerAction::EditBuildings => "Building tool",
        }
    }
}

/// The current movement type of the camera
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraMovement {
    Translate,
    Pan,
    Orbit,
    Zoom,
}

impl CameraMovement {
    /// What the legend calls asking the camera for this movement.
    pub fn label(&self) -> &'static str {
        match self {
            CameraMovement::Translate => "Move the camera",
            CameraMovement::Pan => "Hold and drag to pan the camera",
            CameraMovement::Orbit => "Hold and drag to orbit the camera",
            CameraMovement::Zoom => "Hold and drag to zoom the camera",
        }
    }
}

#[derive(Resource, Default)]
pub struct PlayerInput {
    /// The point on the surface the cursor is over, settled onto a tile while a build tool is held
    pub world_cursor_position: Option<Vec3>,
    /// Where the cursor meets the ground plane, whatever stands between the two
    pub ground_cursor_position: Option<Vec3>,
    /// The tile the cursor is over, which is the one under it when the cursor is over a building
    pub cursor_tile: Option<Entity>,
    /// The node of the road lattice the cursor is over, while the road tool is the one held
    pub cursor_node: Option<LatticeNode>,
    /// The normalized vector representing the player's movement (WASD)
    pub movement_vector: Vec3,
    /// Whether the player just tap or clicked
    pub tap: bool,
    /// Whether the player just clicked with the secondary mouse button
    pub secondary_tap: bool,
    /// Whether the player just asked to finish placing what they are part way through
    pub finish: bool,
}

#[derive(Component)]
#[require(Transform, Visibility)]
struct EditingTargetIndicator;

#[derive(SystemParam)]
struct CursorReading<'w, 's> {
    window: Option<Single<'w, 's, &'static Window>>,
    camera: Option<Single<'w, 's, (&'static Camera, &'static GlobalTransform)>>,
    surfaces: CursorRayCast<'w, 's>,
    tiles: Query<'w, 's, &'static MapTile>,
}

impl Plugin for PlayerInputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(PlayerAction::Select)
            .insert_state(CameraMovement::Translate)
            .insert_resource(PlayerInput::default())
            .declare_bindings(TOOL_KEYS.map(|(key, tool)| Binding {
                input: BindingInput::Key(key),
                action: tool.label(),
                context: BindingContext::Always,
            }))
            .declare_bindings(CAMERA_MODIFIERS.map(|(input, movement)| Binding {
                input,
                action: movement.label(),
                context: BindingContext::Always,
            }))
            .declare_bindings(MOVEMENT_KEYS.map(|(key, _, action)| Binding {
                input: BindingInput::Key(key),
                action,
                context: BindingContext::Always,
            }))
            .declare_bindings([Binding {
                input: BindingInput::Key(FINISH_KEY),
                action: "Finish what you are placing",
                context: BindingContext::Always,
            }])
            .add_systems(Startup, (spawn_indicator, hide_the_cursor))
            .add_systems(
                PreUpdate,
                (
                    update_camera_movement_type,
                    update_player_action,
                    update_player_input,
                )
                    .after(InputSystems),
            )
            .add_systems(Update, update_indicator);
    }
}

fn spawn_indicator(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        EditingTargetIndicator {},
        Visibility::Hidden,
        Mesh3d(meshes.add(Sphere::new(0.1))),
        MeshMaterial3d(materials.add(Color::srgb(0.1, 0.2, 0.9))),
    ));
}

fn hide_the_cursor(mut cursor_q: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    for mut cursor_options in &mut cursor_q {
        cursor_options.visible = false;
    }
}

/// Update the camera movement type based on the player's input
fn update_camera_movement_type(
    input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    current_state: Res<State<CameraMovement>>,
    mut next_state: ResMut<NextState<CameraMovement>>,
) {
    let wanted = CAMERA_MODIFIERS
        .iter()
        .find(|(binding, _)| is_held(*binding, &input, &mouse_input))
        .map_or(CameraMovement::Translate, |(_, movement)| *movement);

    if *current_state.get() != wanted {
        next_state.set(wanted);
    }
}

fn is_held(
    binding: BindingInput,
    input: &ButtonInput<KeyCode>,
    mouse_input: &ButtonInput<MouseButton>,
) -> bool {
    match binding {
        BindingInput::Key(key) => input.pressed(key),
        BindingInput::Mouse(button) => mouse_input.pressed(button),
        BindingInput::Scroll => false,
    }
}

/// Update the tool the player is holding based on the player's input.
///
/// A tool is picked up rather than held down: it stays until another key puts it down, which is
/// what a camera movement is not.
fn update_player_action(
    input: Res<ButtonInput<KeyCode>>,
    current_state: Res<State<PlayerAction>>,
    mut next_state: ResMut<NextState<PlayerAction>>,
) {
    let Some((_, wanted)) = TOOL_KEYS.iter().find(|(key, _)| input.just_pressed(*key)) else {
        return;
    };
    let wanted = *wanted;

    if *current_state.get() != wanted {
        next_state.set(wanted);
    }
}

/// Read the player's input into `PlayerInput`.
///
/// The cursor points nowhere without a window to point in, and nowhere without a camera to point
/// from, but a key and a click are still a key and a click: neither half stops the other.
fn update_player_input(
    mut player_input: ResMut<PlayerInput>,
    input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    cursor: CursorReading,
    action: Res<State<PlayerAction>>,
) {
    player_input.tap = mouse_input.just_pressed(MouseButton::Left);
    player_input.secondary_tap = mouse_input.just_pressed(MouseButton::Right);
    player_input.finish = player_input.secondary_tap || input.just_pressed(FINISH_KEY);

    let Some(camera) = &cursor.camera else {
        player_input.movement_vector = Vec3::ZERO;
        player_input.world_cursor_position = None;
        player_input.ground_cursor_position = None;
        player_input.cursor_tile = None;
        player_input.cursor_node = None;
        return;
    };
    let (camera, camera_transform) = **camera;

    player_input.movement_vector = get_movement_vector(&input, camera_transform);

    let ray = cursor
        .window
        .as_ref()
        .and_then(|window| get_cursor_ray(window, camera, camera_transform));
    player_input.ground_cursor_position = ray.and_then(ground_plane_position);

    let hit = ray.and_then(|ray| cursor.surfaces.cast(ray));
    let node = hit
        .as_ref()
        .filter(|_| *action.get() == PlayerAction::EditRoads)
        .and_then(|hit| {
            let tile = cursor.tiles.get(hit.tile?.entity).ok()?;
            Some(LatticeNode::nearest_on(tile.coordinates, hit.point))
        });

    player_input.cursor_node = node;
    player_input.world_cursor_position =
        hit.as_ref().map(|hit| settled_position(hit, &action, node));
    player_input.cursor_tile = hit.and_then(|hit| hit.tile).map(|tile| tile.entity);
}

/// Where the cursor reports itself to be, given the tool the player is holding.
///
/// The road tool settles it on `node`, which a road may be built through, and the building tool
/// over the middle of the tile, which is where a building stands. The height it landed at is its
/// own either way, so a cursor over a building stays on top of the building rather than dropping
/// through it.
fn settled_position(hit: &CursorHit, action: &PlayerAction, node: Option<LatticeNode>) -> Vec3 {
    if let Some(node) = node {
        let settled = node.world_position();
        return Vec3::new(settled.x, hit.point.y, settled.z);
    }
    match hit.tile {
        Some(tile) if *action == PlayerAction::EditBuildings => {
            Vec3::new(tile.centre.x, hit.point.y, tile.centre.z)
        }
        _ => hit.point,
    }
}

/// Calculate the movement vector based on the player's input (WASD) and the camera's orientation
fn get_movement_vector(input: &ButtonInput<KeyCode>, camera_transform: &GlobalTransform) -> Vec3 {
    let mut direction = Vec3::ZERO;
    for (key, towards, _) in MOVEMENT_KEYS {
        if input.pressed(key) {
            direction += towards;
        }
    }

    ground_plane_direction(camera_transform.rotation() * direction)
}

/// Flatten a direction onto the ground plane, as a unit vector or nothing.
fn ground_plane_direction(direction: Vec3) -> Vec3 {
    Vec3::new(direction.x, 0.0, direction.z).normalize_or_zero()
}

/// The ray originating from the camera and passing through the cursor
fn get_cursor_ray(
    window: &Window,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<Ray3d> {
    window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world(camera_transform, cursor).ok())
}

/// Where a ray meets the ground plane
fn ground_plane_position(ray: Ray3d) -> Option<Vec3> {
    ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::default())
        .map(|distance| ray.get_point(distance))
}

fn update_indicator(
    mut indicator_q: Query<(&mut Transform, &mut Visibility), With<EditingTargetIndicator>>,
    player_input: Res<PlayerInput>,
) {
    for (mut indicator, mut visibility) in &mut indicator_q {
        if let Some(point) = player_input.world_cursor_position {
            indicator.translation = point;
            *visibility = Visibility::Visible;
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::cursor::{CursorSurface, TileSurface};
    use crate::map::HexCoordinates;
    use crate::testing::{headless_app, press_key, press_mouse, release_key, release_mouse, tick};
    use bevy::camera::RenderTargetInfo;
    use bevy::math::DVec2;
    use std::f32::consts::FRAC_PI_2;

    const SURFACE_RADIUS: f32 = 10.;
    const WINDOW_SIZE: UVec2 = UVec2::new(1280, 720);
    /// A tile centre the ray down the world's Y axis lands on, but not at the middle of.
    const OFF_CENTRE_TILE: Vec3 = Vec3::new(3., 0., 1.);

    fn input_app() -> App {
        let mut app = headless_app();
        app.add_plugins(PlayerInputPlugin);
        app
    }

    fn spawn_a_camera(app: &mut App) {
        app.world_mut().spawn(Camera3d::default());
    }

    /// An app whose cursor sits at the middle of the window, under a camera looking straight down.
    ///
    /// A headless camera has no render target to size a viewport from, so the test gives it one.
    /// The ray it casts through the middle of that viewport falls down the world's Y axis.
    fn app_looking_down_at_the_origin() -> App {
        let mut app = input_app();
        let mut camera = Camera::default();
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: WINDOW_SIZE,
            scale_factor: 1.,
        });
        app.world_mut().spawn((
            camera,
            Camera3d::default(),
            Transform::from_xyz(0., 50., 0.).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
        ));

        let mut window = Window::default();
        window.set_physical_cursor_position(Some(DVec2::new(
            WINDOW_SIZE.x as f64 / 2.,
            WINDOW_SIZE.y as f64 / 2.,
        )));
        app.world_mut().spawn(window);

        tick(&mut app);
        app
    }

    fn spawn_surface_at(app: &mut App, centre: Vec3, height: f32) -> Entity {
        app.world_mut()
            .spawn((
                CursorSurface {
                    radius: SURFACE_RADIUS,
                    height,
                },
                Transform::from_translation(centre),
            ))
            .id()
    }

    fn spawn_surface(app: &mut App, height: f32) -> Entity {
        spawn_surface_at(app, Vec3::ZERO, height)
    }

    fn spawn_tile_at(app: &mut App, centre: Vec3) -> Entity {
        let tile = spawn_surface_at(app, centre, 0.);
        app.world_mut().entity_mut(tile).insert(TileSurface);
        tile
    }

    /// A tile of the grid standing at `centre`, which the cursor can name a node of.
    fn spawn_grid_tile_at(app: &mut App, centre: Vec3, coordinates: HexCoordinates) -> Entity {
        let tile = spawn_tile_at(app, centre);
        app.world_mut()
            .entity_mut(tile)
            .insert(MapTile { coordinates });
        tile
    }

    fn cursor_node(app: &App) -> Option<LatticeNode> {
        player_input(app).cursor_node
    }

    /// The tile the fixtures put under the cursor, whose middle node stands at the world's origin.
    fn origin_tile() -> HexCoordinates {
        HexCoordinates::from_offset_row(0, 0)
    }

    fn spawn_tile(app: &mut App) -> Entity {
        spawn_tile_at(app, Vec3::ZERO)
    }

    /// Pick up `tool` and let the state it asks for reach the next read of the cursor.
    fn hold_tool(app: &mut App, tool: KeyCode) {
        press_key(app, tool);
        tick(app);
        tick(app);
    }

    /// A ray through a viewport carries the rounding of that viewport, so compare places loosely.
    fn assert_lands_on(landed: Option<Vec3>, expected: Vec3) {
        let landed = landed.expect("the cursor landed somewhere");
        assert!(
            landed.distance(expected) < 1e-4,
            "{landed:?} is not where {expected:?} is"
        );
    }

    fn spawn_a_primary_window(app: &mut App) {
        app.world_mut()
            .spawn((Window::default(), PrimaryWindow, CursorOptions::default()));
    }

    #[test]
    fn every_tool_on_the_table_is_picked_up_by_its_key() {
        for (key, tool) in TOOL_KEYS {
            let mut app = input_app();
            tick(&mut app);

            press_key(&mut app, key);
            tick(&mut app);

            assert_eq!(
                *app.world().resource::<State<PlayerAction>>().get(),
                tool,
                "{key:?} did not pick up {tool:?}"
            );
        }
    }

    fn tool_key(tool: PlayerAction) -> KeyCode {
        TOOL_KEYS
            .iter()
            .find(|(_, bound)| *bound == tool)
            .map(|(key, _)| *key)
            .expect("the tool is on a key")
    }

    fn camera_key(movement: CameraMovement) -> KeyCode {
        CAMERA_MODIFIERS
            .iter()
            .find_map(|(input, bound)| match input {
                BindingInput::Key(key) if *bound == movement => Some(*key),
                _ => None,
            })
            .expect("the movement is on a key")
    }

    fn camera_movement(app: &App) -> CameraMovement {
        *app.world().resource::<State<CameraMovement>>().get()
    }

    fn player_input(app: &App) -> &PlayerInput {
        app.world().resource::<PlayerInput>()
    }

    #[test]
    fn holding_the_orbit_key_switches_the_camera_to_orbiting() {
        let mut app = input_app();
        tick(&mut app);

        press_key(&mut app, camera_key(CameraMovement::Orbit));
        tick(&mut app);

        assert_eq!(camera_movement(&app), CameraMovement::Orbit);
    }

    #[test]
    fn letting_the_orbit_key_go_returns_the_camera_to_translating() {
        let mut app = input_app();
        press_key(&mut app, camera_key(CameraMovement::Orbit));
        tick(&mut app);

        release_key(&mut app, camera_key(CameraMovement::Orbit));
        tick(&mut app);

        assert_eq!(camera_movement(&app), CameraMovement::Translate);
    }

    #[test]
    fn holding_the_middle_mouse_button_pans_the_camera() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Middle);
        tick(&mut app);

        assert_eq!(camera_movement(&app), CameraMovement::Pan);
    }

    #[test]
    fn letting_the_middle_mouse_button_go_returns_the_camera_to_translating() {
        let mut app = input_app();
        press_mouse(&mut app, MouseButton::Middle);
        tick(&mut app);

        release_mouse(&mut app, MouseButton::Middle);
        tick(&mut app);

        assert_eq!(camera_movement(&app), CameraMovement::Translate);
    }

    #[test]
    fn a_click_is_reported_as_a_tap() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Left);
        tick(&mut app);

        assert!(player_input(&app).tap);
    }

    #[test]
    fn a_right_click_is_reported_as_a_secondary_tap() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Right);
        tick(&mut app);

        assert!(player_input(&app).secondary_tap);
    }

    #[test]
    fn a_right_click_is_reported_as_finishing() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Right);
        tick(&mut app);

        assert!(player_input(&app).finish);
    }

    #[test]
    fn pressing_escape_is_reported_as_finishing() {
        let mut app = input_app();
        tick(&mut app);

        press_key(&mut app, FINISH_KEY);
        tick(&mut app);

        assert!(player_input(&app).finish);
    }

    #[test]
    fn holding_escape_down_finishes_only_once() {
        let mut app = input_app();
        press_key(&mut app, FINISH_KEY);
        tick(&mut app);

        tick(&mut app);

        assert!(!player_input(&app).finish);
    }

    #[test]
    fn a_left_click_is_not_reported_as_finishing() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Left);
        tick(&mut app);

        assert!(!player_input(&app).finish);
    }

    #[test]
    fn a_right_click_is_not_reported_as_a_tap() {
        let mut app = input_app();
        tick(&mut app);

        press_mouse(&mut app, MouseButton::Right);
        tick(&mut app);

        assert!(!player_input(&app).tap);
    }

    #[test]
    fn pressing_forward_moves_away_from_the_camera() {
        let mut app = input_app();
        spawn_a_camera(&mut app);
        tick(&mut app);

        press_key(&mut app, KeyCode::KeyW);
        tick(&mut app);

        assert_eq!(player_input(&app).movement_vector, -Vec3::Z);
    }

    #[test]
    fn without_a_window_the_cursor_points_nowhere() {
        let mut app = input_app();
        spawn_a_camera(&mut app);

        tick(&mut app);

        assert_eq!(player_input(&app).world_cursor_position, None);
        assert_eq!(player_input(&app).ground_cursor_position, None);
        assert_eq!(player_input(&app).cursor_tile, None);
    }

    #[test]
    fn the_cursor_lands_on_a_raised_tile_rather_than_the_ground_under_it() {
        let mut app = app_looking_down_at_the_origin();
        let tile = spawn_surface(&mut app, 2.);
        app.world_mut().entity_mut(tile).insert(TileSurface);

        tick(&mut app);
        tick(&mut app);

        assert_lands_on(
            player_input(&app).world_cursor_position,
            Vec3::new(0., 2., 0.),
        );
        assert_eq!(player_input(&app).cursor_tile, Some(tile));
    }

    #[test]
    fn the_cursor_lands_on_an_object_while_still_naming_the_tile_it_stands_on() {
        let mut app = app_looking_down_at_the_origin();
        let tile = spawn_tile(&mut app);
        spawn_surface(&mut app, 3.);

        tick(&mut app);
        tick(&mut app);

        assert_lands_on(
            player_input(&app).world_cursor_position,
            Vec3::new(0., 3., 0.),
        );
        assert_eq!(player_input(&app).cursor_tile, Some(tile));
    }

    #[test]
    fn off_the_map_the_cursor_lands_nowhere_but_the_ground_still_answers() {
        let mut app = app_looking_down_at_the_origin();

        tick(&mut app);

        assert_eq!(player_input(&app).world_cursor_position, None);
        assert_eq!(player_input(&app).cursor_tile, None);
        assert_lands_on(player_input(&app).ground_cursor_position, Vec3::ZERO);
    }

    #[test]
    fn the_indicator_stands_on_the_surface_the_cursor_landed_on() {
        let mut app = app_looking_down_at_the_origin();
        spawn_surface(&mut app, 2.);

        tick(&mut app);
        tick(&mut app);

        let mut query = app
            .world_mut()
            .query_filtered::<(&Transform, &Visibility), With<EditingTargetIndicator>>();
        let (transform, visibility) = query
            .iter(app.world())
            .next()
            .expect("the plugin spawns an indicator on startup");
        assert_lands_on(Some(transform.translation), Vec3::new(0., 2., 0.));
        assert_eq!(visibility, &Visibility::Visible);
    }

    #[test]
    fn the_indicator_hides_where_the_cursor_lands_on_nothing() {
        let mut app = app_looking_down_at_the_origin();

        tick(&mut app);

        let mut query = app
            .world_mut()
            .query_filtered::<&Visibility, With<EditingTargetIndicator>>();
        let visibility = query
            .iter(app.world())
            .next()
            .expect("the plugin spawns an indicator on startup");
        assert_eq!(visibility, &Visibility::Hidden);
    }

    #[test]
    fn the_cursor_settles_on_the_tile_centre_while_the_building_tool_is_held() {
        let mut app = app_looking_down_at_the_origin();
        spawn_tile_at(&mut app, OFF_CENTRE_TILE);

        hold_tool(&mut app, tool_key(PlayerAction::EditBuildings));

        assert_lands_on(player_input(&app).world_cursor_position, OFF_CENTRE_TILE);
    }

    #[test]
    fn the_cursor_reports_a_lattice_node_while_the_road_tool_is_held() {
        let mut app = app_looking_down_at_the_origin();
        spawn_grid_tile_at(&mut app, OFF_CENTRE_TILE, origin_tile());

        hold_tool(&mut app, tool_key(PlayerAction::EditRoads));

        assert_eq!(
            cursor_node(&app),
            Some(LatticeNode::from_tile(origin_tile()))
        );
    }

    #[test]
    fn the_cursor_settles_on_the_node_it_reports_while_the_road_tool_is_held() {
        let mut app = app_looking_down_at_the_origin();
        spawn_grid_tile_at(&mut app, OFF_CENTRE_TILE, origin_tile());

        hold_tool(&mut app, tool_key(PlayerAction::EditRoads));

        assert_lands_on(player_input(&app).world_cursor_position, Vec3::ZERO);
    }

    #[test]
    fn the_cursor_reports_no_node_while_the_building_tool_is_held() {
        let mut app = app_looking_down_at_the_origin();
        spawn_grid_tile_at(&mut app, OFF_CENTRE_TILE, origin_tile());

        hold_tool(&mut app, tool_key(PlayerAction::EditBuildings));

        assert_eq!(cursor_node(&app), None);
        assert_lands_on(player_input(&app).world_cursor_position, OFF_CENTRE_TILE);
    }

    #[test]
    fn the_cursor_reports_no_node_while_selecting() {
        let mut app = app_looking_down_at_the_origin();
        spawn_grid_tile_at(&mut app, OFF_CENTRE_TILE, origin_tile());

        tick(&mut app);
        tick(&mut app);

        assert_eq!(cursor_node(&app), None);
    }

    #[test]
    fn the_cursor_reports_no_node_off_the_grid() {
        let mut app = app_looking_down_at_the_origin();
        spawn_surface_at(&mut app, OFF_CENTRE_TILE, 2.);

        hold_tool(&mut app, tool_key(PlayerAction::EditRoads));

        assert_eq!(cursor_node(&app), None);
    }

    #[test]
    fn the_cursor_stays_where_it_landed_while_selecting() {
        let mut app = app_looking_down_at_the_origin();
        spawn_tile_at(&mut app, OFF_CENTRE_TILE);

        tick(&mut app);
        tick(&mut app);

        assert_lands_on(player_input(&app).world_cursor_position, Vec3::ZERO);
    }

    #[test]
    fn the_tile_the_cursor_settles_on_is_the_one_the_ray_hit() {
        let mut app = app_looking_down_at_the_origin();
        let tile = spawn_tile_at(&mut app, OFF_CENTRE_TILE);

        hold_tool(&mut app, tool_key(PlayerAction::EditRoads));

        assert_eq!(player_input(&app).cursor_tile, Some(tile));
    }

    #[test]
    fn a_cursor_on_an_object_settles_over_the_centre_of_the_tile_it_stands_on() {
        let mut app = app_looking_down_at_the_origin();
        spawn_tile_at(&mut app, OFF_CENTRE_TILE);
        spawn_surface_at(&mut app, OFF_CENTRE_TILE, 3.);

        hold_tool(&mut app, tool_key(PlayerAction::EditBuildings));

        assert_lands_on(
            player_input(&app).world_cursor_position,
            OFF_CENTRE_TILE + Vec3::new(0., 3., 0.),
        );
    }

    #[test]
    fn a_held_tool_leaves_a_cursor_off_the_grid_where_it_landed() {
        let mut app = app_looking_down_at_the_origin();
        spawn_surface_at(&mut app, OFF_CENTRE_TILE, 2.);

        hold_tool(&mut app, tool_key(PlayerAction::EditRoads));

        assert_lands_on(
            player_input(&app).world_cursor_position,
            Vec3::new(0., 2., 0.),
        );
        assert_eq!(player_input(&app).cursor_tile, None);
    }

    #[test]
    fn a_primary_window_has_its_cursor_hidden() {
        let mut app = headless_app();
        spawn_a_primary_window(&mut app);
        app.add_plugins(PlayerInputPlugin);

        tick(&mut app);

        let mut query = app.world_mut().query::<&CursorOptions>();
        let cursor = query
            .iter(app.world())
            .next()
            .expect("the window the test spawned is still there");
        assert!(!cursor.visible);
    }

    #[test]
    fn a_tilted_direction_flattens_onto_the_ground_plane() {
        let flattened = ground_plane_direction(Vec3::new(1.0, 4.0, 0.0));
        assert_eq!(flattened.y, 0.0);
        assert_eq!(flattened, Vec3::X);
    }

    #[test]
    fn a_direction_straight_up_flattens_to_nothing() {
        assert_eq!(ground_plane_direction(Vec3::Y), Vec3::ZERO);
    }

    fn player_action(app: &App) -> PlayerAction {
        *app.world().resource::<State<PlayerAction>>().get()
    }

    #[test]
    fn the_player_starts_out_selecting() {
        let mut app = input_app();

        tick(&mut app);

        assert_eq!(player_action(&app), PlayerAction::Select);
    }

    #[test]
    fn pressing_the_road_key_holds_the_road_tool() {
        let mut app = input_app();
        tick(&mut app);

        press_key(&mut app, tool_key(PlayerAction::EditRoads));
        tick(&mut app);

        assert_eq!(player_action(&app), PlayerAction::EditRoads);
    }

    #[test]
    fn pressing_the_building_key_holds_the_building_tool() {
        let mut app = input_app();
        tick(&mut app);

        press_key(&mut app, tool_key(PlayerAction::EditBuildings));
        tick(&mut app);

        assert_eq!(player_action(&app), PlayerAction::EditBuildings);
    }

    #[test]
    fn pressing_the_select_key_puts_the_road_tool_down() {
        let mut app = input_app();
        press_key(&mut app, tool_key(PlayerAction::EditRoads));
        tick(&mut app);

        press_key(&mut app, tool_key(PlayerAction::Select));
        tick(&mut app);

        assert_eq!(player_action(&app), PlayerAction::Select);
    }

    #[test]
    fn letting_the_road_key_go_keeps_the_road_tool() {
        let mut app = input_app();
        press_key(&mut app, tool_key(PlayerAction::EditRoads));
        tick(&mut app);

        release_key(&mut app, tool_key(PlayerAction::EditRoads));
        tick(&mut app);

        assert_eq!(player_action(&app), PlayerAction::EditRoads);
    }
}
