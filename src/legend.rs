//! Every command the game answers to, named on screen while it is being play tested.
//!
//! A binding is declared by the plugin that owns it, from the same table that plugin's systems
//! read to decide what a press does, so a key cannot start working without saying what it is for.
//! The legend renders those declarations and nothing else: adding a command adds its row, and
//! there is no second list to forget. This is a debug view under invariant 5, drawn in plain text
//! on the footing the diagnostics overlay already stands on. Choosing the widget layer a player
//! will read is #59's, and nothing here decides it.

use crate::input::PlayerAction;
use bevy::prelude::*;

/// Key binding that shows and hides the legend
const LEGEND_KEY: KeyCode = KeyCode::F1;
/// Whether the legend is on screen when the game opens
const LEGEND_AT_STARTUP: bool = cfg!(debug_assertions);

/// What the player presses to reach a command.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindingInput {
    /// A key on the keyboard.
    Key(KeyCode),
    /// A button on the mouse.
    Mouse(MouseButton),
    /// The scroll wheel, either way.
    Scroll,
}

/// When a binding applies.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindingContext {
    /// Whatever the player is holding.
    Always,
    /// Only while this tool is held.
    Tool(PlayerAction),
}

/// One command the player can reach, and what reaches it.
pub struct Binding {
    /// What the player presses.
    pub input: BindingInput,
    /// What pressing it does, as the legend says it.
    pub action: &'static str,
    /// When pressing it does that.
    pub context: BindingContext,
}

/// Every binding the plugins have declared, in the order they declared them.
///
/// Declaration order is display order, so a plugin's rows stay together and the legend does not
/// reshuffle when an unrelated one is added.
#[derive(Resource, Default)]
pub struct PlayerBindings(Vec<Binding>);

/// Say what a plugin's bindings are, so the legend can name them.
///
/// Call it from `build`, mapping the same table the plugin's systems read. The resource is
/// created by whoever declares first, so this does not depend on the order plugins are added.
pub trait DeclareBindings {
    /// Add these bindings to what the legend draws.
    fn declare_bindings(&mut self, bindings: impl IntoIterator<Item = Binding>) -> &mut Self;
}

impl DeclareBindings for App {
    fn declare_bindings(&mut self, bindings: impl IntoIterator<Item = Binding>) -> &mut Self {
        self.init_resource::<PlayerBindings>();
        self.world_mut()
            .resource_mut::<PlayerBindings>()
            .0
            .extend(bindings);
        self
    }
}

/// The legend of commands, on a key, over whatever else is on screen.
///
/// A tester who has not read the source has no other way to learn what the game answers to, and
/// a briefing goes stale the next time a binding lands. This reads the declarations instead.
pub struct LegendPlugin;

impl Plugin for LegendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerBindings>()
            .declare_bindings([Binding {
                input: BindingInput::Key(LEGEND_KEY),
                action: "Show or hide this legend",
                context: BindingContext::Always,
            }])
            .add_systems(Startup, open_the_legend.run_if(|| LEGEND_AT_STARTUP))
            .add_systems(
                Update,
                (
                    toggle_the_legend,
                    redraw_the_legend.run_if(resource_exists_and_changed::<State<PlayerAction>>),
                ),
            );
    }
}

#[derive(Component)]
struct Legend;

fn open_the_legend(
    mut commands: Commands,
    bindings: Res<PlayerBindings>,
    held: Option<Res<State<PlayerAction>>>,
) {
    spawn_legend(&mut commands, &bindings, held_tool(held.as_deref()));
}

fn toggle_the_legend(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    bindings: Res<PlayerBindings>,
    held: Option<Res<State<PlayerAction>>>,
    legend_q: Query<Entity, With<Legend>>,
) {
    if !input.just_pressed(LEGEND_KEY) {
        return;
    }

    match legend_q.iter().next() {
        Some(legend) => {
            commands.entity(legend).despawn();
        }
        None => spawn_legend(&mut commands, &bindings, held_tool(held.as_deref())),
    }
}

