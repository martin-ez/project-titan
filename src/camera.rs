use crate::input::{CameraMovement, PlayerInput};
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
const ZOOM_RADIUS_RANGE: Range<f32> = 8. ..100.;
/// For devices with a notched scroll wheel
const SCROLL_LINE_SENSITIVITY: f32 = 4.;
/// For devices with smooth scrolling (e.g. touchpad)
const SCROLL_PIXEL_SENSITIVITY: f32 = 0.5;
/// Smoothing factor for the camera zoom
const ZOOM_SMOOTHING: f32 = 12.;

pub struct CameraPlugin;

/// Internal state for the camera, use to construct its transform
#[derive(Component)]
#[require(Transform, InheritedVisibility)]
struct PanOrbitCamera {
    target: Vec3,
    radius: f32,
    pitch: f32,
    yaw: f32,
}

fn not_panning(state: Res<State<CameraMovement>>) -> bool {
    *state != CameraMovement::Pan
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, (scroll_zoom, pan))
            .add_systems(
                FixedUpdate,
                (
                    translate.run_if(not_panning),
                    orbit.run_if(in_state(CameraMovement::Orbit)),
                    mouse_zoom.run_if(in_state(CameraMovement::Zoom)),
                ),
            )
            .add_systems(PostUpdate, smooth_tracking);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands
        .spawn((
            Name::new("PanOrbitCamera"),
            PanOrbitCamera {
                target: Vec3::ZERO,
                radius: 20.0,
                pitch: 30.0f32.to_radians(),
                yaw: -90.0f32.to_radians(),
            },
        ))
        .with_children(|parent| {
            parent.spawn((Name::new("Camera"), Camera3d::default()));
        });
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
    player_input: Res<PlayerInput>,
) {
    let mut controller = controller_q.single_mut();
    let zoom_multiplier = (controller.radius * TRANSLATION_ZOOM_MULTIPLIER).exp();
    controller.target += player_input.movement_vector * TRANSLATION_SENSITIVITY * zoom_multiplier;
}

/// Pan the camera based on the mouse motion
///
/// This works by calculating the point at the intersection of the cursor ray and the ground plane,
/// ensuring that point remains under the cursor as long as the user is holding the mouse button.
fn pan(
    mut controller_q: Query<(&mut PanOrbitCamera, &mut Transform), With<PanOrbitCamera>>,
    mut selected_point: Local<Option<Vec3>>,
    mut inertia: Local<Option<Vec3>>,
    player_input: Res<PlayerInput>,
    action: Res<State<CameraMovement>>,
    time: Res<Time>,
) {
    for (mut controller, mut transform) in &mut controller_q {
        match action.get() {
            CameraMovement::Pan => {
                if let Some(point) = player_input.world_cursor_position {
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
            CameraMovement::Translate => {
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
