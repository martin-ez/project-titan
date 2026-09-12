//! What is letting traffic through the junction the player picked out.
//!
//! One row per road that meets there, in the order the signal key walks them, naming the road by
//! the way its arms head away from the crossing — the very arms the debug view draws an arrow
//! down, so a row can be matched to the ground it is about. A signalled junction says how long
//! each road holds the green, which is the reading `#75` left the player without: the cycle could
//! be watched going by but never read, and a green cannot be tuned deliberately by watching it.
//!
//! An unsignalled one says so, and says what its roads settled between themselves instead, rather
//! than showing a timing that is not running.
//!
//! Nothing here writes. The keys that put a signal on and time it are declared by `crate::road`,
//! beside the component they change (invariant 4).

use crate::road::{Junction, JunctionLegs, JunctionPolicy, Signal, Signalled};
use crate::ui::selection::{Picked, Selection};
use crate::ui::{
    panel, panel_row, panel_text, Panel, PanelCorner, BODY_TEXT, HEADING_TEXT, KEYED_TEXT,
};
use bevy::prelude::*;

/// How wide the panel is, in logical pixels, held fixed so no name can resize it
const PANEL_WIDTH: f32 = 240.0;

/// How wide the column naming a road is, in logical pixels
const ROAD_COLUMN_WIDTH: f32 = 100.0;

/// What the panel is headed with
const HEADING: &str = "Junction";

/// What marks the road the signal favours, and what stands in its place on the rest
const FAVOURED: [&str; 2] = ["  ", "▸ "];

/// The points of the compass an arm is named by, from due north and going round clockwise
///
/// Twelve rather than eight: the lattice a road runs on steps at sixty degrees and its tile
/// middles lie ninety apart, so the directions a road actually runs sit thirty apart, and eight
/// points would hand two of them the same name.
const COMPASS: [&str; 12] = [
    "N", "NNE", "NE", "E", "SE", "SSE", "S", "SSW", "SW", "W", "NW", "NNW",
];

/// What stands between the names of the arms a road has at the junction
const ARMS_JOINED: &str = " – ";

/// What the panel says of a junction the player has put no signal on
const NO_SIGNAL: &str = "No signal";

/// The panel reading out the junction the player picked out.
pub struct JunctionPanelPlugin;

/// The panel on screen, of which there is at most one.
#[derive(Component)]
struct JunctionPanel;

/// What the panel reads off a junction: who meets there, and what is letting them through.
type JunctionReading = (
    &'static Junction,
    Option<&'static JunctionLegs>,
    Option<&'static JunctionPolicy>,
    Option<&'static Signal>,
);

/// A junction whose reading has moved, which is what a redraw waits for.
type SignalMoved = Or<(
    Changed<Signal>,
    Changed<JunctionLegs>,
    Changed<JunctionPolicy>,
)>;

/// One line of the panel: the road, how long it holds the green, and whether it is favoured.
struct RoadRow {
    road: String,
    green: String,
    favoured: bool,
}

/// What the panel says: the roads meeting at the junction, and the right of way under no signal.
struct Reading {
    rows: Vec<RoadRow>,
    right_of_way: Option<String>,
}

impl Plugin for JunctionPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, redraw_the_panel.after(Picked).after(Signalled));
    }
}

/// Draw the junction the player picked out, whenever that or what lets traffic through it moves.
///
/// It is ordered after the keys that signal a junction rather than merely after the selection, so
/// a press is read back on the frame it is made instead of the frame after it. Taking a signal
/// off changes no component, so the removal is watched for as well as the change.
fn redraw_the_panel(
    mut commands: Commands,
    selection: Res<Selection>,
    moved: Query<(), SignalMoved>,
    mut unsignalled: RemovedComponents<Signal>,
    panels: Query<Entity, With<JunctionPanel>>,
    junctions: Query<JunctionReading>,
) {
    let picked = selection.junction();
    let taken_off = unsignalled.read().any(|junction| Some(junction) == picked);
    let stirred = picked.is_some_and(|junction| moved.contains(junction));
    if !selection.is_changed() && !stirred && !taken_off {
        return;
    }

    let reading = picked
        .and_then(|junction| junctions.get(junction).ok())
        .map(|(junction, legs, policy, signal)| reading_of(junction, legs, policy, signal));
    let standing = panels.iter().next();

    match (reading, standing) {
        (None, Some(panel)) => commands.entity(panel).despawn(),
        (None, None) => {}
        (Some(reading), Some(panel)) => {
            commands
                .entity(panel)
                .despawn_related::<Children>()
                .with_children(|panel| fill_the_panel(panel, &reading));
        }
        (Some(reading), None) => {
            commands
                .spawn((
                    JunctionPanel,
                    panel(
                        Panel::Junction,
                        PanelCorner::BottomRight,
                        Val::Px(PANEL_WIDTH),
                    ),
                ))
                .with_children(|panel| fill_the_panel(panel, &reading));
        }
    }
}

