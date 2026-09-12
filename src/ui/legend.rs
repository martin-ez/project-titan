//! Every command the game answers to, named on screen for the player who is looking for one.
//!
//! A binding is declared by the plugin that owns it, from the same table that plugin's systems
//! read to decide what a press does, so a key cannot start working without saying what it is for.
//! The legend renders those declarations, and only the ones the player can reach where they are
//! standing: each sits under a [`BindingCategory`], and a category the situation does not reach is
//! a heading carrying the count of what is under it rather than its rows. A row naming a key that
//! does nothing there is worse than no panel at all, so a command wanting more than its category
//! says which more in a [`BindingCondition`]. The left button is never a row, being the main
//! action wherever the player stands. It is drawn in `bevy_ui` nodes rather than Bevy's
//! `bevy_feathers` widgets, an editor set and not a game's, for the reasons [`crate::ui`] gives:
//! a row is a pair of text nodes, which is what lets a column line up under a heading.

use crate::building::{BuildingType, ChosenBuildingType, Flow, Port};
use crate::input::{DeclareCommands, PlayerAction, PlayerCommand, Requested};
use crate::ui::selection::Selection;
use crate::ui::{
    panel, panel_font, panel_row, panel_text, Panel, PanelCorner, BODY_TEXT, HEADING_TEXT,
    KEYED_TEXT,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Key binding that shows and hides the legend
const LEGEND_KEY: KeyCode = KeyCode::F1;
/// How much space sits above a heading, holding it off the rows before it, in logical pixels
const HEADING_GAP: f32 = 10.0;
/// How wide the column naming what the player presses is, in logical pixels
const KEY_COLUMN_WIDTH: f32 = 92.0;
/// How wide the panel is, in logical pixels, held fixed so no heading can resize it
const PANEL_WIDTH: f32 = 320.0;

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

/// Where a binding sits on the legend, which is the heading its row is drawn under.
///
/// Where a command sits and when it answers are two questions: the settings panel's adjust keys
/// belong under `Panels` whatever the player is doing, and answer only while that panel is up.
/// This is the first of the two, and [`BindingCondition`] the second.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindingCategory {
    /// Picking up one tool or another.
    Tools,
    /// Carried by one tool, and doing nothing under the others.
    Tool(PlayerAction),
    /// Putting the panels on screen and driving them.
    Panels,
    /// Moving the camera over the surface.
    Camera,
    /// Changing how fast the world runs.
    Simulation,
    /// The debug views drawn over the game.
    Debug,
}

/// What a command needs beyond the category it sits under, before it answers at all.
///
/// A panel claiming to show what is available now is worth less than no panel if a row on it names
/// a key that does nothing where the player is standing. A command that asks for more than its
/// tool says so here, as data the declaring plugin writes down beside the table its own systems
/// read, rather than as a system the legend would have to run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindingCondition {
    /// Wherever the player is standing.
    Always,
    /// Only while this panel is on screen.
    PanelOpen(Panel),
    /// Only while the player has this picked out.
    PickedOut(PickedOut),
}

/// What the player has picked out, for a command that answers only while they have.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickedOut {
    /// A junction of the road.
    Junction,
    /// A port of a building, the one that moves goods this way.
    Port(Flow),
}

/// One command the player can reach, and what reaches it.
pub struct Binding {
    /// What the player presses.
    pub input: BindingInput,
    /// What pressing it does, as the legend says it.
    pub action: &'static str,
    /// Where the legend lists it.
    pub category: BindingCategory,
}

struct Declared {
    binding: Binding,
    condition: BindingCondition,
}

/// The categories the panel lays out, in the order it lays them out
const CATEGORIES: [BindingCategory; 8] = [
    BindingCategory::Tools,
    BindingCategory::Tool(PlayerAction::Select),
    BindingCategory::Tool(PlayerAction::EditRoads),
    BindingCategory::Tool(PlayerAction::EditBuildings),
    BindingCategory::Panels,
    BindingCategory::Camera,
    BindingCategory::Simulation,
    BindingCategory::Debug,
];

