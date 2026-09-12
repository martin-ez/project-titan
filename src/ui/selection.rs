//! What the player has picked out of the map to read and to change.
//!
//! The select tool settles on the building under the cursor and, when the cursor is nearer one of
//! that building's port corners than its middle, on the port standing there. Both are answered by
//! the grid rather than by measuring what is near a click: the node meant is
//! [`LatticeNode::nearest_on`], and a port stands on one node exactly (invariant 3). A corner is
//! shared by three tiles, so the answer is held to the building on the tile under the cursor,
//! which is what keeps one node from naming two ports.
//!
//! A junction is the one thing here the grid cannot answer, standing at a distance along an arc
//! rather than on a node of the lattice. It is picked out by the ground it takes — [`Junction::reaches`],
//! which is the circle the debug view draws — and ahead of any building, being the smaller target.
//!
//! Nothing here writes the game. It says what the player is pointing at; the tools that change
//! what they picked out live with the components they change.

use crate::building::{BuildingTiles, BuildingType, Port};
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{HexCoordinates, LatticeNode, MapTile};
use crate::road::{Junction, RoadEndpoint};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// What the player has picked out with the select tool.
pub struct SelectionPlugin;

/// The point in a frame by which the click has been read and the selection says what it named.
///
/// A tool acting on what was picked out, or a panel drawing it, orders itself after this rather
/// than after any one system inside it — otherwise it draws or acts on the pick before last.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Picked;

/// What the player picked out: a building and a port of it, or a junction of the road.
///
/// A port is picked out only alongside the building it belongs to, so a selection naming a port
/// names the building too. A junction is picked out instead of either, being a thing of the road
/// rather than of the map. All of them are entities the world may take away underneath it, which
/// is why nothing outside this module builds one.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    building: Option<Entity>,
    port: Option<Entity>,
    junction: Option<Entity>,
}

/// What the cursor is over, for a tool that acts on what the player points at.
#[derive(SystemParam)]
pub struct PointedAt<'w, 's> {
    input: Res<'w, PlayerInput>,
    buildings: Res<'w, BuildingTiles>,
    tiles: Query<'w, 's, &'static MapTile>,
    children: Query<'w, 's, &'static Children>,
    ports: Query<'w, 's, &'static RoadEndpoint, With<Port>>,
    junctions: Query<'w, 's, (Entity, &'static Junction)>,
}

impl Selection {
    /// The building picked out, or nothing while none is.
    pub fn building(&self) -> Option<Entity> {
        self.building
    }

    /// The port picked out, or nothing while the player picked the building rather than a door.
    pub fn port(&self) -> Option<Entity> {
        self.port
    }

    /// The junction picked out, or nothing while the player picked something off the road.
    pub fn junction(&self) -> Option<Entity> {
        self.junction
    }
}

#[cfg(test)]
impl Selection {
    /// A selection standing on `port` and the `building` it is a door of.
    pub fn of_a_port(building: Entity, port: Entity) -> Self {
        Self {
            building: Some(building),
            port: Some(port),
            junction: None,
        }
    }

    /// A selection standing on `junction`.
    pub fn of_a_junction(junction: Entity) -> Self {
        Self {
            building: None,
            port: None,
            junction: Some(junction),
        }
    }
}

impl PointedAt<'_, '_> {
    /// What the cursor is over, which is nothing at all when it is over neither road nor building.
    ///
    /// A junction answers ahead of a building because it is the smaller target of the two and
    /// stands on ground a building may also be standing on, so a road crossing a built-up tile
    /// stays something the player can point at.
    pub fn under_the_cursor(&self) -> Selection {
        if let Some(junction) = self.junction_under_the_cursor() {
            return Selection {
                junction: Some(junction),
                ..Selection::default()
            };
        }
        let Some((tile, building)) = self.building_under_the_cursor() else {
            return Selection::default();
        };
        Selection {
            building: Some(building),
            port: self.port_of(building, tile),
            junction: None,
        }
    }

