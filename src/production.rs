//! What a building makes, and what stops it making it.
//!
//! A building runs its type's recipe on the tick: when its intakes hold what the recipe takes and
//! its outlets have room for what it makes, it consumes the one, spends the recipe's ticks and
//! puts the other down. A run is all of its outputs or none of them, so a byproduct nobody hauls
//! away stops the building making it just as surely as a missing input does — which is most of
//! what a player is diagnosing when a chain that balanced on paper has stopped on the map.
//!
//! What a building can do next is a function of what stands at its ports and nothing else, so it
//! is worked out again only for a building whose stock moved and for one whose run came due. An
//! idle building nothing has delivered to is not looked at at all, which is what keeps the tick's
//! cost proportional to what is happening rather than to what is built.

use crate::building::{BuildingType, Flow, Holding, Item, Port, Recipe, Stack, PORT_CAPACITY};
use crate::fleet::FleetsServed;
use crate::rover::RoversDriven;
use crate::simulation::{Simulation, Ticks};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Runs each building's recipe on the simulation tick.
pub struct ProductionPlugin;

/// The set the recipes run in, so a system reading what a building made can follow them.
///
/// It runs last of the three that move stock, after the rovers have unloaded at the intakes they
/// reached and the fleets have collected from the outlets they serve. Both of those are named,
/// rather than one implying the other, so the order holds in an app carrying only one of them —
/// two systems writing the same port in no fixed order is a world that jams differently twice.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecipesRun;

/// A building part-way through a run of its recipe.
///
/// A component of its own rather than a state every building carries, so what is running is an
/// archetype the tick iterates rather than a set it has to go looking through the map for.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Running {
    /// The tick this run finishes on, settled from the recipe when the run started.
    pub finishes_on: u64,
}

/// A building that is not running, and what it is waiting for.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stopped(pub WaitingOn);

/// What a stopped building is waiting for.
///
/// A factory starved by a road nothing reaches and one jammed by a product nobody collects are
/// the same idle box otherwise, and which of the two it is is the whole of what the player has to
/// find out to unjam their chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitingOn {
    /// An intake is short of what the recipe takes.
    AnInput,
    /// An outlet has no room for what a run would put there.
    OutletRoom,
}

/// A building whose stock moved, so what it can do next may have changed.
#[derive(Message)]
struct StockMoved(Entity);

/// Everything running a recipe reads and writes: what a building is, and what stands at its ports.
#[derive(SystemParam)]
struct Machinery<'w, 's> {
    commands: Commands<'w, 's>,
    kinds: Query<'w, 's, (&'static BuildingType, &'static Children)>,
    ports: Query<'w, 's, (&'static Port, &'static mut Holding)>,
}

impl Plugin for ProductionPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<StockMoved>()
            .configure_sets(
                FixedUpdate,
                RecipesRun
                    .in_set(Simulation)
                    .after(RoversDriven)
                    .after(FleetsServed),
            )
            .add_systems(
                FixedUpdate,
                (
                    note_the_buildings_whose_stock_moved,
                    run_the_buildings_that_can_run,
                )
                    .chain()
                    .in_set(RecipesRun),
            );
    }
}

/// Name every building a port of which took something in or handed something out.
///
/// A system of its own because a `Changed<Holding>` filter reads what the run itself writes, and
/// one system cannot hold both. What it buys is the whole of the scanning rule: a building nobody
/// delivered to and nobody collected from is not named, so the run never looks at it.
fn note_the_buildings_whose_stock_moved(
    mut moved: MessageWriter<StockMoved>,
    ports: Query<&ChildOf, (With<Port>, Changed<Holding>)>,
) {
    for of in &ports {
        moved.write(StockMoved(of.0));
    }
}

