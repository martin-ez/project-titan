use crate::common::cleanup::Destroy;
use crate::common::cursor::CursorSurface;
use crate::common::initialize::{initialize_system, Initialize, NeedsInitialization};
use crate::diagnostics::DebugGizmos;
use crate::input::{PlayerAction, PlayerInput};
use crate::map::{
    HexCoordinates, LatticeNode, MapTile, RawMaterial, TileCorner, MAP_TILE_INRADIUS, MAP_TILE_SIZE,
};
use crate::road::{RoadEndpoint, RoadTiles};
use crate::ui::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashMap;

const BUILDING_HEIGHT: f32 = 4.;
const BUILDING_WIDTH: f32 = MAP_TILE_SIZE / 2.;

/// How far the debug view lifts the mark on a tile, so it does not fight the tile it lies on.
const GIZMO_LIFT: Vec3 = Vec3::new(0., 0.1, 0.);

/// The colour a tile that will not take a building is crossed out in
const REFUSED_COLOUR: Color = Color::srgb(0.95, 0.3, 0.3);

/// How far the cross on a refused tile reaches, as a share of the tile's inradius.
const REFUSED_MARK: f32 = 0.5;

/// How far into the tile a port's arrow reaches, as a share of the tile's inradius.
const PORT_MARK: f32 = 0.4;

/// The colour a port taking goods off the road is drawn in
const INTAKE_COLOUR: Color = Color::srgb(0.4, 0.75, 0.95);

/// The colour a port handing goods to the road is drawn in
const OUTLET_COLOUR: Color = Color::srgb(0.95, 0.8, 0.35);

/// The corners a building takes goods in on, in the order its recipe names what it consumes.
///
/// Three of the six, because the widest recipe of the production tree takes three items. The
/// first of them is a corner of the same three-tile class as the first `OUTLET_CORNERS`, so a
/// building's intake stands on the very corner another building's outlet does one tile away —
/// two links of a chain served by one road node, which is density the player earns by placing
/// well.
const INTAKE_CORNERS: [TileCorner; 3] = [
    TileCorner::SouthWest,
    TileCorner::NorthWest,
    TileCorner::South,
];

/// The corners a building hands goods out on, in the order its recipe names what it makes.
const OUTLET_CORNERS: [TileCorner; 3] = [
    TileCorner::North,
    TileCorner::NorthEast,
    TileCorner::SouthEast,
];

/// The keys that step through the catalogue, and how far each steps through it.
const CHOOSE_KEYS: [(KeyCode, isize); 2] = [(KeyCode::KeyQ, -1), (KeyCode::KeyE, 1)];

pub struct BuildingPlugin;

/// A building, standing on the tile whose coordinates it carries.
#[derive(Component)]
#[require(
    Transform,
    InheritedVisibility,
    NeedsInitialization,
    CursorSurface = building_surface()
)]
struct Building {
    coordinates: HexCoordinates,
}

/// Which way goods move through a port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flow {
    /// Goods a rover brought, handed over here for the building to take in.
    Intake,
    /// Goods the building has made, standing here until a rover takes them away.
    Outlet,
}

/// One place a building hands one item to the road or takes it from it, on a corner of its tile.
///
/// A port is an entity of its own and a child of the building, an entity carrying one
/// `RoadEndpoint` and a building having several ports. That is also what makes it the thing a
/// rover is sent to, so a delivery names the door it is for rather than the building it is at.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Port {
    /// Which way this port moves goods.
    pub flow: Flow,
    /// The one item that passes through it.
    pub item: Item,
}

/// One good the production tree moves, which is either dug out of the ground or made from others.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Item {
    /// A material a deposit holds, which an extractor draws rather than a recipe making it.
    Raw(RawMaterial),
    Water,
    Hydrogen,
    Oxygen,
    CarbonDioxide,
    Carbon,
    Ammonia,
    SiliconWafer,
    Glass,
    SiliconCarbide,
    ActivatedCharcoal,
    RefinedCobalt,
    Electronics,
    HydrocarbonPolymer,
    FertilizedSoil,
    Battery,
    FuelCell,
    DomeHabitatPanel,
    HydroponicsBay,
}

/// How much of one item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stack {
    /// Which item this is a quantity of.
    pub item: Item,
    /// How many of it.
    pub count: u32,
}

/// What one run of a building takes in and what it puts out.
///
/// The quantities are `docs/production_tree.md`'s, which is where the balance they are chosen for
/// is argued. A recipe may put out more than one thing: where a reaction really splits, one
/// recipe makes both, so a byproduct nobody hauls away is a jam like any other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Recipe {
    /// What one run consumes.
    pub inputs: &'static [Stack],
    /// What one run produces.
    pub outputs: &'static [Stack],
}

