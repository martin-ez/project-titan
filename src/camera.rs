use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};
use std::ops::Range;

/// Translation settings
const TRANSLATION_SENSITIVITY: f32 = 0.12;
const PAN_SENSITIVITY: f32 = 0.02;
const TRANSLATION_TRACKING_DECAY_RATE: f32 = 4.;

/// Rotation settings
const ORBIT_SENSITIVITY: f32 = 0.4 * (PI / 180.0);
const ROTATION_TRACKING_DECAY_RATE: f32 = 8.;
const PITCH_RANGE: Range<f32> = 20.0 * (PI / 180.0)..89.0 * (PI / 180.0);

/// Zoom settings
const ZOOM_RANGE: Range<f32> = 4. ..40.;
const ZOOM_SENSITIVITY: f32 = 0.01;
const SCROLL_LINE_SENSITIVITY: f32 = 4.;
const SCROLL_PIXEL_SENSITIVITY: f32 = 0.5;
const ZOOM_TRACKING_DECAY_RATE: f32 = 12.;

/// Camera controls
const ORBIT_KEY: KeyCode = KeyCode::ShiftLeft;
const ZOOM_KEY: KeyCode = KeyCode::ControlLeft;

pub struct CameraPlugin;

#[derive(States, Debug, Clone, PartialEq, Eq, Hash)]
enum CameraAction {
    Translate,
    Pan,
    Orbit,
    Zoom,
}

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
            .add_systems(Update, (action_input, scroll_zoom))
            .add_systems(FixedUpdate, translate.run_if(not_panning))
            .add_systems(FixedUpdate, pan.run_if(in_state(CameraAction::Pan)))
            .add_systems(FixedUpdate, orbit.run_if(in_state(CameraAction::Orbit)))
            .add_systems(FixedUpdate, zoom.run_if(in_state(CameraAction::Zoom)))
            .add_systems(PostUpdate, smooth_tracking);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands
        .spawn((
            Name::new("PanOrbitCamera"),
            PanOrbitCamera {
                target: Vec3::ZERO,
                radius: 5.0,
                pitch: 25.0f32.to_radians(),
                yaw: 30.0f32.to_radians(),
            },
        ))
        .with_children(|parent| {
            parent.spawn((Name::new("Camera"), Camera3d::default()));
        });
}

fn action_input(
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
            ROTATION_TRACKING_DECAY_RATE,
            time.delta_secs(),
        );
        let mut smooth_radius = camera_transform.translation.length();
        smooth_radius.smooth_nudge(
            &controller.radius,
            ZOOM_TRACKING_DECAY_RATE,
            time.delta_secs(),
        );
        camera_transform.translation = Vec3::ZERO + camera_transform.back() * smooth_radius;
        root_transform.translation.smooth_nudge(
            &controller.target,
            TRANSLATION_TRACKING_DECAY_RATE,
            time.delta_secs(),
        );
    }
}

fn translate(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    camera_q: Query<&GlobalTransform, (With<Camera>, Without<PanOrbitCamera>)>,
    input: Res<ButtonInput<KeyCode>>,
) {
    // TODO: Ray-casting is still a better approach
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
    controller.target += world_translation.normalize_or_zero() * TRANSLATION_SENSITIVITY;
}

fn pan(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    camera_q: Query<&GlobalTransform, (With<Camera>, Without<PanOrbitCamera>)>,
    mut mouse_motion: EventReader<MouseMotion>,
) {
    let total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();
    let translation_vector = Vec3::new(total_motion.x, 0.0, total_motion.y);
    let camera_transform = camera_q.single();
    let mut world_translation = camera_transform.rotation() * translation_vector;
    world_translation.y = 0.0;

    let mut controller = controller_q.single_mut();
    controller.target -= world_translation * PAN_SENSITIVITY;
}

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
    controller.radius = controller.radius.clamp(ZOOM_RANGE.start, ZOOM_RANGE.end);
}

fn zoom(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut mouse_motion: EventReader<MouseMotion>,
) {
    let total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();

    let mut controller = controller_q.single_mut();
    controller.radius *= (total_motion.y * ZOOM_SENSITIVITY).exp();
    controller.radius = controller.radius.clamp(ZOOM_RANGE.start, ZOOM_RANGE.end);
}
