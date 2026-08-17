use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

/// Key binding for panning the camera
const CAMERA_PAN_KEY: KeyCode = KeyCode::Space;
/// Key binding for orbiting the camera
const CAMERA_ORBIT_KEY: KeyCode = KeyCode::ShiftLeft;
/// Key binding for orbiting the camera
const CAMERA_ZOOM_KEY: KeyCode = KeyCode::ControlLeft;

pub struct PlayerInputPlugin;

/// The current desired action of the player, controlled by the UI or keyboard shortcuts
#[derive(States, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlayerAction {
    Select,
    EditRoads,
    EditBuildings,
}

/// The current movement type of the camera
#[derive(States, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CameraMovement {
    Translate,
    Pan,
    Orbit,
    Zoom,
}

#[derive(Resource)]
pub struct PlayerInput {
    /// The point in the world where the cursor is pointing
    pub world_cursor_position: Option<Vec3>,
    /// The normalized vector representing the player's movement (WASD)
    pub movement_vector: Vec3,
    /// Whether the player just tap or clicked
    pub tap: bool,
}

impl Default for PlayerInput {
    fn default() -> Self {
        Self {
            world_cursor_position: None,
            movement_vector: Vec3::ZERO,
            tap: false,
        }
    }
}

#[derive(Component)]
#[require(Transform, Visibility)]
struct EditingTargetIndicator;

impl Plugin for PlayerInputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(PlayerAction::Select)
            .insert_state(CameraMovement::Translate)
            .insert_resource(PlayerInput::default())
            .add_systems(Startup, (spawn_indicator, hide_the_cursor))
            .add_systems(
                PreUpdate,
                (update_camera_movement_type, update_player_input).after(InputSystems),
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
    let wanted = if input.pressed(CAMERA_ORBIT_KEY) {
        CameraMovement::Orbit
    } else if input.pressed(CAMERA_ZOOM_KEY) {
        CameraMovement::Zoom
    } else if input.pressed(CAMERA_PAN_KEY) || mouse_input.pressed(MouseButton::Middle) {
        CameraMovement::Pan
    } else {
        CameraMovement::Translate
    };

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
    window: Option<Single<&Window>>,
    camera: Option<Single<(&Camera, &GlobalTransform)>>,
) {
    player_input.tap = mouse_input.just_pressed(MouseButton::Left);

    let Some(camera) = camera else {
        player_input.movement_vector = Vec3::ZERO;
        player_input.world_cursor_position = None;
        return;
    };
    let (camera, camera_transform) = *camera;

    player_input.movement_vector = get_movement_vector(&input, camera_transform);
    player_input.world_cursor_position =
        window.and_then(|window| get_world_cursor_position(&window, camera, camera_transform));
}

/// Calculate the movement vector based on the player's input (WASD) and the camera's orientation
fn get_movement_vector(input: &ButtonInput<KeyCode>, camera_transform: &GlobalTransform) -> Vec3 {
    let mut direction = Vec3::ZERO;
    if input.pressed(KeyCode::KeyW) {
        direction -= Vec3::Z;
    }
    if input.pressed(KeyCode::KeyS) {
        direction += Vec3::Z;
    }
    if input.pressed(KeyCode::KeyA) {
        direction -= Vec3::X;
    }
    if input.pressed(KeyCode::KeyD) {
        direction += Vec3::X;
    }

    ground_plane_direction(camera_transform.rotation() * direction)
}

/// Flatten a direction onto the ground plane, as a unit vector or nothing.
fn ground_plane_direction(direction: Vec3) -> Vec3 {
    Vec3::new(direction.x, 0.0, direction.z).normalize_or_zero()
}

/// Calculate the intersection with the ground plane of the ray originating from the camera and
/// passing through the cursor
fn get_world_cursor_position(
    window: &Window,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<Vec3> {
    window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world(camera_transform, cursor).ok())
        .and_then(|ray| {
            ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::default())
                .map(|distance| ray.get_point(distance))
        })
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
    use crate::testing::{headless_app, press_key, press_mouse, release_key, release_mouse, tick};

    fn input_app() -> App {
        let mut app = headless_app();
        app.add_plugins(PlayerInputPlugin);
        app
    }

    fn spawn_a_camera(app: &mut App) {
        app.world_mut().spawn(Camera3d::default());
    }

    fn spawn_a_primary_window(app: &mut App) {
        app.world_mut()
            .spawn((Window::default(), PrimaryWindow, CursorOptions::default()));
    }

    fn camera_movement(app: &App) -> CameraMovement {
        app.world()
            .resource::<State<CameraMovement>>()
            .get()
            .clone()
    }

    fn player_input(app: &App) -> &PlayerInput {
        app.world().resource::<PlayerInput>()
    }

    #[test]
    fn holding_the_orbit_key_switches_the_camera_to_orbiting() {
        let mut app = input_app();
        tick(&mut app);

        press_key(&mut app, CAMERA_ORBIT_KEY);
        tick(&mut app);

        assert_eq!(camera_movement(&app), CameraMovement::Orbit);
    }

    #[test]
    fn letting_the_orbit_key_go_returns_the_camera_to_translating() {
        let mut app = input_app();
        press_key(&mut app, CAMERA_ORBIT_KEY);
        tick(&mut app);

        release_key(&mut app, CAMERA_ORBIT_KEY);
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
}