/// What a building is made to do, which is what decides the ports it stands.
///
/// Two archetypes rather than a kind of building per row of the production tree: an extractor
/// draws any raw material, an assembler runs any recipe, and everything that tells one apart from
/// another is the configuration it carries. A type is a definition rather than an entity, so
/// every copy of one has the same ports and does the same thing.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildingType {
    /// Draws a raw material out of the ground it stands on: nothing in, one thing out.
    Extractor(RawMaterial),
    /// Runs one recipe of the production tree.
    Assembler(Recipe),
}

/// The type the building tool will place next, which the player steps through with `CHOOSE_KEYS`.
#[derive(Resource, Default)]
pub struct ChosenBuildingType(usize);

/// Which building stands on each tile of the map.
///
/// Keyed by the tile, because both rules it answers are asked of one: a tile carrying a building
/// refuses another, and an arc the road tool proposes over that tile is refused too. That second
/// reader is why the record is a resource rather than a marker on the tile, and it is `RoadTiles`
/// seen from the other end.
#[derive(Resource, Default)]
pub struct BuildingTiles {
    on: HashMap<HexCoordinates, Entity>,
}

/// The roof a building offers the cursor, claiming the whole of the tile it stands on.
fn building_surface() -> CursorSurface {
    CursorSurface {
        radius: MAP_TILE_SIZE / 2.,
        height: BUILDING_HEIGHT,
    }
}

#[derive(SystemParam)]
struct BuildingInitializeParams<'w, 's> {
    query: Query<'w, 's, (&'static mut Transform, &'static mut Visibility), With<Building>>,
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

impl BuildingTiles {
    /// The building standing on `tile`, of which there is at most one.
    pub fn building_on(&self, tile: HexCoordinates) -> Option<Entity> {
        self.on.get(&tile).copied()
    }

    fn claim(&mut self, tile: HexCoordinates, building: Entity) {
        self.on.insert(tile, building);
    }

    fn release(&mut self, tile: HexCoordinates) {
        self.on.remove(&tile);
    }
}

const fn stack(count: u32, item: Item) -> Stack {
    Stack { item, count }
}

const fn assembler(inputs: &'static [Stack], outputs: &'static [Stack]) -> BuildingType {
    BuildingType::Assembler(Recipe { inputs, outputs })
}

fn extracted(material: RawMaterial) -> &'static [Stack] {
    match material {
        RawMaterial::Ice => const { &[stack(1, Item::Raw(RawMaterial::Ice))] },
        RawMaterial::CarbonMonoxide => {
            const { &[stack(1, Item::Raw(RawMaterial::CarbonMonoxide))] }
        }
        RawMaterial::Nitrogen => const { &[stack(1, Item::Raw(RawMaterial::Nitrogen))] },
        RawMaterial::Silicon => const { &[stack(1, Item::Raw(RawMaterial::Silicon))] },
        RawMaterial::CobaltOre => const { &[stack(1, Item::Raw(RawMaterial::CobaltOre))] },
    }
}

fn counted(stacks: &[Stack]) -> String {
    stacks
        .iter()
        .map(|stack| format!("{} {}", stack.count, stack.item.name()))
        .collect::<Vec<String>>()
        .join(" + ")
}

impl Item {
    /// What the player is told this item is called.
    pub fn name(self) -> &'static str {
        match self {
            Self::Raw(material) => material.name(),
            Self::Water => "Water",
            Self::Hydrogen => "Hydrogen",
            Self::Oxygen => "Oxygen",
            Self::CarbonDioxide => "Carbon Dioxide",
            Self::Carbon => "Carbon",
            Self::Ammonia => "Ammonia",
            Self::SiliconWafer => "Silicon Wafer",
            Self::Glass => "Glass",
            Self::SiliconCarbide => "Silicon Carbide",
            Self::ActivatedCharcoal => "Activated Charcoal",
            Self::RefinedCobalt => "Refined Cobalt",
            Self::Electronics => "Electronics",
            Self::HydrocarbonPolymer => "Hydrocarbon Polymer",
            Self::FertilizedSoil => "Fertilized Soil",
            Self::Battery => "Battery",
            Self::FuelCell => "Fuel Cell",
            Self::DomeHabitatPanel => "Dome Habitat Panel",
            Self::HydroponicsBay => "Hydroponics Bay",
        }
    }
}

