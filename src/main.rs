use bevy::prelude::*;
mod camera;
mod common;
mod diagnostics;
mod input;
mod map;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(camera::CameraPlugin)
        .add_plugins(common::CommonPlugin)
        .add_plugins(diagnostics::DiagnosticsPlugin)
        .add_plugins(input::PlayerInputPlugin)
        .add_plugins(map::MapPlugin)
        .add_systems(Startup, setup_test_scene)
        .run();
}

fn setup_test_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        PointLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::default())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Transform::from_xyz(1.5, 0.5, 1.5),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::default())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Transform::from_xyz(1.5, 0.5, -1.5),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::default())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
        Transform::from_xyz(-1.5, 0.5, 1.5),
    ));
}