impl BindingCategory {
    fn label(self) -> String {
        match self {
            BindingCategory::Tools => "Tools".to_string(),
            BindingCategory::Tool(tool) => tool.label().to_string(),
            BindingCategory::Panels => "Panels".to_string(),
            BindingCategory::Camera => "Camera & movement".to_string(),
            BindingCategory::Simulation => "Simulation".to_string(),
            BindingCategory::Debug => "Debug".to_string(),
        }
    }
}

/// Every binding the plugins have declared, in the order they declared them.
///
/// [`CATEGORIES`] is the order the panel reads in, and declaration order the order within one of
/// them, so a plugin's rows stay together and the legend does not reshuffle when an unrelated
/// plugin is added ahead of it.
#[derive(Resource, Default)]
pub struct PlayerBindings(Vec<Declared>);

/// Say what a plugin's bindings are, so the legend can name them.
///
/// Call it from `build`, mapping the same table the plugin's systems read. The resource is
/// created by whoever declares first, so this does not depend on the order plugins are added.
pub trait DeclareBindings {
    /// Add these bindings, as commands that answer wherever the player is standing.
    fn declare_bindings(&mut self, bindings: impl IntoIterator<Item = Binding>) -> &mut Self {
        self.declare_bindings_when(BindingCondition::Always, bindings)
    }

    /// Add these bindings, as commands that answer only while `condition` holds.
    fn declare_bindings_when(
        &mut self,
        condition: BindingCondition,
        bindings: impl IntoIterator<Item = Binding>,
    ) -> &mut Self;
}

impl DeclareBindings for App {
    fn declare_bindings_when(
        &mut self,
        condition: BindingCondition,
        bindings: impl IntoIterator<Item = Binding>,
    ) -> &mut Self {
        self.init_resource::<PlayerBindings>();
        self.world_mut().resource_mut::<PlayerBindings>().0.extend(
            bindings
                .into_iter()
                .map(|binding| Declared { binding, condition }),
        );
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
            .declare_commands([PlayerCommand {
                input: BindingInput::Key(LEGEND_KEY),
                asks: ShowTheLegend,
                action: "Show or hide this legend",
                category: BindingCategory::Panels,
            }])
            .init_resource::<Situation>()
            .add_systems(Startup, open_the_legend)
            .add_systems(Update, (toggle_the_legend, redraw_the_legend).chain());
    }
}

#[derive(Component)]
struct Legend;

/// What the player asks for to show or hide the legend.
#[derive(Clone, Copy, PartialEq)]
struct ShowTheLegend;

/// Where the player is standing, which is what says which commands answer them there.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
struct Situation {
    held: Option<PlayerAction>,
    placing: Option<BuildingType>,
    picked: Option<PickedOut>,
    panels: u8,
}

/// The world a situation is read out of, none of which a test app has to carry.
#[derive(SystemParam)]
struct Standing<'w, 's> {
    held: Option<Res<'w, State<PlayerAction>>>,
    chosen: Option<Res<'w, ChosenBuildingType>>,
    selection: Option<Res<'w, Selection>>,
    ports: Query<'w, 's, &'static Port>,
    panels: Query<'w, 's, &'static Panel>,
}

impl Standing<'_, '_> {
    fn read(&self) -> Situation {
        let held = self.held.as_deref().map(|state| *state.get());
        Situation {
            held,
            placing: self.placing(held),
            picked: self.picked(),
            panels: self
                .panels
                .iter()
                .fold(0, |on_screen, panel| on_screen | panel_bit(*panel)),
        }
    }

    fn placing(&self, held: Option<PlayerAction>) -> Option<BuildingType> {
        if held != Some(PlayerAction::EditBuildings) {
            return None;
        }
        self.chosen.as_deref().map(|chosen| chosen.chosen())
    }

    fn picked(&self) -> Option<PickedOut> {
        let selection = self.selection.as_deref()?;
        if selection.junction().is_some() {
            return Some(PickedOut::Junction);
        }
        let port = self.ports.get(selection.port()?).ok()?;
        Some(PickedOut::Port(port.flow))
    }
}

