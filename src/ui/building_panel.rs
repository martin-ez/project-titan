//! What the building the player picked out is doing, and what its rovers were told to do.
//!
//! One row per port, naming what passes through it and — for an intake — how many rovers serve it
//! and which port they collect from. Every one of those is read off the world each time it moves:
//! the count on screen is [`Fleet::rovers`] itself rather than a copy kept in step, so the panel
//! cannot disagree with the game it is a reading of. A panel that can is worse than none, because
//! it is believed.
//!
//! Nothing here writes. The keys that change an assignment are declared by `crate::fleet`, beside
//! the component they change (invariant 4).

use crate::building::{BuildingType, Flow, Port};
use crate::fleet::Fleet;
use crate::ui::selection::{Picked, Selection};
use crate::ui::{panel, panel_row, panel_text, PanelCorner, BODY_TEXT, HEADING_TEXT, KEYED_TEXT};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// How wide the panel is, in logical pixels, held fixed so no name can resize it
const PANEL_WIDTH: f32 = 300.0;

/// How wide the column naming a port is, in logical pixels
const PORT_COLUMN_WIDTH: f32 = 120.0;

/// What marks the row of the port the player picked out, and what stands in its place otherwise
const PICKED_OUT: [&str; 2] = ["  ", "▸ "];

/// What an intake with nowhere to collect from says instead of a count
const UNPOINTED: &str = "right-click a port to collect from";

/// The panel reading out the building the player picked out.
pub struct BuildingPanelPlugin;

/// The panel on screen, of which there is at most one.
#[derive(Component)]
struct BuildingPanel;

/// One line of the panel: what the port is, what its fleet was told, and whether it is picked out.
struct PortRow {
    port: String,
    fleet: String,
    picked_out: bool,
}

/// Everything the panel reads to say what a building is doing.
#[derive(SystemParam)]
struct WhatToSay<'w, 's> {
    buildings: Query<'w, 's, (&'static BuildingType, &'static Children)>,
    kinds: Query<'w, 's, &'static BuildingType>,
    ports: Query<'w, 's, (&'static Port, Option<&'static Fleet>)>,
    homes: Query<'w, 's, &'static ChildOf>,
}

impl Plugin for BuildingPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, redraw_the_panel.after(Picked));
    }
}

/// Draw the panel of whatever the player picked out, whenever that or a fleet has moved.
///
/// A fleet raised from anywhere at all redraws it, which is what makes the count on the panel the
/// count the simulation is running rather than the last one the panel was told about. The panel
/// keeps its entity and only its rows are built again, so one already on screen stays the one on
/// screen rather than blinking out and back.
fn redraw_the_panel(
    mut commands: Commands,
    selection: Res<Selection>,
    moved: Query<(), Changed<Fleet>>,
    panels: Query<Entity, With<BuildingPanel>>,
    saying: WhatToSay,
) {
    if !selection.is_changed() && moved.is_empty() {
        return;
    }
    let reading = selection
        .building()
        .and_then(|building| saying.reading_of(building, selection.port()));
    let standing = panels.iter().next();

    match (reading, standing) {
        (None, Some(panel)) => commands.entity(panel).despawn(),
        (None, None) => {}
        (Some((heading, rows)), Some(panel)) => {
            commands
                .entity(panel)
                .despawn_related::<Children>()
                .with_children(|panel| fill_the_panel(panel, &heading, &rows));
        }
        (Some((heading, rows)), None) => {
            commands
                .spawn((
                    BuildingPanel,
                    panel(PanelCorner::BottomRight, Val::Px(PANEL_WIDTH)),
                ))
                .with_children(|panel| fill_the_panel(panel, &heading, &rows));
        }
    }
}

