//! Where the player is aiming, and what pressing there asks for.
//!
//! The pointer is over the interface or over the world, never both, and that is the one fact
//! everything else here follows from. Over a panel the window's own cursor comes back and the
//! world cursor points nowhere, so the editing mark goes out and nothing is placed under it; away
//! from every panel the cursor goes off and the mark returns. There is one thing to aim with at
//! any moment, and the swap between them is what says which side of the screen it is aiming at.
//!
//! What is under the pointer is a fact about the frame, set where it is found and let go of at the
//! end of it, like the click it belongs to (invariant 2). Finding it is the only part needing a
//! laid-out screen and the only part not tested: `contains_point` is Bevy's to prove, and a
//! headless `App` has no window, so the finding does nothing there and leaves what a test put in
//! its place. Everything downstream of it is driven from the resource and asserted without one.

use crate::input::{CommandsRead, DeclareCommands, PlayerInput, Requested};
use crate::ui::{Panel, PANEL_RADIUS};
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorOptions, PrimaryWindow};

/// The button a widget answers to, the same one a click on the world is made with
const PRESS_BUTTON: MouseButton = MouseButton::Left;

/// What a widget is drawn on while the player is aiming somewhere else
const WIDGET_BACKGROUND: Color = Color::srgba(0.14, 0.15, 0.20, 0.90);

/// What a widget is drawn on while the pointer is over it
const WIDGET_UNDER_THE_POINTER: Color = Color::srgba(0.24, 0.31, 0.44, 0.95);

/// How much space a widget keeps between its edge and what it is labelled with, in logical pixels
const WIDGET_PADDING: f32 = 3.0;

/// Where the player is aiming, and the presses they make on what the interface has drawn.
pub struct PointerPlugin;

/// A part of a panel the player can press.
#[derive(Component)]
pub struct Widget;

/// What pressing the widget this is on asks for, among the commands of `C`.
#[derive(Component)]
pub struct Asks<C: Send + Sync + 'static>(pub C);

/// What the pointer is over on this frame.
///
/// Nothing outside this module writes one: it is derived from where the pointer is and what the
/// interface has drawn, and a second opinion about where the player is aiming is how a panel and
/// the world come to disagree about who took a click.
#[derive(Resource, Default)]
pub struct Pointer {
    on_the_interface: bool,
    widget: Option<Entity>,
}

impl Pointer {
    /// Aim at `widget`, without the window and the layout that would otherwise put it there.
    #[cfg(test)]
    pub fn point_at(&mut self, widget: Entity) {
        self.on_the_interface = true;
        self.widget = Some(widget);
    }

    /// Aim at a panel, but at no widget on it.
    #[cfg(test)]
    pub fn point_at_the_interface(&mut self) {
        self.on_the_interface = true;
        self.widget = None;
    }
}

/// Say that a plugin's commands can be asked for by pressing a widget as well as by a key.
///
/// Call it from `build`, for the same `C` the commands were declared with. A widget carries the
/// command it asks for rather than the panel knowing what the command does, so a panel can offer
/// a command belonging to the plugin that owns the thing it changes (invariant 4).
pub trait DeclareWidgets {
    /// Read presses on widgets asking for one of `C`'s commands.
    fn declare_widgets<C: Copy + PartialEq + Send + Sync + 'static>(&mut self) -> &mut Self;
}

impl DeclareWidgets for App {
    fn declare_widgets<C: Copy + PartialEq + Send + Sync + 'static>(&mut self) -> &mut Self {
        self.read_requests_of::<C>().add_systems(
            PreUpdate,
            press_the_widget::<C>
                .in_set(CommandsRead)
                .after(PointerFound),
        )
    }
}

/// The frame a widget is drawn in, asking for `command` when the player presses it.
pub fn widget<C: Send + Sync + 'static>(command: C) -> impl Bundle {
    (
        Widget,
        Asks(command),
        Node {
            padding: UiRect::all(Val::Px(WIDGET_PADDING)),
            border_radius: BorderRadius::all(Val::Px(PANEL_RADIUS)),
            ..default()
        },
        BackgroundColor(WIDGET_BACKGROUND),
    )
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PointerFound;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pointer>()
            .add_systems(
                PreUpdate,
                find_what_is_under_the_pointer
                    .after(InputSystems)
                    .before(CommandsRead)
                    .in_set(PointerFound),
            )
            .add_systems(
                PreUpdate,
                let_the_interface_take_the_pointer.after(CommandsRead),
            )
            .add_systems(
                Update,
                (
                    mark_what_is_under_the_pointer,
                    show_the_cursor_over_the_interface,
                ),
            )
            .add_systems(Last, forget_the_pointer);
    }
}