fn panel_bit(panel: Panel) -> u8 {
    match panel {
        Panel::Legend => 1,
        Panel::Settings => 2,
        Panel::Building => 4,
        Panel::Junction => 8,
        Panel::ProductionTree => 16,
    }
}

impl Situation {
    fn holds(&self, condition: BindingCondition) -> bool {
        match condition {
            BindingCondition::Always => true,
            BindingCondition::PanelOpen(panel) => self.panels & panel_bit(panel) != 0,
            BindingCondition::PickedOut(picked) => self.picked == Some(picked),
        }
    }

    /// Whether the player's situation reaches `category`, which is what opens it on the panel.
    ///
    /// A tool's own is reached while that tool is held. Any other is reached while something
    /// under it is live for a reason other than always, which a category of unconditional
    /// commands never is: it offers the same thing wherever the player stands, so opening it
    /// every time tells them nothing they could not have read once. `Tools` is the exception,
    /// being how they reach all the rest.
    fn reaches(&self, category: BindingCategory, shown: &[&Declared]) -> bool {
        if shown.is_empty() {
            return false;
        }
        match category {
            BindingCategory::Tools => true,
            BindingCategory::Tool(tool) => self.held == Some(tool),
            _ => shown
                .iter()
                .any(|it| it.condition != BindingCondition::Always),
        }
    }
}

fn open_the_legend(
    mut commands: Commands,
    bindings: Res<PlayerBindings>,
    standing: Standing,
    mut drawn: ResMut<Situation>,
) {
    *drawn = standing.read();
    spawn_legend(&mut commands, &bindings, &drawn);
}

fn toggle_the_legend(
    mut commands: Commands,
    asked_for: Res<Requested<ShowTheLegend>>,
    bindings: Res<PlayerBindings>,
    standing: Standing,
    mut drawn: ResMut<Situation>,
    legend_q: Query<Entity, With<Legend>>,
) {
    if !asked_for.asked(ShowTheLegend) {
        return;
    }

    match legend_q.iter().next() {
        Some(legend) => {
            commands.entity(legend).despawn();
        }
        None => {
            *drawn = standing.read();
            spawn_legend(&mut commands, &bindings, &drawn);
        }
    }
}

/// Rewrite the legend when the player's situation changes under it.
///
/// The panel keeps its entity, and only its rows are built again, so a legend already on screen
/// stays the one on screen rather than blinking out and back. The situation is read every frame
/// and compared with the one drawn, rather than watched for a change: four separate facts make
/// it up, and a panel coming on screen is a change to none of them.
fn redraw_the_legend(
    mut commands: Commands,
    bindings: Res<PlayerBindings>,
    standing: Standing,
    mut drawn: ResMut<Situation>,
    legend_q: Query<Entity, With<Legend>>,
) {
    let now = standing.read();
    if now == *drawn {
        return;
    }

    *drawn = now;
    for legend in &legend_q {
        commands
            .entity(legend)
            .despawn_related::<Children>()
            .with_children(|panel| fill_the_panel(panel, &bindings, &now));
    }
}

fn spawn_legend(commands: &mut Commands, bindings: &PlayerBindings, situation: &Situation) {
    commands
        .spawn((
            Legend,
            panel(Panel::Legend, PanelCorner::TopLeft, Val::Px(PANEL_WIDTH)),
        ))
        .with_children(|panel| fill_the_panel(panel, bindings, situation));
}