fn reading_of(
    junction: &Junction,
    legs: Option<&JunctionLegs>,
    policy: Option<&JunctionPolicy>,
    signal: Option<&Signal>,
) -> Reading {
    Reading {
        rows: junction
            .roads()
            .into_iter()
            .filter_map(|road| row_of(road, legs, signal))
            .collect(),
        right_of_way: signal.is_none().then(|| right_of_way(policy, legs)),
    }
}

fn row_of(road: Entity, legs: Option<&JunctionLegs>, signal: Option<&Signal>) -> Option<RoadRow> {
    let name = name_of(road, legs);
    if name.is_empty() {
        return None;
    }
    Some(RoadRow {
        road: name,
        green: signal
            .map(|signal| green(signal.green_for(road)))
            .unwrap_or_default(),
        favoured: signal.is_some_and(|signal| signal.favours() == road),
    })
}

/// What a junction runs on while no signal is on it, which is what its roads settled between them.
fn right_of_way(policy: Option<&JunctionPolicy>, legs: Option<&JunctionLegs>) -> String {
    match policy {
        Some(JunctionPolicy::GiveWayTo(road)) => match name_of(*road, legs) {
            name if name.is_empty() => NO_SIGNAL.to_string(),
            name => format!("{NO_SIGNAL} — the {name} road goes first"),
        },
        Some(JunctionPolicy::TakeTurns) => format!("{NO_SIGNAL} — every road in turn"),
        None => NO_SIGNAL.to_string(),
    }
}

/// What to call `road` here: the points of the compass its arms head off towards, in turn.
fn name_of(road: Entity, legs: Option<&JunctionLegs>) -> String {
    let mut points: Vec<usize> = legs
        .map(|legs| legs.arms_of(road))
        .unwrap_or_default()
        .into_iter()
        .map(point_of)
        .collect();
    points.sort_unstable();
    points
        .into_iter()
        .map(|point| COMPASS[point])
        .collect::<Vec<&str>>()
        .join(ARMS_JOINED)
}

/// Which point of the compass `heading` lies nearest, due north being the way the grid's rows run.
fn point_of(heading: Vec3) -> usize {
    let a_point = 360. / COMPASS.len() as f32;
    let bearing = heading.x.atan2(heading.z).to_degrees().rem_euclid(360.);
    (bearing / a_point).round() as usize % COMPASS.len()
}

fn green(ticks: u32) -> String {
    match ticks {
        1 => "green for 1 tick".to_string(),
        many => format!("green for {many} ticks"),
    }
}

