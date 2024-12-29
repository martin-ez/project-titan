use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};
use std::ops::Range;

/// Speed of the camera when translating with WASD
const TRANSLATION_SENSITIVITY: f32 = 0.12;
/// How much the speed of the camera increases when zooming out
const TRANSLATION_ZOOM_MULTIPLIER: f32 = 0.04;
/// Smoothing factor for the camera translation
const TRANSLATION_SMOOTHING: f32 = 4.;
/// Damping factor for the panning inertia
const PAN_INERTIA_DAMPING: f32 = 0.08;
/// Radians per pixel of mouse motion
const ORBIT_SENSITIVITY: f32 = 0.4 * (PI / 180.0);
/// Smoothing factor for the camera rotation
const ROTATION_SMOOTHING: f32 = 8.;
/// Minimum and maximum pitch allowed for the camera
const PITCH_RANGE: Range<f32> = 20.0 * (PI / 180.0)..89.0 * (PI / 180.0);
/// Exponent per pixel of mouse motion
const ZOOM_SENSITIVITY: f32 = 0.01;
/// Minimum and maximum radius allowed for the camera
const ZOOM_RADIUS_RANGE: Range<f32> = 4. ..40.;
/// For devices with a notched scroll wheel
const SCROLL_LINE_SENSITIVITY: f32 = 4.;
/// For devices with smooth scrolling (e.g. touchpad)
const SCROLL_PIXEL_SENSITIVITY: f32 = 0.5;
/// Smoothing factor for the camera zoom
const ZOOM_SMOOTHING: f32 = 12.;
/// Key binding for orbiting the camera
const ORBIT_KEY: KeyCode = KeyCode::ShiftLeft;
/// Key binding for orbiting the camera
const ZOOM_KEY: KeyCode = KeyCode::ControlLeft;

pub struct CameraPlugin;

/// The action the camera is currently performing, based on the player's input
#[derive(States, Debug, Clone, PartialEq, Eq, Hash)]
enum CameraAction {
    Translate,
    Pan,
    Orbit,
    Zoom,
}

/// Internal state for the camera, use to construct its transform
#[derive(Component)]
#[require(Transform, InheritedVisibility)]
struct PanOrbitCamera {
    target: Vec3,
    radius: f32,
    pitch: f32,
    yaw: f32,
}

fn not_panning(state: Res<State<CameraAction>>) -> bool {
    *state != CameraAction::Pan
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(CameraAction::Translate)
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (update_action, scroll_zoom, pan))
            .add_systems(FixedUpdate, translate.run_if(not_panning))
            .add_systems(FixedUpdate, orbit.run_if(in_state(CameraAction::Orbit)))
            .add_systems(FixedUpdate, mouse_zoom.run_if(in_state(CameraAction::Zoom)))
            .add_systems(PostUpdate, smooth_tracking);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands
        .spawn((
            Name::new("PanOrbitCamera"),
            PanOrbitCamera {
                target: Vec3::ZERO,
                radius: 8.0,
                pitch: 25.0f32.to_radians(),
                yaw: 30.0f32.to_radians(),
            },
        ))
        .with_children(|parent| {
            parent.spawn((Name::new("Camera"), Camera3d::default()));
        });
}

/// Update the camera action based on the player's input
fn update_action(
    input: Res<ButtonInput<KeyCode>>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut next_state: ResMut<NextState<CameraAction>>,
) {
    if mouse_input.pressed(MouseButton::Left) {
        if input.pressed(ORBIT_KEY) {
            next_state.set(CameraAction::Orbit);
        } else if input.pressed(ZOOM_KEY) {
            next_state.set(CameraAction::Zoom);
        } else {
            next_state.set(CameraAction::Pan);
        }
    } else {
        next_state.set(CameraAction::Translate);
    }
}

/// Smoothly update the camera's position and rotation based on the internal state.
fn smooth_tracking(
    mut controller_query: Query<(&mut Transform, &PanOrbitCamera), With<PanOrbitCamera>>,
    mut camera_query: Query<&mut Transform, (With<Camera>, Without<PanOrbitCamera>)>,
    time: Res<Time>,
) {
    for (mut root_transform, controller) in &mut controller_query {
        let mut camera_transform = camera_query.single_mut();
        let target_rotation =
            Quat::from_euler(EulerRot::YXZ, controller.yaw, -controller.pitch, 0.0);

        camera_transform.rotation.smooth_nudge(
            &target_rotation,
            ROTATION_SMOOTHING,
            time.delta_secs(),
        );
        let mut smooth_radius = camera_transform.translation.length();
        smooth_radius.smooth_nudge(&controller.radius, ZOOM_SMOOTHING, time.delta_secs());
        camera_transform.translation = Vec3::ZERO + camera_transform.back() * smooth_radius;
        root_transform.translation.smooth_nudge(
            &controller.target,
            TRANSLATION_SMOOTHING,
            time.delta_secs(),
        );
    }
}