impl BuildingType {
    /// Every type the player can place, transcribed from `docs/production_tree.md`.
    ///
    /// An extractor for each raw material, then an assembler for each recipe, in the tier order
    /// the tables come in. Adding a building to the game is a row here rather than a kind of
    /// building, which is what the two archetypes buy.
    pub const ALL: [Self; 22] = [
        Self::Extractor(RawMaterial::Ice),
        Self::Extractor(RawMaterial::CarbonMonoxide),
        Self::Extractor(RawMaterial::Nitrogen),
        Self::Extractor(RawMaterial::Silicon),
        Self::Extractor(RawMaterial::CobaltOre),
        assembler(
            &[stack(1, Item::Raw(RawMaterial::Ice))],
            &[stack(1, Item::Water)],
        ),
        assembler(
            &[stack(1, Item::Water)],
            &[stack(2, Item::Hydrogen), stack(1, Item::Oxygen)],
        ),
        assembler(
            &[
                stack(2, Item::Raw(RawMaterial::CarbonMonoxide)),
                stack(1, Item::Oxygen),
            ],
            &[stack(1, Item::CarbonDioxide)],
        ),
        assembler(
            &[stack(1, Item::CarbonDioxide)],
            &[stack(1, Item::Carbon), stack(1, Item::Oxygen)],
        ),
        assembler(
            &[
                stack(1, Item::Raw(RawMaterial::Nitrogen)),
                stack(3, Item::Hydrogen),
            ],
            &[stack(1, Item::Ammonia)],
        ),
        assembler(
            &[stack(1, Item::Raw(RawMaterial::Silicon))],
            &[stack(1, Item::SiliconWafer)],
        ),
        assembler(
            &[
                stack(1, Item::Raw(RawMaterial::Silicon)),
                stack(2, Item::Oxygen),
            ],
            &[stack(1, Item::Glass)],
        ),
        assembler(
            &[
                stack(1, Item::Raw(RawMaterial::Silicon)),
                stack(1, Item::Carbon),
            ],
            &[stack(1, Item::SiliconCarbide)],
        ),
        assembler(
            &[stack(2, Item::Carbon)],
            &[stack(1, Item::ActivatedCharcoal)],
        ),
        assembler(
            &[
                stack(4, Item::Water),
                stack(4, Item::Ammonia),
                stack(2, Item::ActivatedCharcoal),
            ],
            &[stack(1, Item::FertilizedSoil)],
        ),
        assembler(
            &[stack(2, Item::Raw(RawMaterial::CobaltOre))],
            &[stack(1, Item::RefinedCobalt)],
        ),
        assembler(
            &[stack(2, Item::SiliconWafer), stack(1, Item::RefinedCobalt)],
            &[stack(1, Item::Electronics)],
        ),
        assembler(
            &[stack(3, Item::Carbon), stack(6, Item::Hydrogen)],
            &[stack(1, Item::HydrocarbonPolymer)],
        ),
        assembler(
            &[
                stack(1, Item::RefinedCobalt),
                stack(1, Item::SiliconCarbide),
                stack(1, Item::HydrocarbonPolymer),
            ],
            &[stack(1, Item::Battery)],
        ),
        assembler(
            &[
                stack(2, Item::SiliconCarbide),
                stack(2, Item::Electronics),
                stack(2, Item::RefinedCobalt),
            ],
            &[stack(1, Item::FuelCell)],
        ),
        assembler(
            &[
                stack(2, Item::Glass),
                stack(1, Item::Battery),
                stack(1, Item::HydrocarbonPolymer),
            ],
            &[stack(1, Item::DomeHabitatPanel)],
        ),
        assembler(
            &[
                stack(2, Item::FuelCell),
                stack(4, Item::DomeHabitatPanel),
                stack(4, Item::FertilizedSoil),
            ],
            &[stack(1, Item::HydroponicsBay)],
        ),
    ];

    /// What one run of a building of this type takes in and puts out.
    ///
    /// An extractor draws what it stands on rather than taking anything in, so it is the same
    /// machine as an assembler with an empty left-hand side rather than a second kind of thing.
    pub fn recipe(self) -> Recipe {
        match self {
            Self::Extractor(material) => Recipe {
                inputs: &[],
                outputs: extracted(material),
            },
            Self::Assembler(recipe) => recipe,
        }
    }

    /// Which corner of its tile each of this type's ports stands on, and what passes through it.
    ///
    /// Derived from the recipe rather than declared beside it, so a type whose ports do not cover
    /// what it takes and makes cannot be written down: an intake per item consumed, an outlet per
    /// item produced, and the corners taken in the order the recipe names them.
    pub fn ports(self) -> impl Iterator<Item = (TileCorner, Port)> {
        let recipe = self.recipe();
        let intakes = recipe.inputs.iter().zip(INTAKE_CORNERS);
        let outlets = recipe.outputs.iter().zip(OUTLET_CORNERS);
        intakes
            .map(|(stack, corner)| (corner, Flow::Intake, stack.item))
            .chain(outlets.map(|(stack, corner)| (corner, Flow::Outlet, stack.item)))
            .map(|(corner, flow, item)| (corner, Port { flow, item }))
    }

    /// What the legend calls this type, which is what it draws or what it turns into what.
    pub fn label(self) -> String {
        match self {
            Self::Extractor(material) => format!("{} Extractor", material.name()),
            Self::Assembler(recipe) => {
                format!("{} → {}", counted(recipe.inputs), counted(recipe.outputs))
            }
        }
    }
}

impl ChosenBuildingType {
    /// The type the building tool will place on the next tap.
    pub fn chosen(&self) -> BuildingType {
        BuildingType::ALL[self.0]
    }

    fn step(&mut self, by: isize) {
        let catalogue = BuildingType::ALL.len() as isize;
        self.0 = (self.0 as isize + by).rem_euclid(catalogue) as usize;
    }
}

