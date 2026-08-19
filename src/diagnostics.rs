use bevy::dev_tools::diagnostics_overlay::{DiagnosticsOverlay, DiagnosticsOverlayPlugin};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

/// Key binding that shows and hides the diagnostics overlay
const DIAGNOSTICS_OVERLAY_KEY: KeyCode = KeyCode::F3;
/// Key binding that shows and hides the debug gizmos
const DEBUG_GIZMOS_KEY: KeyCode = KeyCode::F4;
/// Whether the debug gizmos are on when the game opens
const DEBUG_GIZMOS_AT_STARTUP: bool = cfg!(debug_assertions);

/// Frame rate and frame time, on a key, over whatever else is on screen.
///
/// The fleet-scale corollary is a claim about frame time under thousands of rovers, and a claim
/// nobody can read while playing is one that goes stale between benchmarks. This is the debug view
/// that makes it readable, so it ships with the game rather than behind a build flag.
pub struct DiagnosticsPlugin;

impl Plugin for DiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            DiagnosticsOverlayPlugin,
        ))
        .add_systems(Update, toggle_overlay);
    }
}

fn toggle_overlay(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    overlay_q: Query<Entity, With<DiagnosticsOverlay>>,
) {
    if !input.just_pressed(DIAGNOSTICS_OVERLAY_KEY) {
        return;
    }

    match overlay_q.iter().next() {
        Some(overlay) => {
            commands.entity(overlay).despawn();
        }
        None => {
            commands.spawn(DiagnosticsOverlay::fps());
        }
    }
}

/// The gizmo config group this game's debug views draw into.
///
/// A view that wants a line takes `Gizmos<DebugGizmos>` rather than registering a group and a key
/// of its own, so the whole debug layer answers to `DebugGizmosPlugin`'s switch.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct DebugGizmos;

/// One switch for every debug gizmo this game draws.
///
/// A debug view is how a jam stops looking like a slow rover, and a key each is how a debug layer
/// stops being usable. One key flips the whole `DebugGizmos` group instead. A development build
/// opens with the group on, because watching a system run is what that build is for; a release
/// build opens with it off, because a player is not there to read it.
pub struct DebugGizmosPlugin;

impl Plugin for DebugGizmosPlugin {
    fn build(&self, app: &mut App) {
        app.insert_gizmo_config(
            DebugGizmos,
            GizmoConfig {
                enabled: DEBUG_GIZMOS_AT_STARTUP,
                ..default()
            },
        )
        .add_systems(Update, toggle_debug_gizmos);
    }
}

fn toggle_debug_gizmos(input: Res<ButtonInput<KeyCode>>, mut config: ResMut<GizmoConfigStore>) {
    if !input.just_pressed(DEBUG_GIZMOS_KEY) {
        return;
    }

    let (config, _) = config.config_mut::<DebugGizmos>();
    config.enabled = !config.enabled;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{headless_app, press_key, release_key, tick};

    fn overlay_app() -> App {
        let mut app = headless_app();
        app.add_systems(Update, toggle_overlay);
        app
    }

    fn overlays(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<DiagnosticsOverlay>>()
            .iter(app.world())
            .count()
    }

    #[test]
    fn the_overlay_is_hidden_until_the_key_is_pressed() {
        let mut app = overlay_app();

        tick(&mut app);

        assert_eq!(overlays(&mut app), 0);
    }

    #[test]
    fn pressing_the_key_shows_the_overlay() {
        let mut app = overlay_app();

        press_key(&mut app, DIAGNOSTICS_OVERLAY_KEY);
        tick(&mut app);

        assert_eq!(overlays(&mut app), 1);
    }

    #[test]
    fn pressing_the_key_again_hides_the_overlay() {
        let mut app = overlay_app();
        press_key(&mut app, DIAGNOSTICS_OVERLAY_KEY);
        tick(&mut app);
        release_key(&mut app, DIAGNOSTICS_OVERLAY_KEY);
        tick(&mut app);

        press_key(&mut app, DIAGNOSTICS_OVERLAY_KEY);
        tick(&mut app);

        assert_eq!(overlays(&mut app), 0);
    }

    #[test]
    fn holding_the_key_down_leaves_one_overlay() {
        let mut app = overlay_app();
        press_key(&mut app, DIAGNOSTICS_OVERLAY_KEY);
        tick(&mut app);

        tick(&mut app);
        tick(&mut app);
        tick(&mut app);

        assert_eq!(overlays(&mut app), 1);
    }

    #[derive(Resource, Default)]
    struct Drew(bool);

    fn gizmo_app() -> App {
        let mut app = headless_app();
        app.add_plugins(DebugGizmosPlugin);
        app
    }

    fn gizmos_are_on(app: &App) -> bool {
        app.world()
            .resource::<GizmoConfigStore>()
            .config::<DebugGizmos>()
            .0
            .enabled
    }

    fn set_gizmos(app: &mut App, on: bool) {
        app.world_mut()
            .resource_mut::<GizmoConfigStore>()
            .config_mut::<DebugGizmos>()
            .0
            .enabled = on;
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_development_build_opens_with_the_debug_gizmos_on() {
        let mut app = gizmo_app();

        tick(&mut app);

        assert!(gizmos_are_on(&app));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_opens_with_the_debug_gizmos_off() {
        let mut app = gizmo_app();

        tick(&mut app);

        assert!(!gizmos_are_on(&app));
    }

    #[test]
    fn pressing_the_key_turns_the_debug_gizmos_on() {
        let mut app = gizmo_app();
        tick(&mut app);
        set_gizmos(&mut app, false);

        press_key(&mut app, DEBUG_GIZMOS_KEY);
        tick(&mut app);

        assert!(gizmos_are_on(&app));
    }

    #[test]
    fn pressing_the_key_turns_the_debug_gizmos_off() {
        let mut app = gizmo_app();
        tick(&mut app);
        set_gizmos(&mut app, true);

        press_key(&mut app, DEBUG_GIZMOS_KEY);
        tick(&mut app);

        assert!(!gizmos_are_on(&app));
    }

    #[test]
    fn holding_the_key_down_leaves_the_debug_gizmos_on() {
        let mut app = gizmo_app();
        tick(&mut app);
        set_gizmos(&mut app, false);
        press_key(&mut app, DEBUG_GIZMOS_KEY);
        tick(&mut app);

        tick(&mut app);
        tick(&mut app);
        tick(&mut app);

        assert!(gizmos_are_on(&app));
    }

    #[test]
    fn a_system_can_draw_into_the_debug_gizmo_group() {
        let mut app = gizmo_app();
        app.init_resource::<Drew>();
        app.add_systems(Update, draw_a_line);

        tick(&mut app);

        assert!(app.world().resource::<Drew>().0);
    }

    fn draw_a_line(mut gizmos: Gizmos<DebugGizmos>, mut drew: ResMut<Drew>) {
        gizmos.line(Vec3::ZERO, Vec3::X, Color::WHITE);
        drew.0 = true;
    }
}
