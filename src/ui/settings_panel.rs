//! The camera sensitivities the player can change while the game runs, and the panel to change
//! them on.
//!
//! Every other panel here reads the game out. This one edits, which is why it holds the keys that
//! change [`CameraSensitivity`] rather than leaving them to `crate::camera`: what a press does
//! depends on the row the panel has picked out, and that row is the panel's own state. It writes
//! nothing else, and a sensitivity is a setting on the player's input rather than anything a rover
//! could observe (invariant 4).
//!
//! The five sensitivities carry different units and differ by three orders of magnitude, so a
//! press moves one by a tenth of its own shipped default and the row reads out what fraction of
//! that default it now stands at. That is the one reading that means the same on every row.

use crate::camera::CameraSensitivity;
use crate::input::{DeclareCommands, PlayerCommand, Requested};
use crate::ui::legend::{BindingCategory, BindingInput};
use crate::ui::{panel, panel_row, panel_text, PanelCorner, BODY_TEXT, HEADING_TEXT, KEYED_TEXT};
use bevy::prelude::*;
use std::ops::Range;

/// Key binding that shows and hides the settings panel
const SETTINGS_KEY: KeyCode = KeyCode::F2;

/// The keys that walk the picked row through the list, the way each walks it, and what to call that
const PICK_KEYS: [(KeyCode, SettingPick, &str); 2] = [
    (KeyCode::ArrowUp, SettingPick(-1), "Pick the setting above"),
    (KeyCode::ArrowDown, SettingPick(1), "Pick the setting below"),
];

/// The keys that change the picked setting, the way each changes it, and what to call that
const ADJUST_KEYS: [(KeyCode, SettingAdjust, &str); 2] = [
    (
        KeyCode::ArrowLeft,
        SettingAdjust(-1.0),
        "Lower the setting you picked",
    ),
    (
        KeyCode::ArrowRight,
        SettingAdjust(1.0),
        "Raise the setting you picked",
    ),
];

/// What the player asks for to show or hide the settings panel.
#[derive(Clone, Copy, PartialEq)]
struct ShowTheSettings;

/// How far down the list the player asked the picked row to move, negative to move it up.
#[derive(Clone, Copy, PartialEq)]
struct SettingPick(isize);

/// Which way the player asked the picked setting to move, as a multiple of one step.
#[derive(Clone, Copy, PartialEq)]
struct SettingAdjust(f32);

/// How much of its own shipped default one press moves a setting by
const ADJUST_STEP: f32 = 0.1;

/// How far a setting can be taken from its shipped default, as a multiple of it
const SETTING_RANGE: Range<f32> = 0.1..3.0;

/// How wide the panel is, in logical pixels, held fixed so no reading can resize it
const PANEL_WIDTH: f32 = 260.0;

/// How wide the column naming a setting is, in logical pixels
const NAME_COLUMN_WIDTH: f32 = 180.0;

/// What marks the row the player picked out, and what stands in its place otherwise
const PICKED_OUT: [&str; 2] = ["  ", "▸ "];

/// What the panel calls the group of settings it carries
const HEADING: &str = "Camera sensitivity";

/// The settings the panel offers, in the order it lists them
const SETTINGS: [Setting; 5] = [
    Setting::Translation,
    Setting::Orbit,
    Setting::Zoom,
    Setting::ScrollLine,
    Setting::ScrollPixel,
];

/// One field of [`CameraSensitivity`], as the panel offers it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Setting {
    Translation,
    Orbit,
    Zoom,
    ScrollLine,
    ScrollPixel,
}

impl Setting {
    /// What the panel calls this setting.
    fn label(self) -> &'static str {
        match self {
            Setting::Translation => "Move the camera",
            Setting::Orbit => "Orbit the camera",
            Setting::Zoom => "Zoom by dragging",
            Setting::ScrollLine => "Zoom by a wheel notch",
            Setting::ScrollPixel => "Zoom by a touchpad",
        }
    }

    fn read(self, sensitivity: &CameraSensitivity) -> f32 {
        match self {
            Setting::Translation => sensitivity.translation,
            Setting::Orbit => sensitivity.orbit,
            Setting::Zoom => sensitivity.zoom,
            Setting::ScrollLine => sensitivity.scroll_line,
            Setting::ScrollPixel => sensitivity.scroll_pixel,
        }
    }

    fn write(self, sensitivity: &mut CameraSensitivity, value: f32) {
        match self {
            Setting::Translation => sensitivity.translation = value,
            Setting::Orbit => sensitivity.orbit = value,
            Setting::Zoom => sensitivity.zoom = value,
            Setting::ScrollLine => sensitivity.scroll_line = value,
            Setting::ScrollPixel => sensitivity.scroll_pixel = value,
        }
    }

    /// What this setting stood at before the player touched it, which every reading is against.
    fn shipped(self) -> f32 {
        self.read(&CameraSensitivity::default())
    }
}