/// Finish the runs that came due, then start one wherever a recipe can now be run.
///
/// Both halves visit the same building at most once a tick, so a run that finishes starts its
/// next on the tick it landed rather than the tick after — which is what makes a chain's output
/// the rate its recipe states rather than one a tick slower.
fn run_the_buildings_that_can_run(
    ticks: Res<Ticks>,
    mut moved: MessageReader<StockMoved>,
    mut looking_at: Local<Vec<Entity>>,
    running: Query<(Entity, &Running)>,
    mut machinery: Machinery,
) {
    looking_at.clear();
    looking_at.extend(
        running
            .iter()
            .filter(|(_, run)| run.finishes_on <= ticks.0)
            .map(|(building, _)| building),
    );
    looking_at.extend(moved.read().map(|moved| moved.0));
    looking_at.sort_unstable();
    looking_at.dedup();

    for building in looking_at.iter().copied() {
        match running.get(building) {
            Ok((_, run)) if run.finishes_on > ticks.0 => continue,
            Ok(_) => put_down_what_the_run_made(building, &mut machinery),
            Err(_) => {}
        }
        start_a_run(building, ticks.0, &mut machinery);
    }
}

/// Put down everything the run that came due at `building` made.
///
/// All of its outputs land, because the room for all of them was found before the run started and
/// nothing but a rover takes from an outlet in the meantime.
fn put_down_what_the_run_made(building: Entity, machinery: &mut Machinery) {
    let Ok((kind, _)) = machinery.kinds.get(building) else {
        return;
    };
    for stack in kind.recipe().outputs {
        machinery.put_out(building, *stack);
    }
    machinery.commands.entity(building).remove::<Running>();
}

/// Start a run at `building` if its recipe can be run, and say what it is waiting for if not.
fn start_a_run(building: Entity, now: u64, machinery: &mut Machinery) {
    let Ok((kind, _)) = machinery.kinds.get(building) else {
        return;
    };
    let recipe = kind.recipe();
    if let Some(waiting) = what_stops_a_run(building, recipe, machinery) {
        machinery
            .commands
            .entity(building)
            .remove::<Running>()
            .insert(Stopped(waiting));
        return;
    }

    for stack in recipe.inputs {
        machinery.take_in(building, *stack);
    }
    machinery
        .commands
        .entity(building)
        .remove::<Stopped>()
        .insert(Running {
            finishes_on: now + u64::from(recipe.ticks),
        });
}

/// What stops `building` running `recipe` this tick, or nothing at all when it can run.
///
/// The room for every output is found here rather than when the run lands, so a building never
/// makes what it has nowhere to put — a full byproduct port stops the run before it takes
/// anything in, which is the jam the player has to notice and haul away.
fn what_stops_a_run(building: Entity, recipe: Recipe, machinery: &Machinery) -> Option<WaitingOn> {
    for stack in recipe.inputs {
        match machinery.stood_at(building, Flow::Intake, stack.item) {
            Some(held) if held >= stack.count => {}
            _ => return Some(WaitingOn::AnInput),
        }
    }
    for stack in recipe.outputs {
        match machinery.stood_at(building, Flow::Outlet, stack.item) {
            Some(held) if held + stack.count <= PORT_CAPACITY => {}
            _ => return Some(WaitingOn::OutletRoom),
        }
    }
    None
}

impl Machinery<'_, '_> {
    /// The port of `building` that `item` passes through the way `flow` says, if it stands one.
    fn port_of(&self, building: Entity, flow: Flow, item: Item) -> Option<Entity> {
        let (_, children) = self.kinds.get(building).ok()?;
        let wanted = Port { flow, item };
        children.iter().find(|child| {
            self.ports
                .get(*child)
                .is_ok_and(|(port, _)| *port == wanted)
        })
    }

    /// How much of `item` stands at that port, and nothing at all where there is no such port.
    fn stood_at(&self, building: Entity, flow: Flow, item: Item) -> Option<u32> {
        let port = self.port_of(building, flow, item)?;
        self.ports.get(port).ok().map(|(_, holding)| holding.held())
    }

    /// Take `stack` off the intake it came in on, which `what_stops_a_run` found it standing at.
    fn take_in(&mut self, building: Entity, stack: Stack) {
        let Some(port) = self.port_of(building, Flow::Intake, stack.item) else {
            return;
        };
        if let Ok((_, mut holding)) = self.ports.get_mut(port) {
            holding.give_out(stack.count);
        }
    }

