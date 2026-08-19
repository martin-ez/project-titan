use crate::input::{CameraMovement, PlayerInput};
use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};
use std::ops::Range;

/// Speed of the camera when translating with WASD, in world units per second.
///
/// Settled by play testing as 0.12 per tick of the 64 Hz clock it used to run on.
const TRANSLATION_SENSITIVITY: f32 = 7.68;
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

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (
                    scroll_zoom,
                    pan,
                    translate.run_if(not(in_state(CameraMovement::Pan))),
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
    time: Res<Time<Real>>,
) -> Result {
    for (mut root_transform, controller) in &mut controller_query {
        let mut camera_transform = camera_query.single_mut()?;
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
    Ok(())
}

/// Translate the camera using the WASD keys
fn translate(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    player_input: Res<PlayerInput>,
    time: Res<Time<Real>>,
) -> Result {
    let mut controller = controller_q.single_mut()?;
    let zoom_multiplier = (controller.radius * TRANSLATION_ZOOM_MULTIPLIER).exp();
    let step = TRANSLATION_SENSITIVITY * zoom_multiplier * time.delta_secs();
    controller.target += player_input.movement_vector * step;
    Ok(())
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
    time: Res<Time<Real>>,
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
    mut mouse_motion: MessageReader<MouseMotion>,
) -> Result {
    let mut total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();
    total_motion.y = -total_motion.y;

    let orbit = -total_motion * ORBIT_SENSITIVITY;
    let mut controller = controller_q.single_mut()?;
    controller.yaw += orbit.x;
    controller.pitch += orbit.y;
    controller.pitch = controller.pitch.clamp(PITCH_RANGE.start, PITCH_RANGE.end);
    controller.yaw = wrap_to_half_turn(controller.yaw);
    controller.pitch = wrap_to_half_turn(controller.pitch);
    Ok(())
}

/// Bring an angle back inside a half turn either side of zero.
fn wrap_to_half_turn(angle: f32) -> f32 {
    if angle > PI {
        angle - TAU
    } else if angle < -PI {
        angle + TAU
    } else {
        angle
    }
}

/// Zoom the camera based on the scroll wheel or trackpad input
fn scroll_zoom(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut evr_scroll: MessageReader<MouseWheel>,
) -> Result {
    let mut zoom = 0.;
    for ev in evr_scroll.read() {
        zoom -= ev.y;
        match ev.unit {
            MouseScrollUnit::Line => zoom *= SCROLL_LINE_SENSITIVITY * ZOOM_SENSITIVITY,
            MouseScrollUnit::Pixel => zoom *= SCROLL_PIXEL_SENSITIVITY * ZOOM_SENSITIVITY,
        }
    }

    let mut controller = controller_q.single_mut()?;
    controller.radius *= (-zoom).exp();
    controller.radius = controller
        .radius
        .clamp(ZOOM_RADIUS_RANGE.start, ZOOM_RADIUS_RANGE.end);
    Ok(())
}

/// Zoom the camera based on the mouse motion
fn mouse_zoom(
    mut controller_q: Query<&mut PanOrbitCamera, With<PanOrbitCamera>>,
    mut mouse_motion: MessageReader<MouseMotion>,
) -> Result {
    let total_motion: Vec2 = mouse_motion.read().map(|motion| motion.delta).sum();

    let mut controller = controller_q.single_mut()?;
    controller.radius *= (total_motion.y * ZOOM_SENSITIVITY).exp();
    controller.radius = controller
        .radius
        .clamp(ZOOM_RADIUS_RANGE.start, ZOOM_RADIUS_RANGE.end);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{advance, headless_app, move_mouse, tick};
    use std::time::Duration;

    /// A frame short enough that virtual time never reaches its maximum delta.
    const FRAME: Duration = Duration::from_millis(10);

    fn camera_app(movement: CameraMovement) -> App {
        let mut app = headless_app();
        app.insert_state(movement)
            .insert_resource(PlayerInput::default())
            .add_plugins(CameraPlugin);
        app
    }

    fn translating_app() -> App {
        let mut app = camera_app(CameraMovement::Translate);
        app.world_mut()
            .resource_mut::<PlayerInput>()
            .movement_vector = Vec3::X;
        app
    }

    fn advance_by(app: &mut App, frames: u32) {
        for _ in 0..frames {
            advance(app, FRAME);
        }
    }

    fn translated_over(app: &mut App, frames: u32, frame: Duration) -> Vec3 {
        advance(app, frame);
        let before = controller(app, |camera| camera.target);
        for _ in 0..frames {
            advance(app, frame);
        }
        controller(app, |camera| camera.target) - before
    }

    fn set_target(app: &mut App, target: Vec3) {
        let mut query = app.world_mut().query::<&mut PanOrbitCamera>();
        let mut controller = query
            .iter_mut(app.world_mut())
            .next()
            .expect("the plugin spawns a controller on startup");
        controller.target = target;
    }

    fn camera_translation(app: &mut App) -> Vec3 {
        let mut query = app
            .world_mut()
            .query_filtered::<&Transform, With<PanOrbitCamera>>();
        query
            .iter(app.world())
            .next()
            .expect("the plugin spawns a controller on startup")
            .translation
    }

    fn controller<T>(app: &mut App, read: impl Fn(&PanOrbitCamera) -> T) -> T {
        let mut query = app.world_mut().query::<&PanOrbitCamera>();
        let controller = query
            .iter(app.world())
            .next()
            .expect("the plugin spawns a controller on startup");
        read(controller)
    }

    #[test]
    fn mouse_motion_orbits_the_camera() {
        let mut app = camera_app(CameraMovement::Orbit);
        tick(&mut app);
        let before = controller(&mut app, |camera| camera.yaw);

        move_mouse(&mut app, Vec2::new(10.0, 0.0));
        tick(&mut app);

        let expected = before - 10.0 * ORBIT_SENSITIVITY;
        assert!((controller(&mut app, |camera| camera.yaw) - expected).abs() < 1e-5);
    }

    #[test]
    fn the_pitch_stops_at_the_top_of_its_range() {
        let mut app = camera_app(CameraMovement::Orbit);
        tick(&mut app);

        move_mouse(&mut app, Vec2::new(0.0, 10_000.0));
        tick(&mut app);

        assert_eq!(controller(&mut app, |camera| camera.pitch), PITCH_RANGE.end);
    }

    #[test]
    fn the_camera_does_not_orbit_while_translating() {
        let mut app = camera_app(CameraMovement::Translate);
        tick(&mut app);
        let before = controller(&mut app, |camera| camera.yaw);

        move_mouse(&mut app, Vec2::new(10.0, 0.0));
        tick(&mut app);

        assert_eq!(controller(&mut app, |camera| camera.yaw), before);
    }

    #[test]
    fn the_camera_translates_along_the_movement_vector() {
        let mut app = camera_app(CameraMovement::Translate);
        tick(&mut app);
        let before = controller(&mut app, |camera| camera.target);

        app.world_mut()
            .resource_mut::<PlayerInput>()
            .movement_vector = Vec3::X;
        tick(&mut app);

        let after = controller(&mut app, |camera| camera.target);
        assert!(after.x > before.x);
        assert_eq!(after.y, before.y);
    }

    #[test]
    fn the_camera_translates_the_same_distance_at_any_frame_rate() {
        let in_one_frame = translated_over(&mut translating_app(), 1, 10 * FRAME);
        let in_ten = translated_over(&mut translating_app(), 10, FRAME);

        assert!((in_one_frame - in_ten).length() < 1e-4);
        assert!(in_one_frame.x > 0.0);
    }

    #[test]
    fn the_camera_translates_at_the_same_speed_whatever_the_tick_rate() {
        let mut slow = translating_app();
        slow.insert_resource(Time::<Fixed>::from_hz(8.0));
        let mut fast = translating_app();
        fast.insert_resource(Time::<Fixed>::from_hz(256.0));

        let travelled = translated_over(&mut slow, 10, FRAME);

        assert!((travelled - translated_over(&mut fast, 10, FRAME)).length() < 1e-4);
        assert!(travelled.x > 0.0);
    }

    #[test]
    fn the_camera_translates_at_the_same_speed_when_the_simulation_runs_fast() {
        let mut normal = translating_app();
        let mut fast = translating_app();
        fast.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(4.0);

        let travelled = translated_over(&mut normal, 10, FRAME);

        assert!((travelled - translated_over(&mut fast, 10, FRAME)).length() < 1e-4);
        assert!(travelled.x > 0.0);
    }

    #[test]
    fn the_camera_eases_at_the_same_rate_when_the_simulation_runs_fast() {
        let mut normal = camera_app(CameraMovement::Translate);
        let mut fast = camera_app(CameraMovement::Translate);
        fast.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(4.0);
        advance_by(&mut normal, 1);
        advance_by(&mut fast, 1);
        set_target(&mut normal, Vec3::X * 10.0);
        set_target(&mut fast, Vec3::X * 10.0);

        advance_by(&mut normal, 10);
        advance_by(&mut fast, 10);

        let eased = camera_translation(&mut normal);
        assert!((eased - camera_translation(&mut fast)).length() < 1e-4);
        assert!(eased.x > 0.0);
    }

    #[test]
    fn the_camera_translates_on_a_frame_with_no_fixed_tick() {
        let mut app = translating_app();
        app.insert_resource(Time::<Fixed>::from_seconds(1_000.0));
        advance_by(&mut app, 1);
        let before = controller(&mut app, |camera| camera.target);

        advance_by(&mut app, 1);

        assert!(controller(&mut app, |camera| camera.target).x > before.x);
    }

    #[test]
    fn the_camera_orbits_on_a_frame_with_no_fixed_tick() {
        let mut app = camera_app(CameraMovement::Orbit);
        app.insert_resource(Time::<Fixed>::from_seconds(1_000.0));
        advance_by(&mut app, 1);
        let before = controller(&mut app, |camera| camera.yaw);

        move_mouse(&mut app, Vec2::new(10.0, 0.0));
        advance_by(&mut app, 1);

        assert_ne!(controller(&mut app, |camera| camera.yaw), before);
    }

    #[test]
    fn the_camera_zooms_on_a_frame_with_no_fixed_tick() {
        let mut app = camera_app(CameraMovement::Zoom);
        app.insert_resource(Time::<Fixed>::from_seconds(1_000.0));
        advance_by(&mut app, 1);
        let before = controller(&mut app, |camera| camera.radius);

        move_mouse(&mut app, Vec2::new(0.0, 10.0));
        advance_by(&mut app, 1);

        assert_ne!(controller(&mut app, |camera| camera.radius), before);
    }

    #[test]
    fn an_angle_past_half_a_turn_wraps_below_it() {
        assert!((wrap_to_half_turn(PI + 0.5) - (-PI + 0.5)).abs() < 1e-5);
    }

    #[test]
    fn an_angle_below_minus_half_a_turn_wraps_above_it() {
        assert!((wrap_to_half_turn(-PI - 0.5) - (PI - 0.5)).abs() < 1e-5);
    }

    #[test]
    fn an_angle_inside_the_range_is_left_alone() {
        assert_eq!(wrap_to_half_turn(1.0), 1.0);
    }
}
