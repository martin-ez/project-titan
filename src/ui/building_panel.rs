//! What the building the player picked out is doing, and what its rovers were told to do.
//!
//! One row per port, naming what passes through it and — for an intake — how many rovers serve it
//! and which port they collect from. Every one of those is read off the world each time it moves:
//! the count on screen is [`Fleet::rovers`] itself rather than a copy kept in step, so the panel
//! cannot disagree with the game it is a reading of. A panel that can is worse than none, because
//! it is believed.
//!
//! The last row is the pool every fleet on the map is drawn from, so what a rover costs is read
//! where it is spent, and a request the pool could not fill says so there.
//!
//! Nothing here writes. The keys that change an assignment are declared by `crate::fleet`, beside
//! the component they change (invariant 4).

use crate::building::{BuildingType, Flow, Port};
use crate::fleet::{Fleet, Refused, RoverPool};
use crate::production::{Running, Stopped, WaitingOn};
use crate::ui::selection::{Picked, Selection};
use crate::ui::{
    panel, panel_row, panel_text, Panel, PanelCorner, BODY_TEXT, HEADING_TEXT, KEYED_TEXT,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// How wide the panel is, in logical pixels, held fixed so no name can resize it
const PANEL_WIDTH: f32 = 300.0;

/// How wide the column naming a port is, in logical pixels
const PORT_COLUMN_WIDTH: f32 = 120.0;

/// What marks the row of the port the player picked out, and what stands in its place otherwise
const PICKED_OUT: [&str; 2] = ["  ", "▸ "];

/// What an intake nobody has given a rover says instead of a count
const UNASSIGNED: &str = "no rovers of its own";

/// What a fleet with nothing on the network making what its port takes says instead of a source
const UNSUPPLIED: &str = "nothing that makes it";

/// What the panel says in place of the pool when the port picked out was refused a rover
const REFUSED_A_ROVER: &str = "no rovers spare — take one off elsewhere";

/// The panel reading out the building the player picked out.
pub struct BuildingPanelPlugin;

/// The panel on screen, of which there is at most one.
#[derive(Component)]
struct BuildingPanel;

/// A building that has just started a run or just stopped, which is what the reading is of.
type ProductionMoved = Or<(Changed<Running>, Changed<Stopped>)>;

/// One line of the panel: what the port is, what its fleet was told, and whether it is picked out.
struct PortRow {
    port: String,
    fleet: String,
    picked_out: bool,
}

/// What the panel says: the building picked out, what it is making of its recipe, what each of its
/// ports is doing, and what is left of the rovers every fleet on the map draws from.
struct Reading {
    heading: String,
    doing: &'static str,
    rows: Vec<PortRow>,
    pool: String,
    refused: bool,
}

/// Everything the panel reads to say what a building is doing.
#[derive(SystemParam)]
struct WhatToSay<'w, 's> {
    buildings: Query<
        'w,
        's,
        (
            &'static BuildingType,
            &'static Children,
            Option<&'static Running>,
            Option<&'static Stopped>,
        ),
    >,
    kinds: Query<'w, 's, &'static BuildingType>,
    ports: Query<'w, 's, (&'static Port, Option<&'static Fleet>)>,
    homes: Query<'w, 's, &'static ChildOf>,
    fleets: Query<'w, 's, &'static Fleet>,
    pool: Res<'w, RoverPool>,
    refused: Res<'w, Refused>,
}

impl Plugin for BuildingPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, redraw_the_panel.after(Picked));
    }
}

/// Draw the panel of whatever the player picked out, whenever that, a fleet or the pool has moved.
///
/// A fleet raised from anywhere at all redraws it, which is what makes the count on the panel the
/// count the simulation is running rather than the last one the panel was told about. A refusal
/// moves no fleet and so would otherwise leave the screen saying what it said before the press.
/// The panel keeps its entity and only its rows are built again, so one already on screen stays
/// the one on screen rather than blinking out and back.
fn redraw_the_panel(
    mut commands: Commands,
    selection: Res<Selection>,
    moved: Query<(), Changed<Fleet>>,
    stirred: Query<(), ProductionMoved>,
    panels: Query<Entity, With<BuildingPanel>>,
    saying: WhatToSay,
) {
    let picked = selection.building();
    let started_or_stopped = picked.is_some_and(|building| stirred.contains(building));
    if !selection.is_changed()
        && moved.is_empty()
        && !saying.refused.is_changed()
        && !started_or_stopped
    {
        return;
    }
    let reading = picked.and_then(|building| saying.reading_of(building, selection.port()));
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
                    BuildingPanel,
                    panel(
                        Panel::Building,
                        PanelCorner::BottomRight,
                        Val::Px(PANEL_WIDTH),
                    ),
                ))
                .with_children(|panel| fill_the_panel(panel, &reading));
        }
    }
}