    /// Stand `stack` at the outlet it leaves on, which was found to have room before the run.
    fn put_out(&mut self, building: Entity, stack: Stack) {
        let Some(port) = self.port_of(building, Flow::Outlet, stack.item) else {
            return;
        };
        if let Ok((_, mut holding)) = self.ports.get_mut(port) {
            holding.take_in(stack.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{Item, Recipe, PORT_CAPACITY};
    use crate::map::RawMaterial;
    use crate::simulation::SimulationPlugin;
    use crate::testing::{headless_app, tick, trace};
    use std::time::Duration;

    /// A type taking one item in and putting one out, which is the plainest machine there is.
    const MELTER: BuildingType = BuildingType::Assembler(Recipe {
        inputs: &[Stack {
            item: ICE,
            count: 1,
        }],
        outputs: &[Stack {
            item: Item::Water,
            count: 1,
        }],
        ticks: A_RUN,
    });

    /// A type putting two items out of one, which is where a byproduct nobody wants comes from.
    const ELECTROLYSER: BuildingType = BuildingType::Assembler(Recipe {
        inputs: &[Stack {
            item: Item::Water,
            count: 1,
        }],
        outputs: &[
            Stack {
                item: Item::Hydrogen,
                count: 2,
            },
            Stack {
                item: Item::Oxygen,
                count: 1,
            },
        ],
        ticks: A_RUN,
    });

    /// A type taking two items in, so a delivery of one of them leaves it still short of the other.
    const AMMONIA_PLANT: BuildingType = BuildingType::Assembler(Recipe {
        inputs: &[
            Stack {
                item: Item::Raw(RawMaterial::Nitrogen),
                count: 1,
            },
            Stack {
                item: Item::Hydrogen,
                count: 3,
            },
        ],
        outputs: &[Stack {
            item: Item::Ammonia,
            count: 1,
        }],
        ticks: A_RUN,
    });

    /// A type drawing out of the ground it stands on, which takes nothing in at all.
    const EXTRACTOR: BuildingType = BuildingType::Extractor(RawMaterial::Ice);

    /// The item an extractor of ice draws and a melter takes in.
    const ICE: Item = Item::Raw(RawMaterial::Ice);

    /// How many ticks one run of every recipe written here takes.
    const A_RUN: u32 = 32;

    /// The tick a building spends being noticed before its first run can start.
    ///
    /// A building is put into the world between ticks and is first looked at on the tick after, so
    /// a window counted from when it was built holds one more tick than the runs that fit in it.
    const NOTICED_ON: u32 = 1;

    /// Take the frame `Time<Real>` sets its baseline on, which carries no tick of its own.
    ///
    /// Taken before anything is built, so every tick a test counts afterwards is one the
    /// simulation actually ran.
    fn take_the_baseline_frame(app: &mut App) {
        tick(app);
    }

    /// How many runs the rate claim is measured over, which is what makes it a count.
    const RUNS_MEASURED: u32 = 10;

    /// How many ticks a run at a steady frame rate is traced over, longer than several runs.
    const TICKS_TRACED: usize = 400;

    /// One frame to the tick, at the 15625 µs timestep every length below divides exactly.
    const A_TICK_A_FRAME: [Duration; 1] = [Duration::from_micros(15_625)];

    /// Frame lengths no clock hands out twice, one of them too short to carry a tick at all.
    const RAGGED_FRAMES: [Duration; 5] = [
        Duration::from_micros(3_125),
        Duration::from_micros(100),
        Duration::from_micros(46_875),
        Duration::from_micros(15_625),
        Duration::from_micros(625),
    ];

    /// A port the stand-in rovers keep stocked, standing for a supply nothing interrupts.
    #[derive(Component)]
    struct Stocked;

    /// A port the stand-in rovers empty, standing for a collection nothing interrupts.
    #[derive(Component)]
    struct Hauled;

    /// How much the hauling stand-in has taken away, which is what a chain's output is counted as.
    #[derive(Resource, Default)]
    struct HauledAway(u32);

    fn keep_the_stocked_ports_full(mut ports: Query<&mut Holding, With<Stocked>>) {
        for mut holding in &mut ports {
            holding.take_in(PORT_CAPACITY);
        }
    }

    fn haul_away_what_the_hauled_ports_hold(
        mut away: ResMut<HauledAway>,
        mut ports: Query<&mut Holding, With<Hauled>>,
    ) {
        for mut holding in &mut ports {
            away.0 += holding.give_out(PORT_CAPACITY);
        }
    }

    fn production_app() -> App {
        let mut app = headless_app();
        app.add_plugins((SimulationPlugin, ProductionPlugin))
            .init_resource::<HauledAway>()
            .add_systems(
                FixedUpdate,
                (
                    keep_the_stocked_ports_full.before(RecipesRun),
                    haul_away_what_the_hauled_ports_hold.after(RecipesRun),
                )
                    .in_set(Simulation),
            );
        take_the_baseline_frame(&mut app);
        app
    }

    /// Put a building of `kind` into the world, standing every port its recipe calls for.
    fn build(app: &mut App, kind: BuildingType) -> Entity {
        app.world_mut()
            .spawn(kind)
            .with_children(|ports| {
                for (_, port) in kind.ports() {
                    ports.spawn((port, Holding::default()));
                }
            })
            .id()
    }

    /// The port of `building` that `item` passes through in the direction `flow` names.
    fn port_of(app: &App, building: Entity, flow: Flow, item: Item) -> Entity {
        let world = app.world();
        let children = world
            .entity(building)
            .get::<Children>()
            .expect("a building stands its ports");
        children
            .iter()
            .find(|child| world.entity(*child).get::<Port>() == Some(&Port { flow, item }))
            .expect("the building stands the port asked for")
    }

    fn stock(app: &App, building: Entity, flow: Flow, item: Item) -> u32 {
        app.world()
            .entity(port_of(app, building, flow, item))
            .get::<Holding>()
            .map_or(0, Holding::held)
    }

    /// Stand `quantity` of `item` at a port, as a delivery or a run that has not been collected.
    fn stand(app: &mut App, building: Entity, flow: Flow, item: Item, quantity: u32) {
        let port = port_of(app, building, flow, item);
        app.world_mut()
            .entity_mut(port)
            .get_mut::<Holding>()
            .expect("a port holds what passes through it")
            .take_in(quantity);
    }

    /// Put the stand-in rovers marked by `mark` on a port, so it is kept full or kept empty.
    fn served_by<M: Component>(app: &mut App, building: Entity, flow: Flow, item: Item, mark: M) {
        let port = port_of(app, building, flow, item);
        app.world_mut().entity_mut(port).insert(mark);
    }

    fn run(app: &mut App, ticks: u32) {
        for _ in 0..ticks {
            tick(app);
        }
    }

    fn waiting_on(app: &App, building: Entity) -> Option<WaitingOn> {
        app.world().entity(building).get::<Stopped>().map(|it| it.0)
    }

    /// What a run of `app` hauled away by each of its first `TICKS_TRACED` ticks.
    fn hauled(world: &World) -> u32 {
        world.resource::<HauledAway>().0
    }

    #[test]
    fn an_extractor_draws_from_the_ground_with_no_input_at_all() {
        let mut app = production_app();
        let extractor = build(&mut app, EXTRACTOR);

        run(&mut app, NOTICED_ON + EXTRACTOR.recipe().ticks);

        assert_eq!(stock(&app, extractor, Flow::Outlet, ICE), 1);
    }

    #[test]
    fn a_building_takes_its_inputs_in_and_puts_its_outputs_out() {
        let mut app = production_app();
        let electrolyser = build(&mut app, ELECTROLYSER);
        stand(&mut app, electrolyser, Flow::Intake, Item::Water, 1);

        run(&mut app, NOTICED_ON + A_RUN);

        assert_eq!(stock(&app, electrolyser, Flow::Intake, Item::Water), 0);
        assert_eq!(stock(&app, electrolyser, Flow::Outlet, Item::Hydrogen), 2);
        assert_eq!(stock(&app, electrolyser, Flow::Outlet, Item::Oxygen), 1);
    }

    #[test]
    fn a_building_short_of_an_input_makes_nothing_and_says_what_it_wants() {
        let mut app = production_app();
        let melter = build(&mut app, MELTER);

        run(&mut app, NOTICED_ON + A_RUN);

        assert_eq!(stock(&app, melter, Flow::Outlet, Item::Water), 0);
        assert_eq!(waiting_on(&app, melter), Some(WaitingOn::AnInput));
    }

    #[test]
    fn a_building_holding_only_some_of_what_it_takes_consumes_none_of_it() {
        let mut app = production_app();
        let plant = build(&mut app, AMMONIA_PLANT);
        let nitrogen = Item::Raw(RawMaterial::Nitrogen);
        stand(&mut app, plant, Flow::Intake, nitrogen, 1);
        stand(&mut app, plant, Flow::Intake, Item::Hydrogen, 2);

        run(&mut app, NOTICED_ON + A_RUN);

        assert_eq!(stock(&app, plant, Flow::Intake, nitrogen), 1);
        assert_eq!(stock(&app, plant, Flow::Intake, Item::Hydrogen), 2);
        assert_eq!(stock(&app, plant, Flow::Outlet, Item::Ammonia), 0);
        assert_eq!(waiting_on(&app, plant), Some(WaitingOn::AnInput));
    }

    #[test]
    fn a_building_with_a_full_outlet_stops_rather_than_consuming_more() {
        let mut app = production_app();
        let melter = build(&mut app, MELTER);
        stand(&mut app, melter, Flow::Intake, ICE, PORT_CAPACITY);
        stand(&mut app, melter, Flow::Outlet, Item::Water, PORT_CAPACITY);

        run(&mut app, NOTICED_ON + A_RUN);

        assert_eq!(stock(&app, melter, Flow::Intake, ICE), PORT_CAPACITY);
        assert_eq!(
            stock(&app, melter, Flow::Outlet, Item::Water),
            PORT_CAPACITY
        );
        assert_eq!(waiting_on(&app, melter), Some(WaitingOn::OutletRoom));
    }

    #[test]
    fn a_full_byproduct_port_stops_the_product_nobody_is_waiting_for() {
        let mut app = production_app();
        let electrolyser = build(&mut app, ELECTROLYSER);
        served_by(&mut app, electrolyser, Flow::Intake, Item::Water, Stocked);
        served_by(&mut app, electrolyser, Flow::Outlet, Item::Hydrogen, Hauled);
        stand(
            &mut app,
            electrolyser,
            Flow::Outlet,
            Item::Oxygen,
            PORT_CAPACITY,
        );

        run(&mut app, NOTICED_ON + RUNS_MEASURED * A_RUN);

        assert_eq!(app.world().resource::<HauledAway>().0, 0);
        assert_eq!(waiting_on(&app, electrolyser), Some(WaitingOn::OutletRoom));
    }

    #[test]
    fn a_chain_makes_one_a_run_for_as_many_runs_as_it_is_given_ticks() {
        let mut app = production_app();
        let melter = build(&mut app, MELTER);
        served_by(&mut app, melter, Flow::Intake, ICE, Stocked);
        served_by(&mut app, melter, Flow::Outlet, Item::Water, Hauled);

        run(&mut app, NOTICED_ON + RUNS_MEASURED * A_RUN);

        assert_eq!(app.world().resource::<HauledAway>().0, RUNS_MEASURED);
    }

    #[test]
    fn a_trace_tells_a_chain_that_is_running_from_one_that_has_stopped() {
        let running = trace(
            a_melter_under_supply(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            hauled,
        );

        let starved = trace(
            a_melter_with_nothing_coming_in(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            hauled,
        );

        assert_ne!(running, starved);
    }

    #[test]
    fn the_same_chain_makes_the_same_total_however_ragged_the_frames() {
        let steady = trace(
            a_melter_under_supply(),
            &A_TICK_A_FRAME,
            TICKS_TRACED,
            hauled,
        );

        let ragged = trace(
            a_melter_under_supply(),
            &RAGGED_FRAMES,
            TICKS_TRACED,
            hauled,
        );

        assert_eq!(steady, ragged);
    }

    /// A melter whose intake never runs dry and whose outlet is never left to fill.
    fn a_melter_under_supply() -> App {
        let mut app = production_app();
        let melter = build(&mut app, MELTER);
        served_by(&mut app, melter, Flow::Intake, ICE, Stocked);
        served_by(&mut app, melter, Flow::Outlet, Item::Water, Hauled);
        app
    }

    /// A melter whose outlet is collected from but whose intake nothing ever delivers to.
    fn a_melter_with_nothing_coming_in() -> App {
        let mut app = production_app();
        let melter = build(&mut app, MELTER);
        served_by(&mut app, melter, Flow::Outlet, Item::Water, Hauled);
        app
    }
}