impl Plugin for BuildingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingTiles>()
            .init_resource::<ChosenBuildingType>()
            .declare_bindings([
                Binding {
                    input: BindingInput::Mouse(MouseButton::Left),
                    action: "Put a building on the tile",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
                Binding {
                    input: BindingInput::Mouse(MouseButton::Right),
                    action: "Take the building off the tile",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
                Binding {
                    input: BindingInput::Key(CHOOSE_KEYS[0].0),
                    action: "Choose the type before this one",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
                Binding {
                    input: BindingInput::Key(CHOOSE_KEYS[1].0),
                    action: "Choose the type after this one",
                    context: BindingContext::Tool(PlayerAction::EditBuildings),
                },
            ])
            .add_observer(release_the_tile_of_a_removed_building)
            .add_systems(
                PreUpdate,
                initialize_system::<Building, BuildingInitializeParams>,
            )
            .add_systems(
                Update,
                (
                    (
                        choose_building_type_system,
                        place_building_system,
                        remove_building_system,
                    )
                        .chain(),
                    draw_the_refused_tile,
                    draw_the_ports,
                    draw_the_type_under_the_cursor,
                ),
            );
    }
}

/// Whether `tile` will take a building, which is the whole of the rule and is asked in one place.
///
/// A road is read off the tile it runs over rather than measured out of its arcs, so the answer
/// costs a lookup however many roads are on the map.
fn takes_a_building(tile: HexCoordinates, buildings: &BuildingTiles, roads: &RoadTiles) -> bool {
    buildings.building_on(tile).is_none() && roads.roads_over(tile).is_empty()
}

/// Give up the tile a building held, whichever way it left the world.
fn release_the_tile_of_a_removed_building(
    removed: On<Remove, Building>,
    buildings: Query<&Building>,
    mut tiles: ResMut<BuildingTiles>,
) {
    if let Ok(building) = buildings.get(removed.entity) {
        tiles.release(building.coordinates);
    }
}

/// Put a building on the tile the cursor is over, when the player taps holding the building tool.
///
/// A tile takes one building and then refuses, so a tap on a tile that is already built on does
/// nothing rather than stacking a second on top of the first. A road running over the tile refuses
/// it the same way, whether the road stops there or only crosses it on the way somewhere else:
/// there is no room for a building on ground a rover drives over (invariant 1).
fn place_building_system(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Res<RoadTiles>,
    chosen: Res<ChosenBuildingType>,
    mut buildings: ResMut<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if !player_input.tap || *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(entity) = player_input.cursor_tile else {
        return;
    };
    let Ok(tile) = tiles.get(entity) else {
        return;
    };
    if !takes_a_building(tile.coordinates, &buildings, &roads) {
        return;
    }

    let kind = chosen.chosen();
    let building = commands
        .spawn((
            Building {
                coordinates: tile.coordinates,
            },
            kind,
            Visibility::Hidden,
        ))
        .with_children(|ports| {
            for (corner, port) in kind.ports() {
                ports.spawn((port, RoadEndpoint::at(corner.node_of(tile.coordinates))));
            }
        })
        .id();
    buildings.claim(tile.coordinates, building);
}

/// Step through the catalogue, so the tool places what the player chose rather than what it last
/// placed.
///
/// The choice is the tool's own record and nothing on the tick reads it, so it is settled on the
/// frame the key was pressed, like every other thing the player holds (invariant 2).
fn choose_building_type_system(
    input: Res<ButtonInput<KeyCode>>,
    action: Res<State<PlayerAction>>,
    mut chosen: ResMut<ChosenBuildingType>,
) {
    if *action.get() != PlayerAction::EditBuildings {
        return;
    }
    for (key, by) in CHOOSE_KEYS {
        if input.just_pressed(key) {
            chosen.step(by);
        }
    }
}

/// Take the building off the tile the cursor is over, when the player clicks the secondary button
/// holding the building tool.
///
/// The tile is left as placeable as it was before anything stood on it: the record of what stands
/// there goes with the building, so nothing is left behind to refuse the next one.
fn remove_building_system(
    mut commands: Commands,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    buildings: Res<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if !player_input.secondary_tap || *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(entity) = player_input.cursor_tile else {
        return;
    };
    let Ok(tile) = tiles.get(entity) else {
        return;
    };
    let Some(building) = buildings.building_on(tile.coordinates) else {
        return;
    };

    commands.entity(building).insert(Destroy);
}

/// Cross out the tile under the cursor when it will not take a building.
///
/// A refused tap is otherwise a click that does nothing, which reads as a game that missed it
/// rather than a tile that is taken. Marking it while the tool is held says so before the player
/// clicks, and says it the same way whether a building or a road is what is in the way.
fn draw_the_refused_tile(
    mut gizmos: Gizmos<DebugGizmos>,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Res<RoadTiles>,
    buildings: Res<BuildingTiles>,
    tiles: Query<&MapTile>,
) {
    if *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(tile) = player_input
        .cursor_tile
        .and_then(|entity| tiles.get(entity).ok())
    else {
        return;
    };
    if takes_a_building(tile.coordinates, &buildings, &roads) {
        return;
    }

    let centre = tile.coordinates.world_position() + GIZMO_LIFT;
    let reach = MAP_TILE_INRADIUS * REFUSED_MARK;
    for across in [Vec3::new(reach, 0., reach), Vec3::new(reach, 0., -reach)] {
        gizmos.line(centre - across, centre + across, REFUSED_COLOUR);
    }
}

/// Draw which way each port moves goods: in off the road, or out onto it.
///
/// The two ports of a building otherwise look alike, each a mark on a corner, and which of them a
/// rover should be sent to is the whole of what tells them apart (invariant 5).
fn draw_the_ports(
    mut gizmos: Gizmos<DebugGizmos>,
    buildings: Query<&Building>,
    ports: Query<(&Port, &RoadEndpoint, &ChildOf)>,
) {
    for (port, endpoint, of) in &ports {
        let Ok(building) = buildings.get(of.0) else {
            continue;
        };
        draw_a_port(
            &mut gizmos,
            building.coordinates,
            endpoint.standing_on(),
            port.flow,
        );
    }
}

/// Draw the ports the chosen type will stand, on the tile the tool would put it on.
///
/// What is drawn under the cursor is what the tap will place, so the player chooses by looking
/// rather than by placing one and reading it back off the map (invariant 5). A tile the tool will
/// refuse is left to `draw_the_refused_tile`, which says so instead.
fn draw_the_type_under_the_cursor(
    mut gizmos: Gizmos<DebugGizmos>,
    player_input: Res<PlayerInput>,
    action: Res<State<PlayerAction>>,
    roads: Res<RoadTiles>,
    buildings: Res<BuildingTiles>,
    chosen: Res<ChosenBuildingType>,
    tiles: Query<&MapTile>,
) {
    if *action.get() != PlayerAction::EditBuildings {
        return;
    }
    let Some(tile) = player_input
        .cursor_tile
        .and_then(|entity| tiles.get(entity).ok())
    else {
        return;
    };
    if !takes_a_building(tile.coordinates, &buildings, &roads) {
        return;
    }

    for (corner, port) in chosen.chosen().ports() {
        draw_a_port(
            &mut gizmos,
            tile.coordinates,
            corner.node_of(tile.coordinates),
            port.flow,
        );
    }
}

fn draw_a_port(
    gizmos: &mut Gizmos<DebugGizmos>,
    middle_of: HexCoordinates,
    standing_on: LatticeNode,
    flow: Flow,
) {
    let standing = standing_on.world_position() + GIZMO_LIFT;
    let middle = middle_of.world_position() + GIZMO_LIFT;
    let reach = standing + (middle - standing).normalize_or_zero() * MAP_TILE_INRADIUS * PORT_MARK;

    let (from, to, colour) = match flow {
        Flow::Intake => (standing, reach, INTAKE_COLOUR),
        Flow::Outlet => (reach, standing, OUTLET_COLOUR),
    };
    gizmos.arrow(from, to, colour);
}

impl Initialize<BuildingInitializeParams<'_, '_>> for Building {
    fn initialize(&mut self, entity: &Entity, params: &mut BuildingInitializeParams) -> Result {
        let (mut transform, mut visibility) = params.query.get_mut(*entity)?;
        transform.translation = self.coordinates.world_position();
        *visibility = Visibility::Visible;

        params.commands.entity(*entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(params.meshes.add(Cuboid::new(
                    BUILDING_WIDTH,
                    BUILDING_HEIGHT,
                    BUILDING_WIDTH,
                ))),
                MeshMaterial3d(params.materials.add(Color::srgb(0.55, 0.6, 0.72))),
                Transform::from_translation(Vec3::new(0., BUILDING_HEIGHT / 2., 0.)),
            ));
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::cleanup::CleanupPlugin;
    use crate::common::initialize::InitializationFailed;
    use crate::diagnostics::DebugGizmosPlugin;
    use crate::map::LatticeNode;
    use crate::road::{Road, RoadPlugin};
    use crate::rover::{Cargo, Route, Rover, RoverPlugin};
    use crate::simulation::SimulationPlugin;
    use crate::testing::{headless_app, tick};

    /// A run of tiles whose nodes are neighbours, so the road takes those tiles and no others.
    const NEIGHBOURING: [(i32, i32); 3] = [(0, 0), (1, 0), (2, 0)];

    /// Two nodes far enough apart that the road runs over the tiles between them.
    ///
    /// A road drawn between neighbouring tile centres never leaves the tiles its nodes stand on,
    /// so a rule tested only against one of those is a rule tested only at the nodes.
    const SPANNING: [(i32, i32); 2] = [(0, 0), (5, 0)];

    fn building_app(action: PlayerAction) -> App {
        let mut app = headless_app();
        app.insert_state(action)
            .insert_resource(PlayerInput::default())
            .add_plugins((BuildingPlugin, CleanupPlugin, DebugGizmosPlugin, RoadPlugin));
        app
    }

    /// Lay a road through `offsets` and let it take its tiles.
    fn spawn_road(app: &mut App, offsets: &[(i32, i32)]) -> Entity {
        let nodes = offsets
            .iter()
            .map(|&(col, row)| LatticeNode::from_tile(HexCoordinates::from_offset_row(col, row)))
            .collect();
        let road = app
            .world_mut()
            .spawn(Road {
                nodes,
                leaving: None,
                one_way: false,
            })
            .id();
        tick(app);
        road
    }

    fn spawn_tile(app: &mut App, col: i32, row: i32) -> Entity {
        app.world_mut()
            .spawn(MapTile {
                coordinates: HexCoordinates::from_offset_row(col, row),
            })
            .id()
    }

    /// Click on `tile`, then let the tap go, so a second frame is not a second click.
    fn tap_on(app: &mut App, tile: Option<Entity>) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.tap = true;
            input.cursor_tile = tile;
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().tap = false;
    }

    /// Right-click on `tile`, then let the button go, so a second frame is not a second click.
    fn secondary_tap_on(app: &mut App, tile: Option<Entity>) {
        {
            let mut input = app.world_mut().resource_mut::<PlayerInput>();
            input.secondary_tap = true;
            input.cursor_tile = tile;
        }
        tick(app);
        app.world_mut().resource_mut::<PlayerInput>().secondary_tap = false;
    }

    fn still_there(app: &App, entity: Entity) -> bool {
        app.world().entities().contains(entity)
    }

    fn buildings(app: &mut App) -> Vec<HexCoordinates> {
        let mut query = app.world_mut().query::<&Building>();
        query
            .iter(app.world())
            .map(|building| building.coordinates)
            .collect()
    }

    fn building_entity(app: &mut App) -> Option<Entity> {
        let mut query = app.world_mut().query_filtered::<Entity, With<Building>>();
        query.iter(app.world()).next()
    }

    #[test]
    fn a_tap_with_the_building_tool_puts_a_building_on_the_tile_under_the_cursor() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 2, 3);

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(2, 3)]);
    }

    /// The tile the ports under test stand on, in offset-row coordinates.
    const PORTED: (i32, i32) = (0, 0);

    /// The tile whose outlet stands on the same corner as `PORTED`'s intake, in offset-row
    /// coordinates. Two buildings placed there share one node of the network.
    const SHARING: (i32, i32) = (-1, 1);

    /// A corner of `PORTED` no port of a building standing there names.
    const UNPORTED: TileCorner = TileCorner::South;

    fn tile_at(offset: (i32, i32)) -> HexCoordinates {
        HexCoordinates::from_offset_row(offset.0, offset.1)
    }

    /// The corner a building's `which` port stands on, whatever corner its layout gives it.
    fn corner_for(which: Port) -> TileCorner {
        BUILDING_PORTS
            .into_iter()
            .find(|(_, port)| *port == which)
            .map(|(corner, _)| corner)
            .expect("every building has both an intake and an outlet")
    }

    /// Put a building on the tile at `offset`, which the tool takes a tap to do.
    fn place_building_at(app: &mut App, offset: (i32, i32)) -> Entity {
        let tile = spawn_tile(app, offset.0, offset.1);
        tap_on(app, Some(tile));
        buildings_in_the_world(app)
            .into_iter()
            .find(|&building| {
                app.world()
                    .entity(building)
                    .get::<Building>()
                    .is_some_and(|standing| standing.coordinates == tile_at(offset))
            })
            .expect("the tap placed a building")
    }

    /// Lay a road ending on `node`, setting off from a tile sharing it that is built on by none
    /// of `built_on`, so a road never has to run over a tile a building already holds.
    fn lay_road_to(app: &mut App, node: LatticeNode, built_on: &[(i32, i32)]) {
        let held: Vec<HexCoordinates> = built_on.iter().copied().map(tile_at).collect();
        let from = node
            .tiles_sharing()
            .expect("a corner is shared by three tiles")
            .into_iter()
            .find(|tile| !held.contains(tile))
            .expect("a corner has a tile no building stands on");
        app.world_mut().spawn(Road {
            nodes: vec![LatticeNode::from_tile(from), node],
            leaving: None,
            one_way: false,
        });
        tick(app);
    }

    fn ports_of(app: &mut App, building: Entity) -> Vec<(Port, Entity)> {
        let children: Vec<Entity> = app
            .world()
            .entity(building)
            .get::<Children>()
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        children
            .into_iter()
            .filter_map(|child| {
                app.world()
                    .entity(child)
                    .get::<Port>()
                    .map(|port| (*port, child))
            })
            .collect()
    }

    fn port_of(app: &mut App, building: Entity, which: Port) -> Entity {
        ports_of(app, building)
            .into_iter()
            .find(|&(port, _)| port == which)
            .map(|(_, entity)| entity)
            .unwrap_or_else(|| panic!("the building has no {which:?}"))
    }

    fn is_served(app: &App, port: Entity) -> bool {
        app.world()
            .entity(port)
            .get::<RoadEndpoint>()
            .and_then(RoadEndpoint::served_by)
            .is_some()
    }

    fn buildings_in_the_world(app: &mut App) -> Vec<Entity> {
        let mut query = app.world_mut().query_filtered::<Entity, With<Building>>();
        query.iter(app.world()).collect()
    }

    #[test]
    fn a_placed_building_has_one_port_for_each_corner_its_layout_names() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let building = place_building_at(&mut app, PORTED);

        let placed: Vec<Port> = ports_of(&mut app, building)
            .into_iter()
            .map(|(port, _)| port)
            .collect();

        assert_eq!(placed.len(), BUILDING_PORTS.len());
        for (_, port) in BUILDING_PORTS {
            assert!(placed.contains(&port), "no {port:?} among {placed:?}");
        }
    }

    #[test]
    fn a_road_on_the_corner_one_port_names_leaves_the_other_port_unserved() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let building = place_building_at(&mut app, PORTED);

        lay_road_to(
            &mut app,
            corner_for(Port::Intake).node_of(tile_at(PORTED)),
            &[PORTED],
        );

        let intake = port_of(&mut app, building, Port::Intake);
        let outlet = port_of(&mut app, building, Port::Outlet);
        assert!(is_served(&app, intake), "the road did not serve the intake");
        assert!(!is_served(&app, outlet), "the outlet was served too");
    }

    #[test]
    fn a_road_on_a_corner_no_port_names_serves_no_port_of_the_building() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let building = place_building_at(&mut app, PORTED);

        lay_road_to(&mut app, UNPORTED.node_of(tile_at(PORTED)), &[PORTED]);

        for (port, entity) in ports_of(&mut app, building) {
            assert!(!is_served(&app, entity), "{port:?} was served all the same");
        }
    }

    #[test]
    fn two_buildings_sharing_a_corner_are_both_served_by_the_one_road_node_on_it() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let taking = place_building_at(&mut app, PORTED);
        let giving = place_building_at(&mut app, SHARING);
        let shared = corner_for(Port::Intake).node_of(tile_at(PORTED));
        assert_eq!(
            shared,
            corner_for(Port::Outlet).node_of(tile_at(SHARING)),
            "the two buildings do not share a corner to be served on"
        );

        lay_road_to(&mut app, shared, &[PORTED, SHARING]);

        let intake = port_of(&mut app, taking, Port::Intake);
        let outlet = port_of(&mut app, giving, Port::Outlet);
        assert!(
            is_served(&app, intake),
            "the intake was left off the network"
        );
        assert!(
            is_served(&app, outlet),
            "the outlet was left off the network"
        );
    }

    /// How much a rover carries to a port in these tests.
    const LOAD: u32 = 3;

    /// A building app that also drives rovers, so a delivery can be run to a port and land.
    fn delivery_app() -> App {
        let mut app = building_app(PlayerAction::EditBuildings);
        app.add_plugins((SimulationPlugin, RoverPlugin));
        app
    }

    fn load_at(app: &App, entity: Entity) -> Option<u32> {
        app.world()
            .entity(entity)
            .get::<Cargo>()
            .map(|held| held.quantity)
    }

    #[test]
    fn a_rover_driven_to_a_port_leaves_its_load_there_rather_than_at_the_building() {
        let mut app = delivery_app();
        let building = place_building_at(&mut app, PORTED);
        let intake = port_of(&mut app, building, Port::Intake);
        lay_road_to(
            &mut app,
            corner_for(Port::Intake).node_of(tile_at(PORTED)),
            &[PORTED],
        );
        let stops = app
            .world()
            .entity(intake)
            .get::<RoadEndpoint>()
            .and_then(RoadEndpoint::served_by)
            .expect("the road serves the intake");
        app.world_mut().spawn((
            Rover {
                segment: stops.segment,
                along: stops.along,
            },
            Cargo { quantity: LOAD },
            Route {
                destination: intake,
                ways_out: Vec::new(),
            },
        ));

        tick(&mut app);

        assert_eq!(load_at(&app, intake), Some(LOAD));
        assert_eq!(load_at(&app, building), None);
    }

    #[test]
    fn taking_a_building_off_the_map_takes_its_ports_with_it() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, PORTED.0, PORTED.1);
        tap_on(&mut app, Some(tile));
        let building = building_entity(&mut app).expect("the tap placed a building");
        let ports: Vec<Entity> = ports_of(&mut app, building)
            .into_iter()
            .map(|(_, entity)| entity)
            .collect();
        assert_eq!(ports.len(), BUILDING_PORTS.len());

        secondary_tap_on(&mut app, Some(tile));

        for port in ports {
            assert!(!still_there(&app, port), "a port outlived its building");
        }
    }

    #[test]
    fn a_placed_building_stands_at_the_world_position_of_its_tile() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 2, 3);

        tap_on(&mut app, Some(tile));
        tick(&mut app);

        let building = building_entity(&mut app).expect("the tap placed a building");
        assert_eq!(
            app.world()
                .entity(building)
                .get::<Transform>()
                .map(|t| t.translation),
            Some(HexCoordinates::from_offset_row(2, 3).world_position())
        );
    }

    #[test]
    fn a_placed_building_is_there_to_see_once_it_is_initialized() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));
        tick(&mut app);

        let building = building_entity(&mut app).expect("the tap placed a building");
        let world = app.world();
        assert_eq!(
            world.entity(building).get::<Visibility>(),
            Some(&Visibility::Visible)
        );
        assert!(!world.entity(building).contains::<InitializationFailed>());
        assert!(world
            .entity(building)
            .get::<Children>()
            .is_some_and(|children| !children.is_empty()));
    }

    #[test]
    fn a_secondary_tap_takes_the_building_off_the_tile() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));

        secondary_tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tile_whose_building_was_removed_takes_another_one() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        let first = building_entity(&mut app).expect("the tap placed a building");
        secondary_tap_on(&mut app, Some(tile));

        tap_on(&mut app, Some(tile));

        let second = building_entity(&mut app).expect("the tile took another building");
        assert!(!still_there(&app, first));
        assert_ne!(second, first);
    }

    #[test]
    fn the_mesh_of_a_removed_building_goes_with_it() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        tick(&mut app);
        let building = building_entity(&mut app).expect("the tap placed a building");
        let mesh = app
            .world()
            .entity(building)
            .get::<Children>()
            .and_then(|children| children.iter().next())
            .expect("the building was given a mesh");

        secondary_tap_on(&mut app, Some(tile));

        assert!(!still_there(&app, building));
        assert!(!still_there(&app, mesh));
    }

    #[test]
    fn a_secondary_tap_while_selecting_leaves_the_building_alone() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::Select);
        tick(&mut app);

        secondary_tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_while_editing_roads_leaves_the_building_alone() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));
        app.world_mut()
            .resource_mut::<NextState<PlayerAction>>()
            .set(PlayerAction::EditRoads);
        tick(&mut app);

        secondary_tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_on_an_empty_tile_does_nothing() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let built = spawn_tile(&mut app, 0, 0);
        let empty = spawn_tile(&mut app, 1, 0);
        tap_on(&mut app, Some(built));

        secondary_tap_on(&mut app, Some(empty));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_secondary_tap_over_no_tile_does_nothing() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        tap_on(&mut app, Some(tile));

        secondary_tap_on(&mut app, None);

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_tap_while_selecting_puts_nothing_down() {
        let mut app = building_app(PlayerAction::Select);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_while_editing_roads_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditRoads);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_cursor_over_no_tile_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, None);

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn moving_over_a_tile_without_tapping_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);
        app.world_mut().resource_mut::<PlayerInput>().cursor_tile = Some(tile);

        tick(&mut app);

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_second_tap_on_an_occupied_tile_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));
        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app).len(), 1);
    }

    #[test]
    fn a_tap_on_a_free_tile_beside_an_occupied_one_still_places() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let occupied = spawn_tile(&mut app, 0, 0);
        let free = spawn_tile(&mut app, 1, 0);

        tap_on(&mut app, Some(occupied));
        tap_on(&mut app, Some(free));

        assert_eq!(buildings(&mut app).len(), 2);
    }

    #[test]
    fn a_building_offers_the_cursor_a_roof_to_climb_onto() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let tile = spawn_tile(&mut app, 0, 0);

        tap_on(&mut app, Some(tile));

        let building = building_entity(&mut app).expect("the tap placed a building");
        assert_eq!(
            app.world()
                .entity(building)
                .get::<CursorSurface>()
                .map(|s| s.height),
            Some(BUILDING_HEIGHT)
        );
    }

    #[test]
    fn a_tap_on_a_tile_a_road_stands_on_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_on_a_tile_a_road_only_crosses_puts_nothing_down() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &SPANNING);
        let tile = spawn_tile(&mut app, 3, 0);

        tap_on(&mut app, Some(tile));

        assert!(buildings(&mut app).is_empty());
    }

    #[test]
    fn a_tap_on_a_tile_beside_a_road_still_places_a_building() {
        let mut app = building_app(PlayerAction::EditBuildings);
        spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 1);

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(1, 1)]);
    }

    #[test]
    fn a_tile_whose_road_was_taken_away_takes_a_building() {
        let mut app = building_app(PlayerAction::EditBuildings);
        let road = spawn_road(&mut app, &NEIGHBOURING);
        let tile = spawn_tile(&mut app, 1, 0);
        app.world_mut().entity_mut(road).despawn();

        tap_on(&mut app, Some(tile));

        assert_eq!(buildings(&mut app), [HexCoordinates::from_offset_row(1, 0)]);
    }
}