/// The panel the player changes the camera's sensitivities from.
pub struct SettingsPanelPlugin;

/// The panel on screen, of which there is at most one.
#[derive(Component)]
struct SettingsPanel;

/// Which of [`SETTINGS`] the adjust keys are holding, as a place in that list.
#[derive(Resource, Default)]
struct PickedSetting(usize);

impl Plugin for SettingsPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSensitivity>()
            .init_resource::<PickedSetting>()
            .declare_commands([PlayerCommand {
                input: BindingInput::Key(SETTINGS_KEY),
                asks: ShowTheSettings,
                action: "Show or hide the camera settings",
                category: BindingCategory::Panels,
            }])
            .declare_commands(PICK_KEYS.map(|(key, asks, action)| PlayerCommand {
                input: BindingInput::Key(key),
                asks,
                action,
                category: BindingCategory::Panels,
            }))
            .declare_commands(ADJUST_KEYS.map(|(key, asks, action)| PlayerCommand {
                input: BindingInput::Key(key),
                asks,
                action,
                category: BindingCategory::Panels,
            }))
            .add_systems(
                Update,
                (
                    toggle_the_panel,
                    pick_a_setting,
                    adjust_the_setting,
                    redraw_the_panel,
                )
                    .chain(),
            );
    }
}

fn toggle_the_panel(
    mut commands: Commands,
    asked_for: Res<Requested<ShowTheSettings>>,
    sensitivity: Res<CameraSensitivity>,
    picked: Res<PickedSetting>,
    panels: Query<Entity, With<SettingsPanel>>,
) {
    if !asked_for.asked(ShowTheSettings) {
        return;
    }

    match panels.iter().next() {
        Some(panel) => {
            commands.entity(panel).despawn();
        }
        None => spawn_the_panel(&mut commands, &sensitivity, picked.0),
    }
}

/// Walk the picked row through the list, stopping at either end rather than wrapping round it.
fn pick_a_setting(
    asked_for: Res<Requested<SettingPick>>,
    mut picked: ResMut<PickedSetting>,
    panels: Query<Entity, With<SettingsPanel>>,
) {
    if panels.is_empty() {
        return;
    }

    for SettingPick(step) in asked_for.iter() {
        picked.0 = picked.0.saturating_add_signed(step).min(SETTINGS.len() - 1);
    }
}

fn adjust_the_setting(
    asked_for: Res<Requested<SettingAdjust>>,
    picked: Res<PickedSetting>,
    mut sensitivity: ResMut<CameraSensitivity>,
    panels: Query<Entity, With<SettingsPanel>>,
) {
    if panels.is_empty() {
        return;
    }
    let Some(setting) = SETTINGS.get(picked.0).copied() else {
        return;
    };

    for SettingAdjust(direction) in asked_for.iter() {
        let shipped = setting.shipped();
        let moved = setting.read(&sensitivity) + direction * ADJUST_STEP * shipped;
        setting.write(
            &mut sensitivity,
            moved.clamp(SETTING_RANGE.start * shipped, SETTING_RANGE.end * shipped),
        );
    }
}

/// Rewrite the rows whenever a setting or the picked row has moved.
///
/// The panel keeps its entity and only its rows are built again, so one already on screen stays
/// the one on screen rather than blinking out and back.
fn redraw_the_panel(
    mut commands: Commands,
    sensitivity: Res<CameraSensitivity>,
    picked: Res<PickedSetting>,
    panels: Query<Entity, With<SettingsPanel>>,
) {
    if !sensitivity.is_changed() && !picked.is_changed() {
        return;
    }

    for standing in &panels {
        commands
            .entity(standing)
            .despawn_related::<Children>()
            .with_children(|panel| fill_the_panel(panel, &sensitivity, picked.0));
    }
}

fn spawn_the_panel(commands: &mut Commands, sensitivity: &CameraSensitivity, picked: usize) {
    commands
        .spawn((
            SettingsPanel,
            panel(PanelCorner::TopRight, Val::Px(PANEL_WIDTH)),
        ))
        .with_children(|panel| fill_the_panel(panel, sensitivity, picked));
}