fn fill_the_panel(panel: &mut ChildSpawnerCommands, reading: &Reading) {
    panel.spawn(panel_text(HEADING.to_string(), HEADING_TEXT, Val::Auto));
    for row in &reading.rows {
        panel.spawn(panel_row()).with_children(|line| {
            line.spawn(panel_text(
                format!("{}{}", FAVOURED[usize::from(row.favoured)], row.road),
                if row.favoured { KEYED_TEXT } else { BODY_TEXT },
                Val::Px(ROAD_COLUMN_WIDTH),
            ));
            line.spawn(panel_text(row.green.clone(), BODY_TEXT, Val::Auto));
        });
    }
    if let Some(right_of_way) = &reading.right_of_way {
        panel.spawn(panel_text(right_of_way.clone(), BODY_TEXT, Val::Auto));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, LatticeNode};
    use crate::road::{JunctionSignalPlugin, Road, RoadPlugin};
    use crate::testing::{headless_app, press_key, tick};
    use crate::ui::selection::SelectionPlugin;

    /// The tiles the road running east and west is laid through, in offset-row coordinates.
    const ALONG: [(i32, i32); 4] = [(1, 0), (2, 0), (3, 0), (4, 0)];

    /// The tiles the road crossing it is laid through, in offset-row coordinates.
    ///
    /// Collinear on the lattice, as `ALONG` is, so both roads are laid as straight arcs and each
    /// arm heads exactly along the run rather than along a tangent fitted through a bend.
    const ACROSS: [(i32, i32); 3] = [(1, 1), (2, 0), (2, -1)];

    /// The tiles a road that ends on `ALONG` rather than crossing it is laid through.
    const UP_TO: [(i32, i32); 2] = [(2, 0), (1, 1)];

    /// What the panel calls the road laid through `ALONG`.
    const ALONG_NAMED: &str = "E – W";

    /// What the panel calls the road laid through `ACROSS`.
    const ACROSS_NAMED: &str = "SSE – NNW";

    /// What the panel calls the road laid through `UP_TO`, which has one arm at the junction.
    const UP_TO_NAMED: &str = "NNW";

    /// The key `crate::road` binds to put a signal on the junction picked out, or move it on.
    const SIGNAL_KEY: KeyCode = KeyCode::KeyG;

    /// The key `crate::road` binds to lengthen the green of the junction picked out.
    const LONGER_GREEN: KeyCode = KeyCode::BracketRight;

    fn panel_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::EditBuildings)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                BuildingPlugin,
                CleanupPlugin,
                DebugGizmosPlugin,
                JunctionPanelPlugin,
                JunctionSignalPlugin,
                RoadPlugin,
                SelectionPlugin,
            ));
        app
    }

    fn tile_of(offsets: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offsets.0, offsets.1)
    }

    /// Lay a two-way road through the middle of each of `offsets`.
    fn lay_a_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        let nodes = offsets
            .iter()
            .map(|&offset| LatticeNode::from_tile(tile_of(offset)))
            .collect();
        app.world_mut()
            .spawn(Road {
                nodes,
                leaving: None,
                one_way: false,
            })
            .id()
    }

    fn hold(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
        tick(app);
    }

    /// Click on the ground at `point`, over no tile, and let the button go.
    fn click_at(app: &mut App, point: Vec3) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = None;
            input.world_cursor_position = Some(point);
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    fn the_junction(app: &mut App) -> (Entity, Vec3) {
        let mut found: Vec<(Entity, Vec3)> = app
            .world_mut()
            .query::<(Entity, &Junction)>()
            .iter(app.world())
            .map(|(entity, junction)| (entity, junction.at))
            .collect();
        let one = found.pop();
        assert!(found.is_empty(), "more than one junction");
        one.expect("the roads meet")
    }

    /// Pick the one junction in the world out with the select tool.
    fn pick_the_junction_out(app: &mut App) -> Entity {
        hold(app, PlayerAction::Select);
        let (junction, at) = the_junction(app);
        click_at(app, at);
        junction
    }

    /// Two straight roads crossing, the junction picked out, and the road running east and west.
    fn a_crossroads() -> (App, Entity, Entity) {
        let mut app = panel_app();
        let along = lay_a_road(&mut app, &ALONG);
        lay_a_road(&mut app, &ACROSS);
        tick(&mut app);
        tick(&mut app);
        let junction = pick_the_junction_out(&mut app);
        (app, junction, along)
    }

    /// A road ending on a straight road, the junction picked out, and the road that ends there.
    fn a_road_ending_on_another() -> App {
        let mut app = panel_app();
        lay_a_road(&mut app, &ALONG);
        tick(&mut app);
        lay_a_road(&mut app, &UP_TO);
        tick(&mut app);
        tick(&mut app);
        pick_the_junction_out(&mut app);
        app
    }

    /// Put a signal on `junction` favouring `road`, as the signal key would.
    fn signal(app: &mut App, junction: Entity, road: Entity) {
        app.world_mut()
            .entity_mut(junction)
            .insert(Signal::favouring(road));
        tick(app);
    }

    fn the_signal(app: &App, junction: Entity) -> Signal {
        app.world()
            .get::<Signal>(junction)
            .expect("a signal is on the junction")
            .clone()
    }

    fn panels(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<JunctionPanel>>()
            .iter(app.world())
            .count()
    }

    fn panel_lines(app: &mut App) -> Vec<String> {
        let Some(panel) = app
            .world_mut()
            .query_filtered::<Entity, With<JunctionPanel>>()
            .iter(app.world())
            .next()
        else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        collect_text(app.world(), panel, &mut lines);
        lines
    }

    fn collect_text(world: &World, entity: Entity, lines: &mut Vec<String>) {
        if let Some(text) = world.get::<Text>(entity) {
            lines.push(text.0.clone());
        }
        let Some(children) = world.get::<Children>(entity) else {
            return;
        };
        for child in children.iter() {
            collect_text(world, child, lines);
        }
    }

    fn says(app: &mut App, wanted: &str) -> bool {
        panel_lines(app).iter().any(|line| line.contains(wanted))
    }

    /// Every node the panel is built out of, which is what a redraw replaces and a skip does not.
    fn panel_nodes(app: &mut App) -> Vec<Entity> {
        let Some(panel) = app
            .world_mut()
            .query_filtered::<Entity, With<JunctionPanel>>()
            .iter(app.world())
            .next()
        else {
            return Vec::new();
        };
        let mut nodes = Vec::new();
        collect_nodes(app.world(), panel, &mut nodes);
        nodes
    }

    fn collect_nodes(world: &World, entity: Entity, nodes: &mut Vec<Entity>) {
        nodes.push(entity);
        let Some(children) = world.get::<Children>(entity) else {
            return;
        };
        for child in children.iter() {
            collect_nodes(world, child, nodes);
        }
    }

    #[test]
    fn a_junction_picked_out_names_the_roads_that_meet_there() {
        let (mut app, _, _) = a_crossroads();

        assert!(says(&mut app, ALONG_NAMED), "{:?}", panel_lines(&mut app));
        assert!(says(&mut app, ACROSS_NAMED), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn a_road_ending_at_the_junction_is_named_by_its_one_arm() {
        let mut app = a_road_ending_on_another();

        assert!(says(&mut app, UP_TO_NAMED), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn the_panel_marks_the_road_the_signal_favours() {
        let (mut app, junction, along) = a_crossroads();

        signal(&mut app, junction, along);

        assert!(says(&mut app, &format!("▸ {ALONG_NAMED}")));
        assert!(!says(&mut app, &format!("▸ {ACROSS_NAMED}")));
    }

    #[test]
    fn the_panel_says_how_long_the_favoured_road_holds_the_green() {
        let (mut app, junction, along) = a_crossroads();

        signal(&mut app, junction, along);

        let green = the_signal(&app, junction).green_for(along);
        assert!(says(&mut app, &format!("green for {green} ticks")));
    }

    #[test]
    fn the_panel_says_every_other_road_holds_a_single_tick() {
        let (mut app, junction, along) = a_crossroads();

        signal(&mut app, junction, along);

        assert!(says(&mut app, "green for 1 tick"));
    }

    #[test]
    fn a_junction_with_no_signal_says_so_rather_than_a_timing() {
        let (mut app, _, _) = a_crossroads();

        assert!(says(&mut app, "No signal"));
        assert!(!says(&mut app, "green for"));
    }

    #[test]
    fn an_unsignalled_junction_names_the_road_that_goes_first() {
        let (mut app, junction, along) = a_crossroads();

        app.world_mut()
            .entity_mut(junction)
            .insert(JunctionPolicy::GiveWayTo(along));
        tick(&mut app);

        assert!(says(
            &mut app,
            &format!("the {ALONG_NAMED} road goes first")
        ));
    }

    #[test]
    fn an_unsignalled_junction_that_takes_turns_says_so() {
        let (mut app, junction, _) = a_crossroads();

        app.world_mut()
            .entity_mut(junction)
            .insert(JunctionPolicy::TakeTurns);
        tick(&mut app);

        assert!(says(&mut app, "every road in turn"));
    }

    #[test]
    fn signalling_the_junction_moves_the_reading_on_the_frame_of_the_press() {
        let (mut app, _, _) = a_crossroads();

        press_key(&mut app, SIGNAL_KEY);
        tick(&mut app);

        assert!(says(&mut app, "green for"));
        assert!(!says(&mut app, "No signal"));
    }

    #[test]
    fn timing_the_green_moves_the_reading_on_the_frame_of_the_press() {
        let (mut app, junction, along) = a_crossroads();
        signal(&mut app, junction, along);
        let green = the_signal(&app, junction).green_for(along);

        press_key(&mut app, LONGER_GREEN);
        tick(&mut app);

        assert!(says(&mut app, &format!("green for {} ticks", green + 1)));
    }

    #[test]
    fn taking_the_signal_off_returns_the_panel_to_the_right_of_way() {
        let (mut app, junction, along) = a_crossroads();
        signal(&mut app, junction, along);

        app.world_mut().entity_mut(junction).remove::<Signal>();
        tick(&mut app);

        assert!(says(&mut app, "No signal"));
        assert!(!says(&mut app, "green for"));
    }

    #[test]
    fn no_junction_picked_out_draws_no_panel() {
        let mut app = panel_app();
        lay_a_road(&mut app, &ALONG);
        lay_a_road(&mut app, &ACROSS);
        tick(&mut app);
        tick(&mut app);

        hold(&mut app, PlayerAction::Select);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn a_junction_taken_off_the_map_closes_the_panel() {
        let (mut app, junction, _) = a_crossroads();

        app.world_mut().entity_mut(junction).despawn();
        tick(&mut app);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn picking_up_another_tool_closes_the_panel() {
        let (mut app, _, _) = a_crossroads();

        hold(&mut app, PlayerAction::EditRoads);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn a_frame_that_changes_nothing_leaves_the_panel_alone() {
        let (mut app, _, _) = a_crossroads();
        let before = panel_nodes(&mut app);

        tick(&mut app);

        assert_eq!(panel_nodes(&mut app), before);
    }
}
