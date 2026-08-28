//! Every command the game answers to, named on screen for the player who is looking for one.
//!
//! A binding is declared by the plugin that owns it, from the same table that plugin's systems
//! read to decide what a press does, so a key cannot start working without saying what it is for.
//! The legend renders those declarations and nothing else: adding a command adds its row, and
//! there is no second list to forget. It is drawn in `bevy_ui` nodes rather than Bevy's
//! `bevy_feathers` widgets, an editor set and not a game's, for the reasons [`crate::ui`] gives.
//! A row is then a pair of text nodes under a panel rather than a line of a formatted string,
//! which is what lets a column line up and a heading read differently from the rows beneath it.

use crate::building::ChosenBuildingType;
use crate::input::PlayerAction;
use bevy::prelude::*;

/// Key binding that shows and hides the legend
const LEGEND_KEY: KeyCode = KeyCode::F1;
/// How far the panel sits from the corner of the screen, in logical pixels
const PANEL_INSET: f32 = 8.0;
/// How much space the panel keeps between its edge and its rows, in logical pixels
const PANEL_PADDING: f32 = 10.0;
/// How round the panel's corners are, in logical pixels
const PANEL_RADIUS: f32 = 4.0;
/// What the panel is drawn on, dark enough to read text over whatever the world puts behind it
const PANEL_BACKGROUND: Color = Color::srgba(0.04, 0.04, 0.06, 0.85);
/// How much space sits between one row and the next, in logical pixels
const ROW_GAP: f32 = 2.0;
/// How much space sits above a heading, holding it off the rows before it, in logical pixels
const HEADING_GAP: f32 = 10.0;
/// How wide the column naming what the player presses is, in logical pixels
const KEY_COLUMN_WIDTH: f32 = 92.0;
/// How large the legend's text is, in logical pixels
const TEXT_SIZE: f32 = 12.0;
/// The colour a heading is written in
const HEADING_COLOR: Color = Color::srgb(0.62, 0.78, 0.98);
/// The colour a key is written in
const KEY_COLOR: Color = Color::srgb(0.98, 0.90, 0.66);
/// The colour an action is written in
const ACTION_COLOR: Color = Color::srgb(0.86, 0.87, 0.90);

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
            .add_systems(Startup, open_the_legend)
            .add_systems(
                Update,
                (
                    toggle_the_legend,
                    redraw_the_legend.run_if(
                        resource_exists_and_changed::<State<PlayerAction>>
                            .or_else(resource_exists_and_changed::<ChosenBuildingType>),
                    ),
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
    chosen: Option<Res<ChosenBuildingType>>,
) {
    let held = held_tool(held.as_deref());
    spawn_legend(
        &mut commands,
        &bindings,
        held,
        placing(chosen.as_deref(), held),
    );
}

fn toggle_the_legend(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    bindings: Res<PlayerBindings>,
    held: Option<Res<State<PlayerAction>>>,
    chosen: Option<Res<ChosenBuildingType>>,
    legend_q: Query<Entity, With<Legend>>,
) {
    if !input.just_pressed(LEGEND_KEY) {
        return;
    }

    let held = held_tool(held.as_deref());
    match legend_q.iter().next() {
        Some(legend) => {
            commands.entity(legend).despawn();
        }
        None => spawn_legend(
            &mut commands,
            &bindings,
            held,
            placing(chosen.as_deref(), held),
        ),
    }
}

/// Rewrite the legend when the player picks up another tool or chooses another thing to build.
///
/// The panel keeps its entity, and only its rows are built again, so a legend already on screen
/// stays the one on screen rather than blinking out and back.
fn redraw_the_legend(
    mut commands: Commands,
    bindings: Res<PlayerBindings>,
    held: Option<Res<State<PlayerAction>>>,
    chosen: Option<Res<ChosenBuildingType>>,
    legend_q: Query<Entity, With<Legend>>,
) {
    let held = held_tool(held.as_deref());
    let placing = placing(chosen.as_deref(), held);
    for legend in &legend_q {
        commands
            .entity(legend)
            .despawn_related::<Children>()
            .with_children(|panel| fill_the_panel(panel, &bindings, held, placing.as_deref()));
    }
}

fn held_tool(held: Option<&State<PlayerAction>>) -> Option<PlayerAction> {
    held.map(|state| *state.get())
}

fn placing(chosen: Option<&ChosenBuildingType>, held: Option<PlayerAction>) -> Option<String> {
    if held != Some(PlayerAction::EditBuildings) {
        return None;
    }
    chosen.map(|chosen| chosen.chosen().label())
}

fn spawn_legend(
    commands: &mut Commands,
    bindings: &PlayerBindings,
    held: Option<PlayerAction>,
    placing: Option<String>,
) {
    commands
        .spawn((Legend, panel()))
        .with_children(|panel| fill_the_panel(panel, bindings, held, placing.as_deref()));
}

fn panel() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(PANEL_INSET),
            left: Val::Px(PANEL_INSET),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(PANEL_PADDING)),
            row_gap: Val::Px(ROW_GAP),
            border_radius: BorderRadius::all(Val::Px(PANEL_RADIUS)),
            ..default()
        },
        BackgroundColor(PANEL_BACKGROUND),
    )
}

