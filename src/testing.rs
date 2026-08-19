//! A headless `App` to drive this game's systems in a test.
//!
//! `MinimalPlugins` on its own cannot hold one of these plugins: states, the input resources and
//! messages and the propagation that gives an entity a `GlobalTransform` all come from plugins
//! outside it, and virtual time advances by however long the last frame happened to take. What
//! this builds instead is an `App` with no window and no renderer whose clock moves exactly one
//! fixed tick per frame, which is the only footing invariant 2 leaves a simulation test to assert
//! from.

use bevy::asset::AssetPlugin;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::mouse::{MouseButtonInput, MouseMotion};
use bevy::input::{ButtonState, InputPlugin};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;

/// An `App` carrying what a plugin needs to run, and nothing that opens a window.
pub fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        InputPlugin,
        TransformPlugin,
        AssetPlugin::default(),
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>();
    app
}

/// Advance the app by one frame, which under this clock is exactly one fixed tick.
///
/// Virtual time is stepped by the fixed timestep itself, so `FixedUpdate` runs once per call and
/// leaves the accumulator empty. A system reading messages on the fixed tick therefore sees each
/// message exactly once, however long the test's own frame took.
pub fn tick(app: &mut App) {
    let timestep = app.world().resource::<Time<Fixed>>().timestep();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(timestep));
    app.update();
}

/// Move the mouse by `delta`, to be seen on the next tick.
pub fn move_mouse(app: &mut App, delta: Vec2) {
    app.world_mut().write_message(MouseMotion { delta });
}

/// Press `key`, to be seen on the next tick.
pub fn press_key(app: &mut App, key: KeyCode) {
    let message = key_message(key, ButtonState::Pressed);
    app.world_mut().write_message(message);
}

/// Release `key`, to be seen on the next tick.
pub fn release_key(app: &mut App, key: KeyCode) {
    let message = key_message(key, ButtonState::Released);
    app.world_mut().write_message(message);
}

/// Press `button`, to be seen on the next tick.
pub fn press_mouse(app: &mut App, button: MouseButton) {
    let message = mouse_message(button, ButtonState::Pressed);
    app.world_mut().write_message(message);
}

/// Release `button`, to be seen on the next tick.
pub fn release_mouse(app: &mut App, button: MouseButton) {
    let message = mouse_message(button, ButtonState::Released);
    app.world_mut().write_message(message);
}

fn mouse_message(button: MouseButton, state: ButtonState) -> MouseButtonInput {
    MouseButtonInput {
        button,
        state,
        window: Entity::PLACEHOLDER,
    }
}

fn key_message(key_code: KeyCode, state: ButtonState) -> KeyboardInput {
    KeyboardInput {
        key_code,
        logical_key: Key::Unidentified(NativeKey::Unidentified),
        state,
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    }
}