/// Find what the interface has drawn under the pointer, if the pointer is over anything of its.
///
/// It says what it found and stays quiet about what it did not, so what a frame leaves behind is
/// what `forget_the_pointer` takes away rather than what the next frame happened to overwrite.
fn find_what_is_under_the_pointer(
    mut pointer: ResMut<Pointer>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    panels: Query<(&ComputedNode, &UiGlobalTransform), With<Panel>>,
    widgets: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<Widget>>,
) {
    let Some(at) = window.and_then(|window| window.physical_cursor_position()) else {
        return;
    };
    if !panels.iter().any(|(node, sits)| covers(node, sits, at)) {
        return;
    }

    pointer.on_the_interface = true;
    pointer.widget = widgets
        .iter()
        .find(|(_, node, sits)| covers(node, sits, at))
        .map(|(widget, _, _)| widget);
}

/// Whether `node`, drawn where `sits` puts it, has `at` inside it.
///
/// A node laid out at no size is one that is not on screen, and nothing is drawn over it: an empty
/// rectangle otherwise takes every point on its edge, so a panel shut would go on taking presses.
fn covers(node: &ComputedNode, sits: &UiGlobalTransform, at: Vec2) -> bool {
    node.size() != Vec2::ZERO && node.contains_point(*sits, at)
}

fn let_the_interface_take_the_pointer(
    pointer: Res<Pointer>,
    mut player_input: ResMut<PlayerInput>,
) {
    if pointer.on_the_interface {
        player_input.the_interface_took_the_pointer();
    }
}

fn press_the_widget<C: Copy + PartialEq + Send + Sync + 'static>(
    pointer: Res<Pointer>,
    mouse: Res<ButtonInput<MouseButton>>,
    widgets: Query<&Asks<C>>,
    mut requested: ResMut<Requested<C>>,
) {
    if !mouse.just_pressed(PRESS_BUTTON) {
        return;
    }
    let Some(pressed) = pointer.widget else {
        return;
    };
    let Ok(Asks(command)) = widgets.get(pressed) else {
        return;
    };
    requested.ask(*command);
}

fn mark_what_is_under_the_pointer(
    pointer: Res<Pointer>,
    mut widgets: Query<(Entity, &mut BackgroundColor), With<Widget>>,
) {
    for (widget, mut drawn_on) in &mut widgets {
        let wanted = if pointer.widget == Some(widget) {
            WIDGET_UNDER_THE_POINTER
        } else {
            WIDGET_BACKGROUND
        };
        if drawn_on.0 != wanted {
            drawn_on.0 = wanted;
        }
    }
}

fn show_the_cursor_over_the_interface(
    pointer: Res<Pointer>,
    mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    for mut cursor in &mut cursors {
        if cursor.visible != pointer.on_the_interface {
            cursor.visible = pointer.on_the_interface;
        }
    }
}