/// Translate the camera using the WASD keys
fn translate(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    camera_q: Query<&GlobalTransform, (With<Camera>, Without<PanOrbitCamera>)>,
    input: Res<ButtonInput<KeyCode>>,
) {
    let mut controller = controller_q.single_mut();
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

    let camera_transform = camera_q.single();
    let mut world_translation = camera_transform.rotation() * direction;
    // Remove the vertical component of the direction vector
    world_translation.y = 0.0;
    let zoom_multiplier = (controller.radius * TRANSLATION_ZOOM_MULTIPLIER).exp();
    controller.target +=
        world_translation.normalize_or_zero() * TRANSLATION_SENSITIVITY * zoom_multiplier;
}

/// Pan the camera based on the mouse motion
///
/// This works by calculating the point at the intersection of the cursor ray and the ground plane,
/// ensuring that point remains under the cursor as long as the user is holding the mouse button.
fn pan(
    mut controller_q: Query<(&mut PanOrbitCamera, &mut Transform), With<PanOrbitCamera>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    mut selected_point: Local<Option<Vec3>>,
    mut inertia: Local<Option<Vec3>>,
    action: Res<State<CameraAction>>,
    time: Res<Time>,
) {
    let (camera, camera_transform) = *camera;
    for (mut controller, mut transform) in &mut controller_q {
        match action.get() {
            CameraAction::Pan => {
                if let Some(point) = cursor_ground_intersection(*window, camera, camera_transform) {
                    if let Some(last_point) = *selected_point {
                        let delta = last_point - point;
                        controller.target += delta;
                        transform.translation += delta;
                        *inertia = Some(delta / time.delta_secs());
                    } else {
                        *selected_point = Some(point);
                        *inertia = None;
                    }
                } else {
                    *selected_point = None;
                    *inertia = None;
                }
            }
            CameraAction::Translate => {
                // Move target based on the inertia
                if let Some(inertia) = *inertia {
                    controller.target += inertia * PAN_INERTIA_DAMPING;
                }

                *selected_point = None;
                *inertia = None;
            }
            _ => {
                *selected_point = None;
                *inertia = None;
            }
        }
    }
}

/// Calculate the intersection with the ground plane of the ray originating from the camera and
/// passing through the cursor
// TODO: We should modify this to track terrain and game objects instead
fn cursor_ground_intersection(
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

/// Orbit the camera around the target based on the mouse motion
fn orbit(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut mouse_motion: EventReader<MouseMotion>,
) {
    let mut total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();
    total_motion.y = -total_motion.y;

    let orbit = -total_motion * ORBIT_SENSITIVITY;
    let mut controller = controller_q.single_mut();
    controller.yaw += orbit.x;
    controller.pitch += orbit.y;
    controller.pitch = controller.pitch.clamp(PITCH_RANGE.start, PITCH_RANGE.end);
    // wrap around, to stay between +- 180 degrees
    if controller.yaw > PI {
        controller.yaw -= TAU;
    }
    if controller.yaw < -PI {
        controller.yaw += TAU;
    }
    if controller.pitch > PI {
        controller.pitch -= TAU;
    }
    if controller.pitch < -PI {
        controller.pitch += TAU;
    }
}

/// Zoom the camera based on the scroll wheel or trackpad input
fn scroll_zoom(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut evr_scroll: EventReader<MouseWheel>,
) {
    let mut zoom = 0.;
    for ev in evr_scroll.read() {
        zoom -= ev.y;
        match ev.unit {
            MouseScrollUnit::Line => zoom *= SCROLL_LINE_SENSITIVITY * ZOOM_SENSITIVITY,
            MouseScrollUnit::Pixel => zoom *= SCROLL_PIXEL_SENSITIVITY * ZOOM_SENSITIVITY,
        }
    }

    let mut controller = controller_q.single_mut();
    controller.radius *= (-zoom).exp();
    controller.radius = controller
        .radius
        .clamp(ZOOM_RADIUS_RANGE.start, ZOOM_RADIUS_RANGE.end);
}

/// Zoom the camera based on the mouse motion
fn mouse_zoom(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut mouse_motion: EventReader<MouseMotion>,
) {
    let total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();

    let mut controller = controller_q.single_mut();
    controller.radius *= (total_motion.y * ZOOM_SENSITIVITY).exp();
    controller.radius = controller
        .radius
        .clamp(ZOOM_RADIUS_RANGE.start, ZOOM_RADIUS_RANGE.end);
}