    fn junction_under_the_cursor(&self) -> Option<Entity> {
        let at = self.input.world_cursor_position?;
        self.junctions
            .iter()
            .filter(|(_, junction)| junction.reaches(at))
            .min_by(|(_, one), (_, other)| one.at.distance(at).total_cmp(&other.at.distance(at)))
            .map(|(entity, _)| entity)
    }

    fn building_under_the_cursor(&self) -> Option<(HexCoordinates, Entity)> {
        let tile = self.tiles.get(self.input.cursor_tile?).ok()?.coordinates;
        let building = self.buildings.building_on(tile)?;
        Some((tile, building))
    }

    fn port_of(&self, building: Entity, tile: HexCoordinates) -> Option<Entity> {
        let node = LatticeNode::nearest_on(tile, self.input.world_cursor_position?);
        self.children
            .get(building)
            .ok()?
            .iter()
            .find(|port| self.stands_on(*port, node))
    }

    fn stands_on(&self, port: Entity, node: LatticeNode) -> bool {
        self.ports
            .get(port)
            .is_ok_and(|endpoint| endpoint.standing_on() == node)
    }
}

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Selection>()
            .add_systems(
                Update,
                (
                    settle_on_what_the_player_clicked,
                    forget_a_selection_that_left_the_world,
                )
                    .chain()
                    .in_set(Picked),
            )
            .add_systems(
                OnExit(PlayerAction::Select),
                put_the_selection_down_with_the_tool,
            );
    }
}

/// Settle the selection on whatever the player clicked, holding the select tool.
///
/// A click on ground nothing stands on picks nothing out, which is what closes the panel: a tool
/// that only ever adds to what is picked out gives the player no way to be looking at nothing.
fn settle_on_what_the_player_clicked(
    input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    pointed: PointedAt,
    mut selection: ResMut<Selection>,
) {
    if !input.tapped() || *action.get() != PlayerAction::Select {
        return;
    }
    selection.set_if_neq(pointed.under_the_cursor());
}

/// Let go of a building or a port the world no longer holds.
///
/// A bulldozed building leaves an entity nothing can be read from, and a panel drawn from one
/// shows the player a machine that is not there. Only what is picked out is looked at, so this
/// reads two entities however many are on the map.
fn forget_a_selection_that_left_the_world(
    mut selection: ResMut<Selection>,
    buildings: Query<&BuildingType>,
    ports: Query<&Port>,
    junctions: Query<&Junction>,
) {
    if selection
        .junction
        .is_some_and(|junction| junctions.get(junction).is_err())
    {
        selection.junction = None;
    }
    let Some(building) = selection.building else {
        return;
    };
    if buildings.get(building).is_err() {
        *selection = Selection::default();
        return;
    }
    if selection.port.is_some_and(|port| ports.get(port).is_err()) {
        selection.port = None;
    }
}