fn forget_the_pointer(mut pointer: ResMut<Pointer>) {
    *pointer = Pointer::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::PlayerInputPlugin;
    use crate::testing::{headless_app, point_at, point_at_the_interface, press_mouse, tick};

    /// A command a widget in these tests asks for, standing in for one a plugin of the game owns.
    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Prod;

    /// Somewhere in the world the cursor is pointing, for a pointer the interface takes to leave.
    const SOMEWHERE: Vec3 = Vec3::new(1., 0., 2.);

    /// How many times the world was asked to prod, which is what a press on a widget has to reach.
    #[derive(Resource, Default)]
    struct Prods(usize);

    /// How many clicks a system in the world saw, which a press on the interface must not reach.
    #[derive(Resource, Default)]
    struct Taps(usize);

    /// An app reading the player the way the game does, with a widget's command among its own.
    fn pointer_app() -> App {
        let mut app = headless_app();
        app.add_plugins((PlayerInputPlugin, PointerPlugin))
            .declare_widgets::<Prod>()
            .init_resource::<Prods>()
            .init_resource::<Taps>()
            .add_systems(Update, (count_the_prods, count_the_taps));
        app
    }

    /// An app with the pointer but nothing reading the cursor, so what it says stays where a test
    /// put it.
    fn aiming_app() -> App {
        let mut app = headless_app();
        app.insert_resource(PlayerInput::default())
            .add_plugins(PointerPlugin);
        app
    }

    fn count_the_prods(asked_for: Res<Requested<Prod>>, mut prods: ResMut<Prods>) {
        if asked_for.asked(Prod) {
            prods.0 += 1;
        }
    }

    fn count_the_taps(input: Res<PlayerInput>, mut taps: ResMut<Taps>) {
        if input.tapped() {
            taps.0 += 1;
        }
    }

    fn spawn_a_widget(app: &mut App) -> Entity {
        app.world_mut().spawn(widget(Prod)).id()
    }

    fn spawn_a_primary_window(app: &mut App) {
        app.world_mut()
            .spawn((Window::default(), PrimaryWindow, CursorOptions::default()));
    }

    fn prods(app: &App) -> usize {
        app.world().resource::<Prods>().0
    }

    fn taps(app: &App) -> usize {
        app.world().resource::<Taps>().0
    }

    fn cursor_is_visible(app: &mut App) -> bool {
        let mut query = app.world_mut().query::<&CursorOptions>();
        query
            .iter(app.world())
            .next()
            .expect("the window the test spawned is still there")
            .visible
    }

    fn colour_of(app: &App, widget: Entity) -> Color {
        app.world()
            .entity(widget)
            .get::<BackgroundColor>()
            .expect("a widget is drawn on something")
            .0
    }

    #[test]
    fn pressing_a_widget_asks_for_its_command() {
        let mut app = pointer_app();
        let widget = spawn_a_widget(&mut app);
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        point_at(&mut app, widget);
        tick(&mut app);

        assert_eq!(prods(&app), 1);
    }

    #[test]
    fn pressing_a_panel_beside_its_widgets_asks_for_nothing() {
        let mut app = pointer_app();
        spawn_a_widget(&mut app);
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        point_at_the_interface(&mut app);
        tick(&mut app);

        assert_eq!(prods(&app), 0);
    }

    #[test]
    fn a_widget_is_pressed_only_while_it_is_under_the_pointer() {
        let mut app = pointer_app();
        let widget = spawn_a_widget(&mut app);
        tick(&mut app);
        press_mouse(&mut app, PRESS_BUTTON);
        point_at(&mut app, widget);
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        tick(&mut app);

        assert_eq!(prods(&app), 1);
    }

    #[test]
    fn pressing_a_widget_leaves_no_click_for_the_world() {
        let mut app = pointer_app();
        let widget = spawn_a_widget(&mut app);
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        point_at(&mut app, widget);
        tick(&mut app);

        assert_eq!(taps(&app), 0);
    }

    #[test]
    fn pressing_a_panel_leaves_no_click_for_the_world() {
        let mut app = pointer_app();
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        point_at_the_interface(&mut app);
        tick(&mut app);

        assert_eq!(taps(&app), 0);
    }

    #[test]
    fn clicking_away_from_every_panel_reaches_the_world() {
        let mut app = pointer_app();
        tick(&mut app);

        press_mouse(&mut app, PRESS_BUTTON);
        tick(&mut app);

        assert_eq!(taps(&app), 1);
    }

    #[test]
    fn pointing_at_a_panel_points_the_world_cursor_nowhere() {
        let mut app = aiming_app();
        app.world_mut()
            .resource_mut::<PlayerInput>()
            .world_cursor_position = Some(SOMEWHERE);

        point_at_the_interface(&mut app);
        tick(&mut app);

        let input = app.world().resource::<PlayerInput>();
        assert_eq!(input.world_cursor_position, None);
    }

    #[test]
    fn pointing_away_from_every_panel_leaves_the_world_cursor_be() {
        let mut app = aiming_app();
        app.world_mut()
            .resource_mut::<PlayerInput>()
            .world_cursor_position = Some(SOMEWHERE);

        tick(&mut app);

        let input = app.world().resource::<PlayerInput>();
        assert_eq!(input.world_cursor_position, Some(SOMEWHERE));
    }

    #[test]
    fn the_cursor_comes_back_over_a_panel() {
        let mut app = pointer_app();
        spawn_a_primary_window(&mut app);
        tick(&mut app);

        point_at_the_interface(&mut app);
        tick(&mut app);

        assert!(cursor_is_visible(&mut app));
    }

    #[test]
    fn the_cursor_is_hidden_over_the_world() {
        let mut app = pointer_app();
        spawn_a_primary_window(&mut app);

        tick(&mut app);

        assert!(!cursor_is_visible(&mut app));
    }

    #[test]
    fn the_widget_under_the_pointer_is_marked() {
        let mut app = pointer_app();
        let widget = spawn_a_widget(&mut app);
        tick(&mut app);

        point_at(&mut app, widget);
        tick(&mut app);

        assert_eq!(colour_of(&app, widget), WIDGET_UNDER_THE_POINTER);
    }

    #[test]
    fn a_widget_the_pointer_left_is_not_marked() {
        let mut app = pointer_app();
        let widget = spawn_a_widget(&mut app);
        point_at(&mut app, widget);
        tick(&mut app);

        tick(&mut app);

        assert_eq!(colour_of(&app, widget), WIDGET_BACKGROUND);
    }
}