/// Lay the categories out, opening the ones the situation reaches and counting what is under the
/// rest, so a command the player cannot reach from here is named but does not take a row.
fn fill_the_panel(
    panel: &mut ChildSpawnerCommands,
    bindings: &PlayerBindings,
    situation: &Situation,
) {
    for (place, category) in categories_declared(bindings).into_iter().enumerate() {
        let under: Vec<&Declared> = bindings
            .0
            .iter()
            .filter(|it| it.binding.category == category)
            .collect();
        let shown: Vec<&Declared> = under
            .iter()
            .copied()
            .filter(|it| situation.holds(it.condition))
            .collect();

        if !situation.reaches(category, &shown) {
            panel.spawn(heading_row(
                heading(category, situation, Some(under.len())),
                place,
            ));
            continue;
        }

        panel.spawn(heading_row(heading(category, situation, None), place));
        for declared in shown {
            panel.spawn(panel_row()).with_children(|row| {
                row.spawn(panel_text(
                    input_label(declared.binding.input),
                    KEYED_TEXT,
                    Val::Px(KEY_COLUMN_WIDTH),
                ));
                row.spawn(panel_text(
                    declared.binding.action.to_string(),
                    BODY_TEXT,
                    Val::Auto,
                ));
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
        panel_font(),
        TextColor(HEADING_TEXT),
    )
}

fn categories_declared(bindings: &PlayerBindings) -> Vec<BindingCategory> {
    CATEGORIES
        .into_iter()
        .filter(|category| bindings.0.iter().any(|it| it.binding.category == *category))
        .collect()
}

/// What the rows beneath it are for, and, while it is `closed`, how many it is holding back.
///
/// A tool that places whatever it last placed, with no way to see which that is, is one the player
/// builds by trial and error, so the heading of the held tool names the choice it carries. It says
/// it is the one in hand whether it is open or closed, this being the only place the panel says
/// which tool the player is holding.
fn heading(category: BindingCategory, situation: &Situation, closed: Option<usize>) -> String {
    let label = category.label();
    let held = matches!(category, BindingCategory::Tool(tool) if situation.held == Some(tool));
    match (held, closed, situation.placing) {
        (false, None, _) => label,
        (false, Some(under), _) => format!("{label} ({under})"),
        (true, Some(under), _) => format!("{label} (held, {under})"),
        (true, None, Some(placing)) => format!("{label} (held) — {}", placing.label()),
        (true, None, None) => format!("{label} (held)"),
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
        KeyCode::KeyQ => "Q",
        KeyCode::KeyE => "E",
        KeyCode::KeyG => "G",
        KeyCode::KeyR => "R",
        KeyCode::ShiftLeft => "Shift",
        KeyCode::ControlLeft => "Ctrl",
        KeyCode::Space => "Space",
        KeyCode::Escape => "Esc",
        KeyCode::ArrowUp => "↑",
        KeyCode::ArrowDown => "↓",
        KeyCode::ArrowLeft => "←",
        KeyCode::ArrowRight => "→",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Minus => "-",
        KeyCode::Equal => "+",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
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
    use crate::building::{Item, Port};
    use crate::input::TURN_KEY;
    use crate::testing::{ask_for, headless_app, press_key, release_key, tick};
    use crate::ui::selection::Selection;

    /// How many lines the legend drew before it was grouped, which the issue asks it to beat by
    /// a factor of three: 36 commands under four headings.
    const LINES_BEFORE_GROUPING: usize = 40;

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

    /// How many lines the panel draws, each of its children being a heading or a row.
    fn legend_line_count(app: &mut App) -> usize {
        let panel = legend_panel(app);
        app.world()
            .get::<Children>(panel)
            .map_or(0, |children| children.len())
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
    fn a_hidden_legend_shows_for_a_command_nobody_pressed_a_key_for() {
        let mut app = legend_app();
        tick(&mut app);
        hide_every_legend(&mut app);

        ask_for(&mut app, ShowTheLegend);
        tick(&mut app);

        assert_eq!(legends(&mut app), 1);
    }

    #[test]
    fn a_declared_command_is_named_in_the_legend() {
        let mut app = legend_app();
        app.declare_commands([PlayerCommand {
            input: BindingInput::Key(KeyCode::KeyQ),
            asks: ShowTheLegend,
            action: "Refuel the rover",
            category: BindingCategory::Tools,
        }]);
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Refuel the rover"), "{legend}");
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
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Scroll,
                    action: "Zoom the camera",
                    category: BindingCategory::Tools,
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
        let mut app = legend_app_holding(PlayerAction::EditRoads);
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Right),
                action: "Place a road node",
                category: BindingCategory::Tool(PlayerAction::EditRoads),
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
    fn the_panel_is_not_sized_by_the_type_the_player_chose() {
        let mut app = building_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        let panel = legend_panel(&mut app);
        let width = app
            .world()
            .entity(panel)
            .get::<Node>()
            .expect("a panel")
            .width;

        assert!(matches!(width, Val::Px(_)), "{width:?}");
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
                category: BindingCategory::Tool(PlayerAction::EditBuildings),
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

    /// Every plugin that declares a command, which is the whole legend as a player meets it.
    fn every_plugin_that_declares_a_binding() -> App {
        let mut app = headless_app();
        app.add_plugins(bevy::diagnostic::DiagnosticsPlugin)
            .add_plugins(LegendPlugin)
            .add_plugins(crate::diagnostics::DebugGizmosPlugin)
            .add_plugins(crate::diagnostics::DiagnosticsPlugin)
            .add_plugins(crate::input::PlayerInputPlugin)
            .add_plugins(crate::simulation::SimulationPlugin)
            .add_plugins(crate::camera::CameraPlugin)
            .add_plugins(crate::road::RoadPlugin)
            .add_plugins(crate::road::JunctionSignalPlugin)
            .add_plugins(crate::ui::selection::SelectionPlugin)
            .add_plugins(crate::fleet::FleetPlugin)
            .add_plugins(crate::building::BuildingPlugin)
            .add_plugins(crate::ui::settings_panel::SettingsPanelPlugin);
        app
    }

    #[test]
    fn no_two_bindings_claim_the_same_input_in_the_same_category() {
        let app = every_plugin_that_declares_a_binding();

        let bindings = app.world().resource::<PlayerBindings>();
        let mut claimed: Vec<(BindingInput, BindingCategory)> = Vec::new();
        for declared in &bindings.0 {
            let binding = &declared.binding;
            let claim = (binding.input, binding.category);
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
            [
                Binding {
                    input: BindingInput::Key(KeyCode::ShiftLeft),
                    action: "Orbit the camera",
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Key(KeyCode::KeyQ),
                    action: "Choose the type before this one",
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Key(KeyCode::KeyE),
                    action: "Choose the type after this one",
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Key(TURN_KEY),
                    action: "Turn the building you are about to place",
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Key(KeyCode::ArrowDown),
                    action: "Pick the setting below",
                    category: BindingCategory::Tools,
                },
            ],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Shift"), "{legend}");
        assert!(legend.contains("↓"), "{legend}");
        assert!(!legend.contains("ShiftLeft"), "{legend}");
        assert!(!legend.contains("KeyQ"), "{legend}");
        assert!(!legend.contains("KeyE"), "{legend}");
        assert!(!legend.contains("KeyR"), "{legend}");
        assert!(!legend.contains("ArrowDown"), "{legend}");
    }

    #[test]
    fn a_row_names_its_key_and_its_action_in_separate_nodes() {
        let mut app = legend_app();
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Left),
                action: "Place a road node",
                category: BindingCategory::Tools,
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
                category: BindingCategory::Tool(PlayerAction::EditRoads),
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

    /// A legend over a game holding `tool`, which is what opens a tool's own category.
    fn legend_app_holding(tool: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(tool).add_plugins(LegendPlugin);
        app
    }

    fn declare_when(
        app: &mut App,
        condition: BindingCondition,
        bindings: impl IntoIterator<Item = Binding>,
    ) {
        app.declare_bindings_when(condition, bindings);
    }

    #[test]
    fn holding_a_tool_opens_its_category() {
        let mut app = legend_app_holding(PlayerAction::EditRoads);
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Mouse(MouseButton::Right),
                action: "Finish the road",
                category: BindingCategory::Tool(PlayerAction::EditRoads),
            }],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(
            lines.iter().any(|line| line == "▸ Road tool (held)"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "Finish the road"),
            "{lines:?}"
        );
    }

    #[test]
    fn the_category_of_a_tool_that_is_not_held_is_a_heading_with_its_count() {
        let mut app = legend_app_holding(PlayerAction::Select);
        declare(
            &mut app,
            [
                Binding {
                    input: BindingInput::Key(KeyCode::KeyQ),
                    action: "Choose the type before this one",
                    category: BindingCategory::Tool(PlayerAction::EditBuildings),
                },
                Binding {
                    input: BindingInput::Key(KeyCode::KeyE),
                    action: "Choose the type after this one",
                    category: BindingCategory::Tool(PlayerAction::EditBuildings),
                },
            ],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(
            lines.iter().any(|line| line == "▸ Building tool (2)"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with("Choose the type")),
            "{lines:?}"
        );
    }

    #[test]
    fn the_tools_category_is_open_whatever_the_player_holds() {
        let mut app = legend_app_holding(PlayerAction::EditRoads);
        declare(
            &mut app,
            [Binding {
                input: BindingInput::Key(KeyCode::Digit3),
                action: "Building tool",
                category: BindingCategory::Tools,
            }],
        );
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(lines.iter().any(|line| line == "▸ Tools"), "{lines:?}");
        assert!(
            lines.iter().any(|line| line == "Building tool"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_category_with_nothing_to_show_is_a_heading_even_while_its_tool_is_held() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(
            lines.iter().any(|line| line == "▸ Select tool (held, 2)"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with("Take a rover")),
            "{lines:?}"
        );
    }

    /// A legend over a game holding the select tool, carrying the two commands a pick unlocks.
    fn picking_legend_app() -> App {
        let mut app = legend_app_holding(PlayerAction::Select);
        app.init_resource::<Selection>();
        declare_when(
            &mut app,
            BindingCondition::PickedOut(PickedOut::Port(Flow::Intake)),
            [Binding {
                input: BindingInput::Key(KeyCode::BracketLeft),
                action: "Take a rover off the port you picked out",
                category: BindingCategory::Tool(PlayerAction::Select),
            }],
        );
        declare_when(
            &mut app,
            BindingCondition::PickedOut(PickedOut::Junction),
            [Binding {
                input: BindingInput::Key(KeyCode::KeyG),
                action: "Signal the junction you picked out",
                category: BindingCategory::Tool(PlayerAction::Select),
            }],
        );
        app
    }

    fn pick_out_a_port(app: &mut App, flow: Flow) {
        let building = app.world_mut().spawn_empty().id();
        let port = app
            .world_mut()
            .spawn(Port {
                flow,
                item: Item::Water,
            })
            .id();
        app.world_mut()
            .insert_resource(Selection::of_a_port(building, port));
    }

    fn pick_out_a_junction(app: &mut App) {
        let junction = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(Selection::of_a_junction(junction));
    }

    #[test]
    fn the_port_keys_are_shown_when_an_intake_is_picked_out() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        pick_out_a_port(&mut app, Flow::Intake);
        tick(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Take a rover off the port"), "{legend}");
    }

    #[test]
    fn the_port_keys_are_hidden_when_an_outlet_is_picked_out() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        pick_out_a_port(&mut app, Flow::Outlet);
        tick(&mut app);

        let legend = shown_legend(&mut app);

        assert!(!legend.contains("Take a rover off the port"), "{legend}");
    }

    #[test]
    fn the_port_keys_are_hidden_when_a_junction_is_picked_out() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        pick_out_a_junction(&mut app);
        tick(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Signal the junction"), "{legend}");
        assert!(!legend.contains("Take a rover off the port"), "{legend}");
    }

    #[test]
    fn the_port_keys_are_hidden_when_nothing_is_picked_out() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(!legend.contains("Take a rover off the port"), "{legend}");
        assert!(!legend.contains("Signal the junction"), "{legend}");
    }

    /// A legend beside the real settings panel, which four of its five keys answer to.
    fn legend_beside_the_settings_panel() -> App {
        let mut app = legend_app_holding(PlayerAction::Select);
        app.add_plugins(crate::ui::settings_panel::SettingsPanelPlugin);
        app
    }

    /// Put the settings panel on screen by pressing the key it is declared on.
    fn open_the_settings_panel(app: &mut App) {
        press_key(app, KeyCode::F2);
        tick(app);
        release_key(app, KeyCode::F2);
        tick(app);
    }

    #[test]
    fn the_settings_keys_are_hidden_while_that_panel_is_closed() {
        let mut app = legend_beside_the_settings_panel();
        tick(&mut app);
        show_a_legend(&mut app);

        let legend = shown_legend(&mut app);

        assert!(!legend.contains("Pick the setting below"), "{legend}");
        assert!(legend.contains("Panels (9)"), "{legend}");
    }

    #[test]
    fn the_settings_keys_are_shown_while_that_panel_is_open() {
        let mut app = legend_beside_the_settings_panel();
        tick(&mut app);
        show_a_legend(&mut app);

        open_the_settings_panel(&mut app);

        let legend = shown_legend(&mut app);

        assert!(legend.contains("Pick the setting below"), "{legend}");
        assert!(legend.contains("Show or hide this legend"), "{legend}");
    }

    #[test]
    fn the_panel_is_under_a_third_of_the_lines_it_drew_before() {
        let mut app = every_plugin_that_declares_a_binding();
        tick(&mut app);
        show_a_legend(&mut app);

        let drawn = legend_line_count(&mut app);

        assert!(
            drawn < LINES_BEFORE_GROUPING / 3,
            "{drawn} lines: {:?}",
            legend_lines(&mut app)
        );
    }

    /// Press `key` and let go again, which is one command asked for.
    fn press_once(app: &mut App, key: KeyCode) {
        press_key(app, key);
        tick(app);
        release_key(app, key);
        tick(app);
    }

    /// A legend over a game holding the select tool, with three categories to walk between.
    ///
    /// `Tools` and the building tool's own come off the bindings declared here, and `Panels` off
    /// the legend's own keys, so the list the picked category walks is `Tools`, `Building tool`,
    /// `Panels`.
    fn legend_over_several_categories() -> App {
        let mut app = legend_app_holding(PlayerAction::Select);
        declare(
            &mut app,
            [
                Binding {
                    input: BindingInput::Key(KeyCode::Digit3),
                    action: "Building tool",
                    category: BindingCategory::Tools,
                },
                Binding {
                    input: BindingInput::Key(KeyCode::KeyQ),
                    action: "Choose the type before this one",
                    category: BindingCategory::Tool(PlayerAction::EditBuildings),
                },
            ],
        );
        app
    }

    /// What `which` panel has written on it, for a test reading one the legend is beside.
    fn panel_lines(app: &mut App, which: Panel) -> Vec<String> {
        let on_screen: Vec<(Entity, Panel)> = app
            .world_mut()
            .query::<(Entity, &Panel)>()
            .iter(app.world())
            .map(|(entity, panel)| (entity, *panel))
            .collect();
        let panel = on_screen
            .into_iter()
            .find(|(_, panel)| *panel == which)
            .map(|(entity, _)| entity)
            .expect("that panel is on screen");
        let mut lines = Vec::new();
        collect_text(app.world(), panel, &mut lines);
        lines
    }

    #[test]
    fn the_legend_marks_the_category_its_keys_are_holding() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(lines.iter().any(|line| line == "▸ Tools"), "{lines:?}");
    }

    #[test]
    fn picking_walks_the_mark_down_the_categories() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        press_once(&mut app, PICK_DOWN_KEY);

        let lines = legend_lines(&mut app);
        assert!(
            lines.iter().any(|line| line == "▸ Building tool (1)"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|line| line == "Tools"), "{lines:?}");
    }

    #[test]
    fn picking_stops_at_the_last_category_rather_than_wrapping() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        for _ in 0..5 {
            press_once(&mut app, PICK_DOWN_KEY);
        }

        let lines = legend_lines(&mut app);
        assert!(
            lines.iter().any(|line| line.starts_with("▸ Panels")),
            "{lines:?}"
        );
    }

    #[test]
    fn picking_stops_at_the_first_category_rather_than_wrapping() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        press_once(&mut app, PICK_UP_KEY);

        let lines = legend_lines(&mut app);
        assert!(lines.iter().any(|line| line == "▸ Tools"), "{lines:?}");
    }

    #[test]
    fn a_category_the_situation_leaves_closed_opens_on_the_key() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        press_once(&mut app, PICK_DOWN_KEY);
        press_once(&mut app, SHOW_CATEGORY_KEY);

        let lines = legend_lines(&mut app);
        assert!(
            lines
                .iter()
                .any(|line| line == "Choose the type before this one"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_category_opened_by_hand_closes_again_on_the_same_key() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);
        press_once(&mut app, PICK_DOWN_KEY);
        press_once(&mut app, SHOW_CATEGORY_KEY);

        press_once(&mut app, SHOW_CATEGORY_KEY);

        let lines = legend_lines(&mut app);
        assert!(
            lines.iter().any(|line| line == "▸ Building tool (1)"),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line == "Choose the type before this one"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_category_opened_by_hand_stays_open_when_another_tool_is_picked_up() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);
        press_once(&mut app, PICK_DOWN_KEY);
        press_once(&mut app, SHOW_CATEGORY_KEY);

        hold_the_tool(&mut app, PlayerAction::EditRoads);

        let lines = legend_lines(&mut app);
        assert!(
            lines
                .iter()
                .any(|line| line == "Choose the type before this one"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_category_closed_by_hand_stays_closed_when_its_tool_is_picked_up() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);
        press_once(&mut app, PICK_DOWN_KEY);
        press_once(&mut app, SHOW_CATEGORY_KEY);
        press_once(&mut app, SHOW_CATEGORY_KEY);

        hold_the_tool(&mut app, PlayerAction::EditBuildings);

        let lines = legend_lines(&mut app);
        assert!(
            !lines
                .iter()
                .any(|line| line == "Choose the type before this one"),
            "{lines:?}"
        );
    }

    /// Put `tool` in the player's hand, which is one of the situations a category reads.
    fn hold_the_tool(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
        tick(app);
        tick(app);
    }

    #[test]
    fn a_category_with_nothing_to_show_does_not_open_by_hand() {
        let mut app = picking_legend_app();
        tick(&mut app);
        show_a_legend(&mut app);

        press_once(&mut app, SHOW_CATEGORY_KEY);

        let lines = legend_lines(&mut app);
        assert!(
            lines.iter().any(|line| line == "▸ Select tool (held, 2)"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with("Take a rover")),
            "{lines:?}"
        );
    }

    #[test]
    fn only_the_settings_panel_answers_an_arrow_while_both_are_open() {
        let mut app = every_plugin_that_declares_a_binding();
        tick(&mut app);
        show_a_legend(&mut app);
        open_the_settings_panel(&mut app);

        press_once(&mut app, KeyCode::ArrowDown);

        let settings = panel_lines(&mut app, Panel::Settings);
        assert!(
            settings.iter().any(|line| line == "▸ Orbit the camera"),
            "{settings:?}"
        );
        let legend = legend_lines(&mut app);
        assert!(legend.iter().any(|line| line == "▸ Tools"), "{legend:?}");
    }

    #[test]
    fn the_legend_says_what_opens_a_category() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        let lines = legend_lines(&mut app);

        assert!(lines.iter().any(|line| line == HOW_TO_OPEN), "{lines:?}");
    }

    #[test]
    fn the_keys_that_open_a_category_are_rows_like_any_other_binding() {
        let mut app = legend_over_several_categories();
        tick(&mut app);
        show_a_legend(&mut app);

        for _ in 0..2 {
            press_once(&mut app, PICK_DOWN_KEY);
        }
        press_once(&mut app, SHOW_CATEGORY_KEY);

        let lines = legend_lines(&mut app);
        assert!(
            lines.iter().any(|line| line == "Open or close the category"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|line| line == "Page Down"), "{lines:?}");
    }

    #[test]
    fn the_main_action_is_never_a_row() {
        let app = every_plugin_that_declares_a_binding();

        for declared in &app.world().resource::<PlayerBindings>().0 {
            assert!(
                declared.binding.input != BindingInput::Mouse(MouseButton::Left),
                "{} is on the left button, which is the main action wherever the player is",
                declared.binding.action
            );
        }
    }
}