impl WhatToSay<'_, '_> {
    /// What the panel says about `building`, given which of its ports the player picked out.
    fn reading_of(&self, building: Entity, picked_out: Option<Entity>) -> Option<Reading> {
        let (kind, ports, running, stopped) = self.buildings.get(building).ok()?;
        let rows = ports
            .iter()
            .filter_map(|port| self.row_of(port, picked_out == Some(port)))
            .collect();
        Some(Reading {
            heading: kind.label(),
            doing: doing(running, stopped),
            rows,
            pool: self.pool_reading(),
            refused: picked_out.is_some() && self.refused.port() == picked_out,
        })
    }

    /// What is left of the pool, which is its size less what every fleet on the map holds.
    fn pool_reading(&self) -> String {
        let assigned: u32 = self.fleets.iter().map(|fleet| fleet.rovers).sum();
        let spare = self.pool.spare(assigned);
        format!("{spare} of {} rovers spare", self.pool.size)
    }

    fn row_of(&self, port: Entity, picked_out: bool) -> Option<PortRow> {
        let (door, fleet) = self.ports.get(port).ok()?;
        Some(PortRow {
            port: format!("{} {}", flow_label(door.flow), door.item.name()),
            fleet: self.assignment_of(door, fleet),
            picked_out,
        })
    }

    fn assignment_of(&self, door: &Port, fleet: Option<&Fleet>) -> String {
        if door.flow == Flow::Outlet {
            return String::new();
        }
        let Some(fleet) = fleet else {
            return UNASSIGNED.to_string();
        };
        let collecting = match fleet.source {
            Some(source) => self.name_of(source),
            None => UNSUPPLIED.to_string(),
        };
        format!("{} ← {}", rovers(fleet.rovers), collecting)
    }

    /// What to call the port a fleet collects from: the building standing it, and what it hands
    /// over. A source is found rather than pointed at, so what the player is shown is a place on
    /// the map they can go and look at rather than the entity behind it.
    fn name_of(&self, port: Entity) -> String {
        let Ok((door, _)) = self.ports.get(port) else {
            return "somewhere that is gone".to_string();
        };
        let standing = self
            .homes
            .get(port)
            .ok()
            .and_then(|home| self.kinds.get(home.parent()).ok());
        match standing {
            Some(kind) => format!("{} · {}", kind.label(), door.item.name()),
            None => door.item.name().to_string(),
        }
    }
}

/// What the panel says a building is making of its recipe, which is what a stalled chain is read
/// off. A building the tick has not looked at yet carries neither mark, and has yet to try.
fn doing(running: Option<&Running>, stopped: Option<&Stopped>) -> &'static str {
    match (running, stopped) {
        (Some(_), _) => "Running",
        (None, Some(Stopped(WaitingOn::AnInput))) => "Waiting on an input",
        (None, Some(Stopped(WaitingOn::OutletRoom))) => "Waiting on room to put its output",
        (None, None) => "Yet to run",
    }
}

fn fill_the_panel(panel: &mut ChildSpawnerCommands, reading: &Reading) {
    panel.spawn(panel_text(reading.heading.clone(), HEADING_TEXT, Val::Auto));
    panel.spawn(panel_text(reading.doing.to_string(), BODY_TEXT, Val::Auto));
    for row in &reading.rows {
        panel.spawn(panel_row()).with_children(|line| {
            line.spawn(panel_text(
                format!("{}{}", PICKED_OUT[usize::from(row.picked_out)], row.port),
                if row.picked_out {
                    KEYED_TEXT
                } else {
                    BODY_TEXT
                },
                Val::Px(PORT_COLUMN_WIDTH),
            ));
            line.spawn(panel_text(row.fleet.clone(), BODY_TEXT, Val::Auto));
        });
    }
    let (pool, colour) = match reading.refused {
        true => (REFUSED_A_ROVER.to_string(), KEYED_TEXT),
        false => (reading.pool.clone(), BODY_TEXT),
    };
    panel.spawn(panel_text(pool, colour, Val::Auto));
}