/// Lay the settings out, one row each, naming the one the adjust keys are holding.
fn fill_the_panel(
    panel: &mut ChildSpawnerCommands,
    sensitivity: &CameraSensitivity,
    picked: usize,
) {
    panel.spawn(panel_text(HEADING.to_string(), HEADING_TEXT, Val::Auto));
    for (place, setting) in SETTINGS.into_iter().enumerate() {
        panel.spawn(panel_row()).with_children(|row| {
            row.spawn(panel_text(
                format!(
                    "{}{}",
                    PICKED_OUT[usize::from(place == picked)],
                    setting.label()
                ),
                if place == picked {
                    KEYED_TEXT
                } else {
                    BODY_TEXT
                },
                Val::Px(NAME_COLUMN_WIDTH),
            ));
            row.spawn(panel_text(
                reading_of(setting, sensitivity),
                BODY_TEXT,
                Val::Auto,
            ));
        });
    }
}

/// Where a setting stands as a share of what it shipped at, which reads the same on every row.
fn reading_of(setting: Setting, sensitivity: &CameraSensitivity) -> String {
    format!(
        "{:.0}%",
        setting.read(sensitivity) / setting.shipped() * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::CameraPlugin;
    use crate::input::{CameraMovement, PlayerInput};
    use crate::testing::{ask_for, headless_app, press_key, release_key, tick};

    /// Frames to let the camera's easing settle before a distance is measured off it.
    const FRAMES_TO_SETTLE: u32 = 40;

    fn settings_app() -> App {
        let mut app = headless_app();
        app.add_plugins(SettingsPanelPlugin);
        app
    }

    fn shown_panel_app() -> App {
        let mut app = settings_app();
        tick(&mut app);
        show_the_panel(&mut app);
        app
    }

    fn show_the_panel(app: &mut App) {
        press(app, SETTINGS_KEY);
    }

    fn press(app: &mut App, key: KeyCode) {
        press_key(app, key);
        tick(app);
        release_key(app, key);
        tick(app);
    }

    fn press_many(app: &mut App, key: KeyCode, times: u32) {
        for _ in 0..times {
            press(app, key);
        }
    }

    fn panels(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<SettingsPanel>>()
            .iter(app.world())
            .count()
    }

    fn shown_panel(app: &mut App) -> String {
        let panel = app
            .world_mut()
            .query_filtered::<Entity, With<SettingsPanel>>()
            .iter(app.world())
            .next()
            .expect("a settings panel is on screen");
        let mut lines = Vec::new();
        collect_text(app.world(), panel, &mut lines);
        lines.join("\n")
    }

    fn collect_text(world: &World, entity: Entity, lines: &mut Vec<String>) {
        if let Some(text) = world.get::<Text>(entity) {
            lines.push(text.0.clone());
        }
        if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter() {
                collect_text(world, child, lines);
            }
        }
    }

    fn sensitivity(app: &App) -> CameraSensitivity {
        *app.world().resource::<CameraSensitivity>()
    }

    #[test]
    fn the_settings_panel_is_off_screen_when_the_game_opens() {
        let mut app = settings_app();

        tick(&mut app);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn pressing_the_key_shows_the_settings_panel() {
        let mut app = settings_app();
        tick(&mut app);

        press(&mut app, SETTINGS_KEY);

        assert_eq!(panels(&mut app), 1);
    }

    #[test]
    fn pressing_the_key_again_hides_it() {
        let mut app = shown_panel_app();

        press(&mut app, SETTINGS_KEY);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn the_panel_shows_for_a_command_nobody_pressed_a_key_for() {
        let mut app = settings_app();
        tick(&mut app);

        ask_for(&mut app, ShowTheSettings);
        tick(&mut app);

        assert_eq!(panels(&mut app), 1);
    }

    #[test]
    fn a_setting_is_raised_by_a_command_nobody_pressed_a_key_for() {
        let mut app = shown_panel_app();
        let before = sensitivity(&app).translation;

        ask_for(&mut app, ADJUST_KEYS[1].1);
        tick(&mut app);

        assert!(sensitivity(&app).translation > before);
    }

    #[test]
    fn the_picked_row_moves_for_a_command_nobody_pressed_a_key_for() {
        let mut app = shown_panel_app();

        ask_for(&mut app, PICK_KEYS[1].1);
        tick(&mut app);
        let before = sensitivity(&app);
        press(&mut app, ADJUST_KEYS[1].0);

        assert_eq!(sensitivity(&app).translation, before.translation);
        assert!(sensitivity(&app).orbit > before.orbit);
    }

    #[test]
    fn every_camera_sensitivity_is_named_on_the_panel() {
        let mut app = shown_panel_app();

        let shown = shown_panel(&mut app);

        for setting in SETTINGS {
            assert!(shown.contains(setting.label()), "{shown}");
        }
    }

    #[test]
    fn raising_the_picked_setting_raises_the_camera_sensitivity() {
        let mut app = shown_panel_app();
        let before = sensitivity(&app).translation;

        press(&mut app, ADJUST_KEYS[1].0);

        assert!(sensitivity(&app).translation > before);
    }

    #[test]
    fn lowering_the_picked_setting_lowers_it() {
        let mut app = shown_panel_app();
        let before = sensitivity(&app).translation;

        press(&mut app, ADJUST_KEYS[0].0);

        assert!(sensitivity(&app).translation < before);
    }

    #[test]
    fn the_panel_reads_out_the_value_it_just_changed() {
        let mut app = shown_panel_app();

        press(&mut app, ADJUST_KEYS[1].0);

        let shown = shown_panel(&mut app);
        assert!(shown.contains("110%"), "{shown}");
    }

    #[test]
    fn picking_the_setting_below_changes_that_one_instead() {
        let mut app = shown_panel_app();
        let before = sensitivity(&app);

        press(&mut app, PICK_KEYS[1].0);
        press(&mut app, ADJUST_KEYS[1].0);

        let after = sensitivity(&app);
        assert_eq!(after.translation, before.translation);
        assert!(after.orbit > before.orbit);
    }

    #[test]
    fn a_setting_stops_at_the_top_of_its_range() {
        let mut app = shown_panel_app();

        press_many(&mut app, ADJUST_KEYS[1].0, 30);

        let ceiling = Setting::Translation.shipped() * SETTING_RANGE.end;
        assert!((sensitivity(&app).translation - ceiling).abs() < 1e-4);
    }

    #[test]
    fn a_setting_stops_at_the_bottom_of_its_range() {
        let mut app = shown_panel_app();

        press_many(&mut app, ADJUST_KEYS[0].0, 30);

        let floor = Setting::Translation.shipped() * SETTING_RANGE.start;
        assert!((sensitivity(&app).translation - floor).abs() < 1e-4);
    }

    #[test]
    fn the_adjust_keys_change_nothing_while_the_panel_is_closed() {
        let mut app = settings_app();
        tick(&mut app);
        let before = sensitivity(&app);

        press(&mut app, ADJUST_KEYS[1].0);

        assert_eq!(sensitivity(&app), before);
    }

    #[test]
    fn the_pick_keys_move_nothing_while_the_panel_is_closed() {
        let mut app = settings_app();
        tick(&mut app);
        press(&mut app, PICK_KEYS[1].0);

        show_the_panel(&mut app);
        let before = sensitivity(&app);
        press(&mut app, ADJUST_KEYS[1].0);

        assert!(sensitivity(&app).translation > before.translation);
    }

    #[test]
    fn the_picked_setting_stops_at_the_end_of_the_list() {
        let mut app = shown_panel_app();

        press_many(&mut app, PICK_KEYS[1].0, SETTINGS.len() as u32 + 3);
        let before = sensitivity(&app);
        press(&mut app, ADJUST_KEYS[1].0);

        assert!(sensitivity(&app).scroll_pixel > before.scroll_pixel);
    }

    #[test]
    fn the_picked_setting_stops_at_the_start_of_the_list() {
        let mut app = shown_panel_app();

        press_many(&mut app, PICK_KEYS[0].0, 3);
        let before = sensitivity(&app);
        press(&mut app, ADJUST_KEYS[1].0);

        assert!(sensitivity(&app).translation > before.translation);
    }

    /// An app holding both the panel and the camera it is a settings panel for.
    fn camera_app() -> App {
        let mut app = headless_app();
        app.insert_state(CameraMovement::Translate)
            .insert_resource(PlayerInput::default())
            .add_plugins(CameraPlugin)
            .add_plugins(SettingsPanelPlugin);
        app.world_mut()
            .resource_mut::<PlayerInput>()
            .movement_vector = Vec3::X;
        app
    }

    /// How far the camera carries over `frames`, once its easing has stopped catching up.
    fn camera_moved_over(app: &mut App, frames: u32) -> f32 {
        for _ in 0..FRAMES_TO_SETTLE {
            tick(app);
        }
        let before = camera_position(app);
        for _ in 0..frames {
            tick(app);
        }
        camera_position(app) - before
    }

    fn camera_position(app: &mut App) -> f32 {
        app.world_mut()
            .query::<(&Name, &Transform)>()
            .iter(app.world())
            .find(|(name, _)| name.as_str() == "PanOrbitCamera")
            .expect("the camera plugin spawns a controller on startup")
            .1
            .translation
            .x
    }

    #[test]
    fn raising_the_sensitivity_from_the_panel_moves_the_camera_further() {
        let mut app = camera_app();
        tick(&mut app);
        show_the_panel(&mut app);
        let at_the_default = camera_moved_over(&mut app, 10);

        press_many(&mut app, ADJUST_KEYS[1].0, 5);

        assert!(camera_moved_over(&mut app, 10) > at_the_default * 1.3);
        assert!(at_the_default > 0.0);
    }
}
