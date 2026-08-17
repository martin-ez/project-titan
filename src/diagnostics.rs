use bevy::dev_tools::diagnostics_overlay::{DiagnosticsOverlay, DiagnosticsOverlayPlugin};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

/// Key binding that shows and hides the diagnostics overlay
const DIAGNOSTICS_OVERLAY_KEY: KeyCode = KeyCode::F3;

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
}