fn flow_label(flow: Flow) -> &'static str {
    match flow {
        Flow::Intake => "In",
        Flow::Outlet => "Out",
    }
}

fn rovers(count: u32) -> String {
    match count {
        1 => "1 rover".to_string(),
        many => format!("{many} rovers"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{BuildingPlugin, BuildingTiles, ChosenBuildingType};
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::fleet::FleetPlugin;
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{Deposit, HexCoordinates, MapTile, TileCorner};
    use crate::road::{Road, RoadEndpoint, RoadPlugin};
    use crate::testing::{headless_app, press_key, release_key, tick};
    use crate::ui::selection::SelectionPlugin;

    /// The tile the building the panel reads stands on, in offset-row coordinates.
    const READING: (i32, i32) = (0, 0);

    /// The tile the building it collects from stands on, in offset-row coordinates.
    ///
    /// Placed so its outlet stands on the very corner the melter's intake does, which is the
    /// chaining the port layout is built for: one road node then serves both, and the supplier
    /// the tick finds is the one the panel is asked to name.
    const SUPPLYING: (i32, i32) = (-1, -1);

    /// The tiles the road serving both buildings runs over, in offset-row coordinates.
    const REACHING: [(i32, i32); 2] = [(0, 0), (1, 0)];

    /// A tile nothing stands on, in offset-row coordinates.
    const BARE: (i32, i32) = (6, 0);

    /// How far through the catalogue the first assembler sits: one item in, one out.
    const MELTER: isize = 5;

    /// The corner a melter's intake stands on, which is `INTAKE_CORNERS[0]` unturned.
    const INTAKE: TileCorner = TileCorner::SouthWest;

    /// How many rovers the player has in these tests, few enough to read the arithmetic off.
    const A_POOL: u32 = 4;

    /// A pool one rover spends outright, so the next request for one is refused.
    const A_SPENT_POOL: u32 = 1;

    /// How many rovers a fleet under test is given.
    const A_ROVER: u32 = 1;

    /// The key that asks the port picked out for another rover, which `crate::fleet` binds.
    const ANOTHER_ROVER: KeyCode = KeyCode::Equal;

    fn panel_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::EditBuildings)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                BuildingPlugin,
                BuildingPanelPlugin,
                CleanupPlugin,
                DebugGizmosPlugin,
                FleetPlugin,
                RoadPlugin,
                SelectionPlugin,
            ));
        app
    }

    /// Give the map a pool of `size` rovers, in place of the one the game ships with.
    fn pool_of(app: &mut App, size: u32) {
        app.insert_resource(RoverPool { size });
    }

    fn tap_key(app: &mut App, key: KeyCode) {
        press_key(app, key);
        tick(app);
        release_key(app, key);
        tick(app);
    }

    fn tile_of(offsets: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offsets.0, offsets.1)
    }

    fn spawn_tile(app: &mut App, offsets: (i32, i32)) -> Entity {
        app.world_mut()
            .spawn(MapTile {
                coordinates: tile_of(offsets),
            })
            .id()
    }

    /// Lay under `tile` the ground the type the tool is holding needs, an extractor standing
    /// nowhere but a deposit of what it draws.
    fn ground_for_the_chosen_type(app: &mut App, tile: Entity) {
        let BuildingType::Extractor(material) =
            app.world().resource::<ChosenBuildingType>().chosen()
        else {
            return;
        };
        app.world_mut().entity_mut(tile).insert(Deposit {
            material,
            richness: 1,
        });
    }

    /// Put the `steps`th type of the catalogue on `offsets`, answering with it and its tile.
    fn place(app: &mut App, offsets: (i32, i32), steps: isize) -> (Entity, Entity) {
        app.world_mut()
            .resource_mut::<ChosenBuildingType>()
            .step(steps);
        let tile = spawn_tile(app, offsets);
        ground_for_the_chosen_type(app, tile);
        click_at(app, tile, tile_of(offsets).world_position());
        app.world_mut()
            .resource_mut::<ChosenBuildingType>()
            .step(-steps);
        let building = app
            .world()
            .resource::<BuildingTiles>()
            .building_on(tile_of(offsets))
            .expect("a building stands there");
        (building, tile)
    }

    fn click_at(app: &mut App, tile: Entity, point: Vec3) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = Some(tile);
            input.world_cursor_position = Some(point);
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    fn hold(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
        tick(app);
    }

    /// Pick out the building standing on `offsets`, by clicking the middle of its tile.
    fn pick_out_the_building(app: &mut App, tile: Entity, offsets: (i32, i32)) {
        click_at(app, tile, tile_of(offsets).world_position());
    }

    /// Pick out the port of the building on `offsets` standing on `corner`.
    fn pick_out_the_port(app: &mut App, tile: Entity, offsets: (i32, i32), corner: TileCorner) {
        click_at(app, tile, corner.node_of(tile_of(offsets)).world_position());
    }

    fn port_at(app: &App, building: Entity, corner: TileCorner, tile: (i32, i32)) -> Entity {
        let node = corner.node_of(tile_of(tile));
        app.world()
            .get::<Children>(building)
            .expect("a building has its ports")
            .iter()
            .find(|port| {
                app.world()
                    .get::<RoadEndpoint>(*port)
                    .is_some_and(|endpoint| endpoint.standing_on() == node)
            })
            .expect("a port stands on that corner")
    }

    fn panels(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<BuildingPanel>>()
            .iter(app.world())
            .count()
    }

    fn the_panel(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<BuildingPanel>>()
            .iter(app.world())
            .next()
            .expect("a panel is on screen")
    }

    /// Every node the panel is built out of, which is what a redraw replaces and a skip does not.
    fn panel_nodes(app: &mut App) -> Vec<Entity> {
        let panel = the_panel(app);
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

    fn panel_lines(app: &mut App) -> Vec<String> {
        let Some(panel) = app
            .world_mut()
            .query_filtered::<Entity, With<BuildingPanel>>()
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

    /// An app holding a melter to read, an extractor supplying it, and the select tool.
    fn read_a_melter() -> (App, Entity, Entity) {
        let mut app = panel_app();
        let (melter, tile) = place(&mut app, READING, MELTER);
        place(&mut app, SUPPLYING, 0);
        lay_road_through(&mut app, INTAKE, &REACHING);
        hold(&mut app, PlayerAction::Select);
        (app, melter, tile)
    }

    /// An app holding a melter nothing on the map supplies, and the select tool.
    fn read_a_melter_nothing_supplies() -> (App, Entity, Entity) {
        let mut app = panel_app();
        let (melter, tile) = place(&mut app, READING, MELTER);
        lay_road_through(&mut app, INTAKE, &REACHING);
        hold(&mut app, PlayerAction::Select);
        (app, melter, tile)
    }

    /// Lay a road through the `corner` of each of `offsets`, and let it take its tiles.
    fn lay_road_through(app: &mut App, corner: TileCorner, offsets: &[(i32, i32)]) {
        let nodes = offsets
            .iter()
            .map(|&offset| corner.node_of(tile_of(offset)))
            .collect();
        app.world_mut().spawn(Road {
            nodes,
            leaving: None,
            one_way: false,
        });
        tick(app);
    }

    /// Give the melter's intake a fleet of `rovers`, and let the tick find it the extractor.
    fn assign(app: &mut App, melter: Entity, rovers: u32) -> Entity {
        let intake = port_at(app, melter, INTAKE, READING);
        app.world_mut().entity_mut(intake).insert(Fleet {
            rovers,
            source: None,
        });
        tick(app);
        intake
    }

    #[test]
    fn no_panel_is_drawn_while_nothing_is_picked_out() {
        let (mut app, _, _) = read_a_melter();

        tick(&mut app);

        assert_eq!(panels(&mut app), 0);
    }

    #[test]
    fn the_panel_names_every_port_of_the_building_picked_out() {
        let (mut app, _, tile) = read_a_melter();

        pick_out_the_building(&mut app, tile, READING);

        assert!(says(&mut app, "In Ice"), "{:?}", panel_lines(&mut app));
        assert!(says(&mut app, "Out Water"), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn an_intake_nobody_has_given_a_rover_says_so() {
        let (mut app, _, tile) = read_a_melter();

        pick_out_the_building(&mut app, tile, READING);

        assert!(says(&mut app, UNASSIGNED), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn a_fleet_with_nothing_making_what_its_port_takes_says_so() {
        let (mut app, melter, tile) = read_a_melter_nothing_supplies();
        assign(&mut app, melter, 2);

        pick_out_the_port(&mut app, tile, READING, INTAKE);

        assert!(says(&mut app, "2 rovers"), "{:?}", panel_lines(&mut app));
        assert!(says(&mut app, UNSUPPLIED), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn the_panel_names_the_port_a_fleet_collects_from() {
        let (mut app, melter, tile) = read_a_melter();
        assign(&mut app, melter, 2);

        pick_out_the_port(&mut app, tile, READING, INTAKE);

        assert!(says(&mut app, "2 rovers"), "{:?}", panel_lines(&mut app));
        assert!(
            says(&mut app, "Ice Extractor · Ice"),
            "{:?}",
            panel_lines(&mut app)
        );
    }

    #[test]
    fn the_panel_shows_the_count_the_simulation_is_running() {
        let (mut app, melter, tile) = read_a_melter();
        let intake = assign(&mut app, melter, 1);
        pick_out_the_port(&mut app, tile, READING, INTAKE);

        app.world_mut()
            .entity_mut(intake)
            .get_mut::<Fleet>()
            .expect("the intake has a fleet")
            .rovers = 7;
        tick(&mut app);

        assert!(says(&mut app, "7 rovers"), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn the_panel_marks_the_port_the_player_picked_out() {
        let (mut app, _, tile) = read_a_melter();

        pick_out_the_port(&mut app, tile, READING, INTAKE);

        assert!(says(&mut app, "▸ In Ice"), "{:?}", panel_lines(&mut app));
        assert!(!says(&mut app, "▸ Out"), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn a_frame_that_moves_nothing_leaves_the_panel_alone() {
        let (mut app, _, tile) = read_a_melter();
        pick_out_the_building(&mut app, tile, READING);
        let drawn = panel_nodes(&mut app);

        tick(&mut app);

        assert_eq!(panel_nodes(&mut app), drawn);
    }

    #[test]
    fn a_count_that_moves_redraws_the_same_panel() {
        let (mut app, melter, tile) = read_a_melter();
        let intake = assign(&mut app, melter, 1);
        pick_out_the_port(&mut app, tile, READING, INTAKE);
        let standing = the_panel(&mut app);

        app.world_mut()
            .entity_mut(intake)
            .get_mut::<Fleet>()
            .expect("the intake has a fleet")
            .rovers = 3;
        tick(&mut app);

        assert_eq!(the_panel(&mut app), standing);
        assert!(says(&mut app, "3 rovers"), "{:?}", panel_lines(&mut app));
    }

    #[test]
    fn the_panel_says_how_many_rovers_are_left_to_give() {
        let (mut app, melter, tile) = read_a_melter();
        pool_of(&mut app, A_POOL);
        assign(&mut app, melter, A_ROVER);

        pick_out_the_building(&mut app, tile, READING);

        assert!(
            says(&mut app, "3 of 4 rovers spare"),
            "{:?}",
            panel_lines(&mut app)
        );
    }

    #[test]
    fn the_panel_says_a_port_was_refused_a_rover() {
        let (mut app, melter, tile) = read_a_melter();
        pool_of(&mut app, A_SPENT_POOL);
        assign(&mut app, melter, A_SPENT_POOL);
        pick_out_the_port(&mut app, tile, READING, INTAKE);

        tap_key(&mut app, ANOTHER_ROVER);

        assert!(
            says(&mut app, "no rovers spare"),
            "{:?}",
            panel_lines(&mut app)
        );
    }

    #[test]
    fn the_panel_forgets_a_refusal_once_the_player_has_picked_something_else_out() {
        let (mut app, melter, tile) = read_a_melter();
        pool_of(&mut app, A_SPENT_POOL);
        assign(&mut app, melter, A_SPENT_POOL);
        pick_out_the_port(&mut app, tile, READING, INTAKE);
        tap_key(&mut app, ANOTHER_ROVER);

        pick_out_the_building(&mut app, tile, READING);
        pick_out_the_port(&mut app, tile, READING, INTAKE);

        assert!(
            !says(&mut app, "no rovers spare"),
            "{:?}",
            panel_lines(&mut app)
        );
        assert!(
            says(&mut app, "0 of 1 rovers spare"),
            "{:?}",
            panel_lines(&mut app)
        );
    }

    #[test]
    fn the_panel_goes_when_the_player_picks_nothing_out() {
        let (mut app, _, tile) = read_a_melter();
        pick_out_the_building(&mut app, tile, READING);
        let bare = spawn_tile(&mut app, BARE);

        click_at(&mut app, bare, tile_of(BARE).world_position());

        assert_eq!(panels(&mut app), 0);
    }
}
