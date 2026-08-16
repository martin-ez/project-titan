use bevy::prelude::*;
use bevy::window::PrimaryWindow;

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
            .add_systems(Startup, setup)
            .add_systems(
                PreUpdate,
                (update_camera_movement_type, update_player_input),
            )
            .add_systems(Update, update_indicator);
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut window_q: Query<&mut Window, With<PrimaryWindow>>,
) {
    commands.spawn((
        EditingTargetIndicator {},
        Visibility::Hidden,
        Mesh3d(meshes.add(Sphere::new(0.1))),
        MeshMaterial3d(materials.add(Color::srgb(0.1, 0.2, 0.9))),
    ));
    let mut primary_window = window_q.single_mut();
    primary_window.cursor_options.visible = false;
}

/// Update the camera movement type based on the player's input
fn update_camera_movement_type(
    input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut next_state: ResMut<NextState<CameraMovement>>,
) {
    if input.pressed(CAMERA_ORBIT_KEY) {
        next_state.set(CameraMovement::Orbit);
    } else if input.pressed(CAMERA_ZOOM_KEY) {
        next_state.set(CameraMovement::Zoom);
    } else if input.pressed(CAMERA_PAN_KEY) || mouse_input.pressed(MouseButton::Middle) {
        next_state.set(CameraMovement::Pan);
    } else {
        next_state.set(CameraMovement::Translate);
    }
}

fn update_player_input(
    mut movement_vector: ResMut<PlayerInput>,
    input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    window: Single<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
) {
    let (camera, camera_transform) = camera.single();
    movement_vector.world_cursor_position =
        get_world_cursor_position(window, camera, camera_transform);
    movement_vector.movement_vector = get_movement_vector(input, camera_transform);
    movement_vector.tap = mouse_input.just_pressed(MouseButton::Left);
}

/// Calculate the movement vector based on the player's input (WASD) and the camera's orientation
fn get_movement_vector(
    input: Res<ButtonInput<KeyCode>>,
    camera_transform: &GlobalTransform,
) -> Vec3 {
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
    window: Single<&Window>,
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
    let (mut indicator, mut visibility) = indicator_q.single_mut();
    if let Some(point) = player_input.world_cursor_position {
        indicator.translation = point;
        *visibility = Visibility::Visible;
    } else {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