/// Put down what the select tool picked out when the player picks up another tool.
///
/// The panel is the tool's, and the keys that change what it shows are the tool's too, so one
/// left on screen under the road tool is a reading the player cannot act on.
fn put_the_selection_down_with_the_tool(mut selection: ResMut<Selection>) {
    selection.set_if_neq(Selection::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::BuildingPlugin;
    use crate::common::cleanup::CleanupPlugin;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::map::{HexCoordinates, TileCorner, MAP_TILE_SIZE};
    use crate::road::{Road, RoadPlugin};
    use crate::testing::{headless_app, tick};

    /// The tile the building under test stands on, in offset-row coordinates.
    const STANDING: (i32, i32) = (0, 0);

    /// How far through the catalogue a type taking one item in and putting one out sits.
    ///
    /// The first assembler of `BuildingType::ALL`: an extractor takes nothing in, so it stands no
    /// intake, and an intake is what a fleet is given to.
    const MELTER: isize = 5;

    /// The corner the melter's intake stands on, which is `INTAKE_CORNERS[0]` unturned.
    const INTAKE: TileCorner = TileCorner::SouthWest;

    /// The corner the melter's outlet stands on, which is `OUTLET_CORNERS[0]` unturned.
    const OUTLET: TileCorner = TileCorner::North;

    fn selection_app() -> App {
        let mut app = headless_app();
        app.insert_state(PlayerAction::EditBuildings)
            .insert_resource(PlayerInput::default())
            .add_plugins((
                BuildingPlugin,
                CleanupPlugin,
                DebugGizmosPlugin,
                RoadPlugin,
                SelectionPlugin,
            ));
        app
    }

    fn hold(app: &mut App, tool: PlayerAction) {
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(tool);
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

    /// Put a melter on `offsets` with the building tool, and answer with the tile it stands on.
    fn place_a_melter(app: &mut App, offsets: (i32, i32)) -> Entity {
        for _ in 0..MELTER {
            app.world_mut()
                .resource_mut::<crate::building::ChosenBuildingType>()
                .step(1);
        }
        let tile = spawn_tile(app, offsets);
        click_at(app, Some(tile), tile_of(offsets).world_position());
        tile
    }

    /// Click with the cursor over `tile` at `point`, then let the button go.
    fn click_at(app: &mut App, tile: Option<Entity>, point: Vec3) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap(true);
            input.cursor_tile = tile;
            input.world_cursor_position = Some(point);
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap(false);
    }

    fn selected(app: &App) -> Selection {
        *app.world().resource::<Selection>()
    }

    fn building_on(app: &App, offsets: (i32, i32)) -> Entity {
        app.world()
            .resource::<BuildingTiles>()
            .building_on(tile_of(offsets))
            .expect("a building stands there")
    }

    /// The port of the building on `offsets` standing on `corner`.
    fn port_at(app: &mut App, offsets: (i32, i32), corner: TileCorner) -> Entity {
        let node = corner.node_of(tile_of(offsets));
        let building = building_on(app, offsets);
        let children = app
            .world()
            .get::<Children>(building)
            .expect("a building has its ports")
            .iter()
            .collect::<Vec<_>>();
        children
            .into_iter()
            .find(|port| {
                app.world()
                    .get::<RoadEndpoint>(*port)
                    .is_some_and(|endpoint| endpoint.standing_on() == node)
            })
            .expect("a port stands on that corner")
    }

    /// A run of tiles a road is laid along, in offset-row coordinates.
    const ALONG: [(i32, i32); 4] = [(1, 0), (2, 0), (3, 0), (4, 0)];

    /// A run of tiles crossing that road at its second tile, in offset-row coordinates.
    const ACROSS: [(i32, i32); 3] = [(2, -1), (2, 0), (2, 1)];

    /// Lay two roads across each other, and answer with the junction and where it stands.
    fn a_crossroads(app: &mut App) -> (Entity, Vec3) {
        for run in [ALONG.as_slice(), ACROSS.as_slice()] {
            let nodes = run
                .iter()
                .map(|&offsets| LatticeNode::from_tile(tile_of(offsets)))
                .collect();
            app.world_mut().spawn(Road {
                nodes,
                leaving: None,
                one_way: false,
            });
        }
        tick(app);
        tick(app);

        let mut found: Vec<(Entity, Vec3)> = app
            .world_mut()
            .query::<(Entity, &Junction)>()
            .iter(app.world())
            .map(|(entity, junction)| (entity, junction.at))
            .collect();
        let one = found.pop();
        assert!(found.is_empty(), "more than one junction");
        one.expect("the roads cross")
    }

    #[test]
    fn a_click_on_a_junction_picks_it_out() {
        let mut app = selection_app();
        let (junction, at) = a_crossroads(&mut app);
        hold(&mut app, PlayerAction::Select);

        click_at(&mut app, None, at);

        assert_eq!(selected(&app).junction(), Some(junction));
    }

    #[test]
    fn a_click_off_every_junction_picks_none_of_them() {
        let mut app = selection_app();
        let (_, at) = a_crossroads(&mut app);
        hold(&mut app, PlayerAction::Select);

        click_at(&mut app, None, at + Vec3::X * MAP_TILE_SIZE);

        assert_eq!(selected(&app).junction(), None);
    }

    #[test]
    fn a_junction_the_world_no_longer_holds_is_let_go_of() {
        let mut app = selection_app();
        let (junction, at) = a_crossroads(&mut app);
        hold(&mut app, PlayerAction::Select);
        click_at(&mut app, None, at);

        app.world_mut().entity_mut(junction).despawn();
        tick(&mut app);

        assert_eq!(selected(&app).junction(), None);
    }

    #[test]
    fn clicking_the_middle_of_a_tile_selects_the_building_standing_on_it() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);

        click_at(&mut app, Some(tile), tile_of(STANDING).world_position());

        assert_eq!(selected(&app).building(), Some(building_on(&app, STANDING)));
        assert_eq!(selected(&app).port(), None);
    }

    #[test]
    fn clicking_near_a_corner_selects_the_port_standing_there() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);

        click_at(
            &mut app,
            Some(tile),
            INTAKE.node_of(tile_of(STANDING)).world_position(),
        );

        let intake = port_at(&mut app, STANDING, INTAKE);
        assert_eq!(selected(&app).port(), Some(intake));
        assert_eq!(selected(&app).building(), Some(building_on(&app, STANDING)));
    }

    #[test]
    fn clicking_a_corner_no_port_stands_on_selects_the_building_alone() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);

        click_at(
            &mut app,
            Some(tile),
            TileCorner::South
                .node_of(tile_of(STANDING))
                .world_position(),
        );

        assert_eq!(selected(&app).building(), Some(building_on(&app, STANDING)));
        assert_eq!(selected(&app).port(), None);
    }

    #[test]
    fn clicking_a_tile_with_no_building_selects_nothing() {
        let mut app = selection_app();
        place_a_melter(&mut app, STANDING);
        let bare = spawn_tile(&mut app, (3, 3));
        hold(&mut app, PlayerAction::Select);
        click_at(&mut app, Some(bare), tile_of((3, 3)).world_position());

        assert_eq!(selected(&app), Selection::default());
    }

    #[test]
    fn a_click_with_another_tool_held_picks_nothing_out() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::EditRoads);

        click_at(&mut app, Some(tile), tile_of(STANDING).world_position());

        assert_eq!(selected(&app), Selection::default());
    }

    #[test]
    fn a_building_taken_off_the_map_is_no_longer_selected() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);
        click_at(
            &mut app,
            Some(tile),
            INTAKE.node_of(tile_of(STANDING)).world_position(),
        );

        let building = building_on(&app, STANDING);
        app.world_mut()
            .entity_mut(building)
            .insert(crate::common::cleanup::Destroy);
        tick(&mut app);
        tick(&mut app);

        assert_eq!(selected(&app), Selection::default());
    }

    #[test]
    fn putting_the_select_tool_down_clears_the_selection() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);
        click_at(&mut app, Some(tile), tile_of(STANDING).world_position());

        hold(&mut app, PlayerAction::EditRoads);

        assert_eq!(selected(&app), Selection::default());
    }

    #[test]
    fn the_outlet_and_the_intake_are_told_apart_by_the_corner_clicked() {
        let mut app = selection_app();
        let tile = place_a_melter(&mut app, STANDING);
        hold(&mut app, PlayerAction::Select);

        click_at(
            &mut app,
            Some(tile),
            OUTLET.node_of(tile_of(STANDING)).world_position(),
        );

        let outlet = port_at(&mut app, STANDING, OUTLET);
        assert_eq!(selected(&app).port(), Some(outlet));
    }
}
