use bevy::prelude::*;
mod building;
mod camera;
mod common;
mod diagnostics;
mod input;
mod map;
mod simulation;
#[cfg(test)]
mod testing;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(building::BuildingPlugin)
        .add_plugins(camera::CameraPlugin)
        .add_plugins(common::CommonPlugin)
        .add_plugins(diagnostics::DiagnosticsPlugin)
        .add_plugins(input::PlayerInputPlugin)
        .add_plugins(map::MapPlugin)
        .add_plugins(simulation::SimulationPlugin)
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
    for place in [
        Vec3::new(1.5, 0.5, 1.5),
        Vec3::new(1.5, 0.5, -1.5),
        Vec3::new(-1.5, 0.5, 1.5),
    ] {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::default())),
            MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
            Transform::from_translation(place),
            box_surface(),
        ));
    }
}

/// The top of one of the test scene's unit boxes, so the cursor has something to climb.
fn box_surface() -> common::cursor::CursorSurface {
    common::cursor::CursorSurface {
        radius: 0.5,
        height: 0.5,
    }
}
