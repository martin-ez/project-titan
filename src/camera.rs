use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use std::f32::consts::PI;
use std::ops::Range;

pub struct CameraPlugin;

#[derive(Debug, Resource)]
struct CameraSettings {
    pub zoom_range: Range<f32>,
    pub zoom_speed: f32,
    pub translation_speed: f32,
    pub rotation_speed: f32,
    pub orbit_speed: f32,
    pub pan_speed: f32,
}

impl CameraSettings {
    fn default() -> Self {
        Self {
            zoom_range: (PI / 5.)..(PI - 0.2),
            zoom_speed: 0.25,
            translation_speed: 5.0,
            rotation_speed: 2.0,
            orbit_speed: 2.0,
            pan_speed: 1.6,
        }
    }
}

#[derive(Component)]
#[require(Transform, InheritedVisibility)]
struct CameraTarget;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CameraSettings::default())
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    camera_translation,
                    camera_zoom,
                    camera_rotation,
                    camera_pan,
                    camera_orbit,
                ),
            );
    }
}

fn setup(
    camera_settings: Res<CameraSettings>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Spawn camera with orthographic projection
    commands
        .spawn((
            Name::new("CameraTarget"),
            CameraTarget {},
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .with_children(|parent| {
            parent.spawn((
                Name::new("Camera"),
                Camera3d::default(),
                Projection::from(PerspectiveProjection {
                    fov: camera_settings.zoom_range.start,
                    ..default()
                }),
                Transform::from_xyz(5.0, 6.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ));
            // Indicator for the camera target
            parent.spawn((
                Mesh3d(meshes.add(Sphere::new(0.1))),
                MeshMaterial3d(materials.add(Color::srgb(0.8, 0.3, 0.2))),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
        });
}

fn camera_translation(
    mut target_query: Query<&mut Transform, With<CameraTarget>>,
    camera_query: Query<&GlobalTransform, With<Camera>>,
    camera_settings: Res<CameraSettings>,
    time: Res<Time>,
    input: Res<ButtonInput<KeyCode>>,
) {
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

    let camera_global_transform = camera_query.single();
    let mut world_translation = camera_global_transform.rotation() * direction;
    // Remove the vertical component of the direction vector
    world_translation.y = 0.0;
    target_query.single_mut().translation +=
        world_translation * time.delta_secs() * camera_settings.translation_speed;
}

fn camera_rotation(
    mut query: Query<&mut Transform, With<CameraTarget>>,
    time: Res<Time>,
    input: Res<ButtonInput<KeyCode>>,
    camera_settings: Res<CameraSettings>,
) {
    let rotation = time.delta_secs() * camera_settings.rotation_speed;
    if input.pressed(KeyCode::KeyQ) {
        query.single_mut().rotate_y(-rotation);
    }
    if input.pressed(KeyCode::KeyE) {
        query.single_mut().rotate_y(rotation);
    }
}

fn camera_zoom(
    mut camera_query: Query<&mut Transform, (With<Camera>, Without<CameraTarget>)>,
    camera_settings: Res<CameraSettings>,
    mouse_wheel_input: Res<AccumulatedMouseScroll>,
) {
    let delta_zoom = -mouse_wheel_input.delta.y * camera_settings.zoom_speed;
    let mut camera_transform = camera_query.single_mut();
    let direction = -camera_transform.translation.normalize_or_zero();
    // TODO - Clamp the zoom range based on the distance from the target?
    camera_transform.translation += direction * delta_zoom;
}

fn camera_pan(
    mut target_query: Query<&mut Transform, With<CameraTarget>>,
    camera_query: Query<&GlobalTransform, With<Camera>>,
    mut last_mouse_position: Local<Option<Vec2>>,
    camera_settings: Res<CameraSettings>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    key_input: Res<ButtonInput<KeyCode>>,
    window: Single<&Window>,
    time: Res<Time>,
) {
    // TODO - Refactor to use ray-casting or something similar
    if let Some(cursor_position) = window.cursor_position() {
        if mouse_input.pressed(MouseButton::Left) && !key_input.pressed(KeyCode::ControlLeft) {
            if let Some(last_position) = *last_mouse_position {
                let delta = cursor_position - last_position;
                let camera_global_transform = camera_query.single();
                let right = camera_global_transform.right();
                let forward = -camera_global_transform.forward();
                let world_delta = (right * delta.x + forward * delta.y)
                    * camera_settings.pan_speed
                    * time.delta_secs();
                let mut transform = target_query.single_mut();
                transform.translation -= world_delta;
                transform.translation.y = 0.0;
            }

            *last_mouse_position = Some(cursor_position);
        } else {
            *last_mouse_position = None;
        }
    }
}

fn camera_orbit(
    target_query: Query<&Transform, With<CameraTarget>>,
    mut camera_query: Query<&mut Transform, (With<Camera>, Without<CameraTarget>)>,
    mut last_mouse_position: Local<Option<Vec2>>,
    camera_settings: Res<CameraSettings>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    key_input: Res<ButtonInput<KeyCode>>,
    window: Single<&Window>,
    time: Res<Time>,
) {
    // TODO - Some issues on top-down view
    if let Some(cursor_position) = window.cursor_position() {
        if mouse_input.pressed(MouseButton::Left) && key_input.pressed(KeyCode::ControlLeft) {
            if let Some(last_position) = *last_mouse_position {
                let delta = cursor_position - last_position;
                let target_transform = target_query.single();
                let mut camera_transform = camera_query.single_mut();

                let yaw = -delta.x * camera_settings.orbit_speed * time.delta_secs();
                let pitch = delta.y * camera_settings.orbit_speed * time.delta_secs();

                let target_position = target_transform.translation;

                // Translate camera to origin for rotation
                let offset = camera_transform.translation - target_position;

                // Apply yaw (rotation around Y-axis in world space)
                let yaw_rotation = Quat::from_rotation_y(yaw);

                // Apply pitch (rotation around the camera's local X-axis)
                let right = offset.normalize().cross(Vec3::Y).normalize(); // Camera's right vector
                let pitch_rotation = Quat::from_axis_angle(right, pitch);

                // Combine rotations and apply to the offset
                let rotated_offset = yaw_rotation * pitch_rotation * offset;

                // Update the camera's position
                camera_transform.translation = target_position + rotated_offset;

                // Make the camera look at the target
                camera_transform.look_at(target_position, Vec3::Y);
            }

            *last_mouse_position = Some(cursor_position);
        } else {
            *last_mouse_position = None;
        }
    }
}
