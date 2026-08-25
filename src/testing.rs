//! A headless `App` to drive this game's systems in a test.
//!
//! `MinimalPlugins` on its own cannot hold one of these plugins: states, the input resources and
//! messages and the propagation that gives an entity a `GlobalTransform` all come from plugins
//! outside it, and virtual time advances by however long the last frame happened to take. What
//! this builds instead is an `App` with no window and no renderer whose clock moves exactly one
//! fixed tick per frame, which is the only footing invariant 2 leaves a simulation test to assert
//! from.

use crate::simulation::Simulation;
use bevy::asset::AssetPlugin;
use bevy::gizmos::GizmoAsset;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::mouse::{MouseButtonInput, MouseMotion};
use bevy::input::{ButtonState, InputPlugin};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

/// How many frames a traced run may spend on one tick before it has plainly stopped ticking.
const FRAMES_ALLOWED_A_TICK: usize = 64;

/// What a traced run has seen so far, one reading per tick.
#[derive(Resource)]
struct Traced<T: Send + Sync + 'static>(Vec<T>);

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
    .init_asset::<GizmoAsset>()
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
    advance(app, timestep);
}

/// Advance the app by one frame lasting `delta` of real time.
///
/// Real time moves by exactly `delta` and virtual time derives from it, so a test can vary the
/// frame rate, the fixed timestep and the simulation's speed multiplier one at a time and watch
/// which of them a system answers to. An app's very first frame is the exception: it is where
/// `Time<Real>` takes its baseline, so it reports no delta at all and a system reading one does
/// nothing. Take a frame before measuring.
pub fn advance(app: &mut App, delta: Duration) {
    app.insert_resource(TimeUpdateStrategy::ManualDuration(delta));
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

/// Run `app` until the simulation has carried `ticks` ticks, reading it on each of them.
///
/// The reading is taken on the tick rather than on the frame, so two runs whose frames divide the
/// same ticks differently still line up entry for entry — which is what lets a run at one frame
/// rate be compared with a run at another (invariant 2). Frame lengths cycle through `frames`, so
/// one length is a steady rate and several are a ragged one.
///
/// The app is handed over already built, so the tick that lays a road is not a tick of the run.
pub fn trace<T: Send + Sync + 'static>(
    mut app: App,
    frames: &[Duration],
    ticks: usize,
    observe: fn(&World) -> T,
) -> Vec<T> {
    app.insert_resource(Traced::<T>(Vec::new()));
    app.add_systems(
        FixedUpdate,
        (move |world: &mut World| {
            let seen = observe(world);
            world.resource_mut::<Traced<T>>().0.push(seen);
        })
        .after(Simulation),
    );

    let mut lengths = frames.iter().copied().cycle();
    for _ in 0..ticks * FRAMES_ALLOWED_A_TICK {
        if app.world().resource::<Traced<T>>().0.len() >= ticks {
            break;
        }
        advance(&mut app, lengths.next().expect("a run has a frame length"));
    }

    let mut traced = app
        .world_mut()
        .remove_resource::<Traced<T>>()
        .expect("the run recorded what it saw")
        .0;
    assert!(
        traced.len() >= ticks,
        "the run carried {} of the {ticks} ticks asked of it",
        traced.len()
    );
    traced.truncate(ticks);
    traced
}