/// Lay the declared bindings out, the ones on offer whatever is held first and a tool's own after.
fn fill_the_panel(
    panel: &mut ChildSpawnerCommands,
    bindings: &PlayerBindings,
    held: Option<PlayerAction>,
    placing: Option<&str>,
) {
    for (place, context) in contexts_in_declaration_order(bindings)
        .into_iter()
        .enumerate()
    {
        panel.spawn(heading_row(heading(context, held, placing), place));
        for binding in bindings.0.iter().filter(|it| it.context == context) {
            panel.spawn(row()).with_children(|row| {
                row.spawn(cell(
                    input_label(binding.input),
                    KEY_COLOR,
                    Val::Px(KEY_COLUMN_WIDTH),
                ));
                row.spawn(cell(binding.action.to_string(), ACTION_COLOR, Val::Auto));
            });
        }
    }
}

fn heading_row(heading: String, place: usize) -> impl Bundle {
    (
        Node {
            margin: UiRect::top(Val::Px(if place == 0 { 0.0 } else { HEADING_GAP })),
            ..default()
        },
        Text(heading),
        legend_font(),
        TextColor(HEADING_COLOR),
    )
}

fn row() -> impl Bundle {
    Node {
        flex_direction: FlexDirection::Row,
        ..default()
    }
}

fn cell(text: String, color: Color, width: Val) -> impl Bundle {
    (
        Node { width, ..default() },
        Text(text),
        legend_font(),
        TextColor(color),
    )
}

fn legend_font() -> TextFont {
    TextFont {
        font_size: FontSize::Px(TEXT_SIZE),
        ..default()
    }
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

/// What the rows beneath it are for, and what the held tool is set to place.
///
/// A tool that places whatever it last placed, with no way to see which that is, is one the
/// player builds by trial and error, so the heading of the held tool names the choice it carries.
fn heading(context: BindingContext, held: Option<PlayerAction>, placing: Option<&str>) -> String {
    match (context, placing) {
        (BindingContext::Always, _) => "Anytime".to_string(),
        (BindingContext::Tool(tool), Some(placing)) if held == Some(tool) => {
            format!("{} (held) — {placing}", tool.label())
        }
        (BindingContext::Tool(tool), _) if held == Some(tool) => format!("{} (held)", tool.label()),
        (BindingContext::Tool(tool), _) => tool.label().to_string(),
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

    fn legend_panel(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<Legend>>()
            .iter(app.world())
            .next()
            .expect("a legend is on screen")
    }

    fn legend_lines(app: &mut App) -> Vec<String> {
        let panel = legend_panel(app);
        let mut lines = Vec::new();
        collect_text(app.world(), panel, &mut lines);
        lines
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

    fn shown_legend(app: &mut App) -> String {
        legend_lines(app).join("\n")
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

    #[test]
    fn the_legend_is_on_screen_when_the_game_opens() {
        let mut app = legend_app();

        tick(&mut app);

        assert_eq!(legends(&mut app), 1);
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

    /// A legend over a game holding the building tool, which is what names a type to place.
    fn building_legend_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::EditBuildings)
            .init_resource::<ChosenBuildingType>()
            .add_plugins(LegendPlugin);
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Left),
                action: "Put a building on the tile",
                context: BindingContext::Tool(PlayerAction::EditBuildings),
            }],
        );
        app
    }

    #[test]
    fn the_legend_names_the_type_the_building_tool_will_place() {
        let mut app = building_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        let placing = app
            .world()
            .resource::<ChosenBuildingType>()
            .chosen()
            .label();
        assert!(legend.contains(&placing), "{legend}");
    }

    #[test]
    fn the_legend_follows_the_player_stepping_to_another_type() {
        let mut app = building_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);
        let before = app
            .world()
            .resource::<ChosenBuildingType>()
            .chosen()
            .label();

        app.world_mut().resource_mut::<ChosenBuildingType>().step(1);
        tick(&mut app);

        let after = app
            .world()
            .resource::<ChosenBuildingType>()
            .chosen()
            .label();
        let legend = shown_legend(&mut app);
        assert!(legend.contains(&after), "{legend}");
        assert!(!legend.contains(&before), "{legend}");
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

    #[test]
    fn a_row_names_its_key_and_its_action_in_separate_nodes() {
        let mut app = legend_app();
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Left),
                action: "Place a road node",
                context: BindingContext::Always,
            }],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(lines.iter().any(|line| line == "Click"), "{lines:?}");
        assert!(
            lines.iter().any(|line| line == "Place a road node"),
            "{lines:?}"
        );
    }

    #[test]
    fn picking_up_a_tool_redraws_the_same_panel() {
        let mut app = legend_app();
        app.insert_state(PlayerAction::Select);
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
        let panel = legend_panel(&mut app);

        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::EditRoads);
        tick(&mut app);

        assert_eq!(legend_panel(&mut app), panel);
        let legend = shown_legend(&mut app);
        assert!(legend.contains("(held)"), "{legend}");
    }
}