impl WhatToSay<'_, '_> {
    /// What the panel says about `building`, given which of its ports the player picked out.
    fn reading_of(
        &self,
        building: Entity,
        picked_out: Option<Entity>,
    ) -> Option<(String, Vec<PortRow>)> {
        let (kind, ports) = self.buildings.get(building).ok()?;
        let rows = ports
            .iter()
            .filter_map(|port| self.row_of(port, picked_out == Some(port)))
            .collect();
        Some((kind.label(), rows))
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
        match fleet {
            Some(fleet) => format!("{} ← {}", rovers(fleet.rovers), self.name_of(fleet.source)),
            None => UNPOINTED.to_string(),
        }
    }

    /// What to call the port a fleet collects from: the building standing it, and what it hands
    /// over. A player names a source by pointing at it, so what they are shown back is where they
    /// pointed rather than the entity they never saw.
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

fn fill_the_panel(panel: &mut ChildSpawnerCommands, heading: &str, rows: &[PortRow]) {
    panel.spawn(panel_text(heading.to_string(), HEADING_TEXT, Val::Auto));
    for row in rows {
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
    use crate::input::{PlayerAction, PlayerInput};
    use crate::map::{HexCoordinates, MapTile, TileCorner};
    use crate::road::{RoadEndpoint, RoadPlugin};
    use crate::testing::{headless_app, tick};
    use crate::ui::selection::SelectionPlugin;

    /// The tile the building the panel reads stands on, in offset-row coordinates.
    const READING: (i32, i32) = (0, 0);

    /// The tile the building it collects from stands on, in offset-row coordinates.
    const SUPPLYING: (i32, i32) = (3, 0);

    /// A tile nothing stands on, in offset-row coordinates.
    const BARE: (i32, i32) = (6, 0);

    /// How far through the catalogue the first assembler sits: one item in, one out.
    const MELTER: isize = 5;

    /// The corner a melter's intake stands on, which is `INTAKE_CORNERS[0]` unturned.
    const INTAKE: TileCorner = TileCorner::SouthWest;

    /// The corner an extractor's outlet stands on, which is `OUTLET_CORNERS[0]` unturned.
    const OUTLET: TileCorner = TileCorner::North;

    fn panel_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::EditBuildings)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                BuildingPlugin,
                BuildingPanelPlugin,
                CleanupPlugin,
                DebugGizmosPlugin,
                RoadPlugin,
                SelectionPlugin,
            ));
        app
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

    /// Put the `steps`th type of the catalogue on `offsets`, answering with it and its tile.
    fn place(app: &mut App, offsets: (i32, i32), steps: isize) -> (Entity, Entity) {
        app.world_mut()
            .resource_mut::<ChosenBuildingType>()
            .step(steps);
        let tile = spawn_tile(app, offsets);
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

    /// An app holding a melter to read, an extractor to collect from, and the select tool.
    fn read_a_melter() -> (App, Entity, Entity) {
        let mut app = panel_app();
        let (melter, tile) = place(&mut app, READING, MELTER);
        place(&mut app, SUPPLYING, 0);
        hold(&mut app, PlayerAction::Select);
        (app, melter, tile)
    }

    /// Give the melter's intake a fleet of `rovers` collecting from the extractor's outlet.
    fn assign(app: &mut App, melter: Entity, rovers: u32) -> Entity {
        let intake = port_at(app, melter, INTAKE, READING);
        let extractor = app
            .world()
            .resource::<BuildingTiles>()
            .building_on(tile_of(SUPPLYING))
            .expect("an extractor stands there");
        let source = port_at(app, extractor, OUTLET, SUPPLYING);
        app.world_mut()
            .entity_mut(intake)
            .insert(Fleet { rovers, source });
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
    fn an_intake_with_nowhere_to_collect_from_says_so() {
        let (mut app, _, tile) = read_a_melter();

        pick_out_the_building(&mut app, tile, READING);

        assert!(says(&mut app, UNPOINTED), "{:?}", panel_lines(&mut app));
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
    fn the_panel_goes_when_the_player_picks_nothing_out() {
        let (mut app, _, tile) = read_a_melter();
        pick_out_the_building(&mut app, tile, READING);
        let bare = spawn_tile(&mut app, BARE);

        click_at(&mut app, bare, tile_of(BARE).world_position());

        assert_eq!(panels(&mut app), 0);
    }
}