/// Rewrite the legend when the player picks up another tool, which changes what is on offer.
fn redraw_the_legend(
    bindings: Res<PlayerBindings>,
    held: Res<State<PlayerAction>>,
    mut legend_q: Query<&mut Text, With<Legend>>,
) {
    for mut text in &mut legend_q {
        text.0 = legend_text(&bindings, Some(*held.get()));
    }
}

fn held_tool(held: Option<&State<PlayerAction>>) -> Option<PlayerAction> {
    held.map(|state| *state.get())
}

fn spawn_legend(commands: &mut Commands, bindings: &PlayerBindings, held: Option<PlayerAction>) {
    commands.spawn((
        Legend,
        Text(legend_text(bindings, held)),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}

/// Lay the declared bindings out, the ones on offer whatever is held first and a tool's own after.
fn legend_text(bindings: &PlayerBindings, held: Option<PlayerAction>) -> String {
    let mut text = String::new();
    for context in contexts_in_declaration_order(bindings) {
        text.push_str(&heading(context, held));
        text.push('\n');
        for binding in bindings.0.iter().filter(|it| it.context == context) {
            let key = input_label(binding.input);
            text.push_str(&format!("  {key:<12}{}\n", binding.action));
        }
    }
    text
}

fn contexts_in_declaration_order(bindings: &PlayerBindings) -> Vec<BindingContext> {
    let mut seen: Vec<BindingContext> = Vec::new();
    for binding in &bindings.0 {
        if !seen.contains(&binding.context) {
            seen.push(binding.context);
        }
    }
    seen.sort_by_key(|context| matches!(context, BindingContext::Tool(_)));
    seen
}

fn heading(context: BindingContext, held: Option<PlayerAction>) -> String {
    match context {
        BindingContext::Always => "Anytime".to_string(),
        BindingContext::Tool(tool) if held == Some(tool) => format!("{} (held)", tool.label()),
        BindingContext::Tool(tool) => tool.label().to_string(),
    }
}

fn input_label(input: BindingInput) -> String {
    match input {
        BindingInput::Key(key) => key_label(key),
        BindingInput::Mouse(button) => mouse_label(button),
        BindingInput::Scroll => "Scroll".to_string(),
    }
}

fn key_label(key: KeyCode) -> String {
    let named = match key {
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::KeyW => "W",
        KeyCode::KeyA => "A",
        KeyCode::KeyS => "S",
        KeyCode::KeyD => "D",
        KeyCode::ShiftLeft => "Shift",
        KeyCode::ControlLeft => "Ctrl",
        KeyCode::Space => "Space",
        KeyCode::Escape => "Esc",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::F1 => "F1",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        other => return format!("{other:?}"),
    };
    named.to_string()
}

fn mouse_label(button: MouseButton) -> String {
    let named = match button {
        MouseButton::Left => "Click",
        MouseButton::Right => "Right click",
        MouseButton::Middle => "Middle drag",
        other => return format!("{other:?}"),
    };
    named.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{headless_app, press_key, release_key, tick};

    fn legend_app() -> App {
        let mut app = headless_app();
        app.add_plugins(LegendPlugin);
        app
    }

    fn legends(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<Legend>>()
            .iter(app.world())
            .count()
    }

    fn shown_legend(app: &mut App) -> String {
        app.world_mut()
            .query_filtered::<&Text, With<Legend>>()
            .iter(app.world())
            .next()
            .expect("a legend is on screen")
            .0
            .clone()
    }

    fn declare(app: &mut App, bindings: impl IntoIterator<Item = Binding>) {
        app.declare_bindings(bindings);
    }

    fn hide_every_legend(app: &mut App) {
        let showing: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<Legend>>()
            .iter(app.world())
            .collect();
        for legend in showing {
            app.world_mut().entity_mut(legend).despawn();
        }
    }

    fn show_a_legend(app: &mut App) {
        hide_every_legend(app);
        press_key(app, LEGEND_KEY);
        tick(app);
        release_key(app, LEGEND_KEY);
        tick(app);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_development_build_opens_with_the_legend_on() {
        let mut app = legend_app();

        tick(&mut app);

        assert_eq!(legends(&mut app), 1);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_opens_with_the_legend_off() {
        let mut app = legend_app();

        tick(&mut app);

        assert_eq!(legends(&mut app), 0);
    }

    #[test]
    fn pressing_the_key_shows_a_legend_that_is_hidden() {
        let mut app = legend_app();
        tick(&mut app);
        hide_every_legend(&mut app);

        press_key(&mut app, LEGEND_KEY);
        tick(&mut app);

        assert_eq!(legends(&mut app), 1);
    }

    #[test]
    fn pressing_the_key_hides_a_legend_that_is_showing() {
        let mut app = legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        press_key(&mut app, LEGEND_KEY);
        tick(&mut app);

        assert_eq!(legends(&mut app), 0);
    }

    #[test]
    fn holding_the_key_down_leaves_one_legend() {
        let mut app = legend_app();
        tick(&mut app);
        hide_every_legend(&mut app);

        press_key(&mut app, LEGEND_KEY);
        tick(&mut app);
        tick(&mut app);
        tick(&mut app);

        assert_eq!(legends(&mut app), 1);
    }

    #[test]
    fn every_declared_binding_is_named_in_the_legend() {
        let mut app = legend_app();
        declare(
            &mut app,
            [
                Binding {
                    input: BindingInput::Key(KeyCode::KeyQ),
                    action: "Refuel the rover",
                    context: BindingContext::Always,
                },
                Binding {
                    input: BindingInput::Scroll,
                    action: "Zoom the camera",
                    context: BindingContext::Always,
                },
            ],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Refuel the rover"), "{legend}");
        assert!(legend.contains("Zoom the camera"), "{legend}");
    }

    #[test]
    fn a_binding_is_listed_under_the_tool_that_holds_it() {
        let mut app = legend_app();
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Left),
                action: "Place a road node",
                context: BindingContext::Tool(PlayerAction::EditRoads),
            }],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);
        let heading = legend
            .find(PlayerAction::EditRoads.label())
            .expect("the tool has a heading");
        let row = legend
            .find("Place a road node")
            .expect("the binding is named");

        assert!(heading < row, "{legend}");
    }

    #[test]
    fn no_two_bindings_claim_the_same_input_in_the_same_context() {
        let mut app = headless_app();
        app.add_plugins(bevy::diagnostic::DiagnosticsPlugin)
            .add_plugins(LegendPlugin)
            .add_plugins(crate::diagnostics::DebugGizmosPlugin)
            .add_plugins(crate::diagnostics::DiagnosticsPlugin)
            .add_plugins(crate::input::PlayerInputPlugin)
            .add_plugins(crate::simulation::SimulationPlugin)
            .add_plugins(crate::camera::CameraPlugin)
            .add_plugins(crate::road::RoadPlugin)
            .add_plugins(crate::building::BuildingPlugin);

        let bindings = app.world().resource::<PlayerBindings>();
        let mut claimed: Vec<(BindingInput, BindingContext)> = Vec::new();
        for binding in &bindings.0 {
            let claim = (binding.input, binding.context);
            assert!(
                !claimed.contains(&claim),
                "{} is on an input another command already answers to",
                binding.action
            );
            claimed.push(claim);
        }
    }

    #[test]
    fn a_key_is_named_the_way_the_player_would_say_it() {
        let mut app = legend_app();
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Key(KeyCode::ShiftLeft),
                action: "Orbit the camera",
                context: BindingContext::Always,
            }],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Shift"), "{legend}");
        assert!(!legend.contains("ShiftLeft"), "{legend}");
    }
}
