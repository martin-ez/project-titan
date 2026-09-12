//! The production tree on screen, and the chain one item of it names.
//!
//! Every step the game can run and every good it moves, read off [`BuildingType::ALL`] — the same
//! definitions the tick runs — rather than copied beside them. A step is a node of its own, so one
//! that splits draws as a single step with both its products coming off it rather than as two
//! recipes the player could build separately.
//!
//! An item can be put in focus, and focus names a chain: everything that item needs, transitively,
//! and everything that needs it. That is the set "what do I need to build to get Electronics" asks
//! for, and it is worked out from the edges each redraw rather than stored, so it cannot drift
//! from the tree it is a fact about.
//!
//! Nothing here writes the world. Focus is the panel's own record and lives on the frame.

use crate::building::{BuildingType, Item};
use crate::input::{DeclareCommands, PlayerCommand, Requested};
use crate::ui::legend::{BindingCategory, BindingCondition, BindingInput};
use crate::ui::{overlay, panel_text, Panel, BODY_TEXT, HEADING_TEXT, KEYED_TEXT};
use bevy::prelude::*;
use std::collections::VecDeque;

/// Key binding that shows and hides the production tree
const TREE_KEY: KeyCode = KeyCode::F5;

/// The keys that walk the focus through the items, the way each walks it, and what to call that
///
/// Not the arrows, which the settings panel already asks for: a press reaches a command whatever
/// the legend is showing, so two open panels would both answer one press.
const FOCUS_KEYS: [(KeyCode, FocusStep, &str); 2] = [
    (
        KeyCode::KeyZ,
        FocusStep(-1),
        "Focus the item before this one",
    ),
    (KeyCode::KeyX, FocusStep(1), "Focus the item after this one"),
];

/// Key binding that takes the focus off, drawing the whole tree plain again
const CLEAR_KEY: KeyCode = KeyCode::KeyC;

/// What the panel calls itself
const HEADING: &str = "Production tree";

/// What the panel says under the heading while no item is in focus
const NOTHING_FOCUSED: &str = "Z and X focus an item, C clears it";

/// How wide a node of the tree is drawn, in logical pixels
const NODE_WIDTH: f32 = 104.0;

/// How tall a node of the tree is drawn, in logical pixels
const NODE_HEIGHT: f32 = 30.0;

/// How much space sits between one column of the tree and the next, in logical pixels
const COLUMN_GAP: f32 = 52.0;

/// How much space sits between one node of a column and the next, in logical pixels
const NODE_GAP: f32 = 10.0;

/// How thick an edge is drawn, in logical pixels
const EDGE_THICKNESS: f32 = 1.0;

/// What a node outside the focused chain is drawn in, and what one inside it is
const NODE_FILL: [Color; 2] = [
    Color::srgba(0.10, 0.10, 0.13, 0.9),
    Color::srgba(0.16, 0.22, 0.32, 0.95),
];

/// What an edge outside the focused chain is drawn in, and what one inside it is
const EDGE_FILL: [Color; 2] = [
    Color::srgba(0.30, 0.31, 0.34, 0.5),
    Color::srgba(0.62, 0.78, 0.98, 0.9),
];

/// One node of the tree: a good the chain moves, or a step that makes one.
///
/// A place in [`Tree::items`] or [`Tree::recipes`] rather than the thing itself, because a
/// [`Recipe`](crate::building::Recipe) holds its stacks by reference and so compares by contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TreeNode {
    /// A good, at this place in [`Tree::items`].
    Item(usize),
    /// A step, at this place in [`Tree::recipes`].
    Recipe(usize),
}

/// Which way along an edge to walk: towards what a node needs, or towards what needs it.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Direction {
    /// Towards what this node is made from.
    Needs,
    /// Towards what is made from this node.
    Feeds,
}

/// Every step the game can run and every good those steps move, with the edges between them.
///
/// Built from the catalogue rather than beside it, so a type added there is in the tree with no
/// second edit. A step is a node, which is what draws one that splits as a single step.
#[derive(Resource)]
struct Tree {
    items: Vec<Item>,
    recipes: Vec<BuildingType>,
    edges: Vec<(TreeNode, TreeNode)>,
}

/// One value held for each node of a tree, the way the tree holds the nodes themselves.
struct PerNode<T> {
    items: Vec<T>,
    recipes: Vec<T>,
}

/// Which item the tree is focused on, as a place in [`Tree::items`], and none while it is not.
#[derive(Resource, Default)]
struct Focus(Option<usize>);

/// The production tree, on a key, over whatever else is on screen.
pub struct ProductionTreePlugin;

/// The tree on screen, of which there is at most one.
#[derive(Component)]
struct ProductionTree;

/// One node of the tree as it is drawn, which is what a test reads the focused chain off.
#[derive(Component)]
struct DrawnNode;

/// What the player asks for to show or hide the tree.
#[derive(Clone, Copy, PartialEq)]
struct ShowTheTree;

/// How far through the items the player asked the focus to move, negative to move it back.
#[derive(Clone, Copy, PartialEq)]
struct FocusStep(isize);

/// What the player asks for to take the focus off again.
#[derive(Clone, Copy, PartialEq)]
struct ClearTheFocus;

impl Plugin for ProductionTreePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Tree::of(&BuildingType::ALL))
            .init_resource::<Focus>()
            .declare_commands([PlayerCommand {
                input: BindingInput::Key(TREE_KEY),
                asks: ShowTheTree,
                action: "Show or hide the production tree",
                category: BindingCategory::Panels,
            }])
            .declare_commands_when(
                BindingCondition::PanelOpen(Panel::ProductionTree),
                FOCUS_KEYS.map(|(key, asks, action)| PlayerCommand {
                    input: BindingInput::Key(key),
                    asks,
                    action,
                    category: BindingCategory::Panels,
                }),
            )
            .declare_commands_when(
                BindingCondition::PanelOpen(Panel::ProductionTree),
                [PlayerCommand {
                    input: BindingInput::Key(CLEAR_KEY),
                    asks: ClearTheFocus,
                    action: "Take the focus off the tree",
                    category: BindingCategory::Panels,
                }],
            )
            .add_systems(
                Update,
                (
                    toggle_the_tree,
                    step_the_focus,
                    clear_the_focus,
                    redraw_the_tree,
                )
                    .chain(),
            );
    }
}

impl Tree {
    /// The tree `catalogue` describes: a node per step, a node per good, and the edges between.
    fn of(catalogue: &[BuildingType]) -> Self {
        let mut tree = Self {
            items: Vec::new(),
            recipes: catalogue.to_vec(),
            edges: Vec::new(),
        };
        for (place, kind) in catalogue.iter().enumerate() {
            let recipe = kind.recipe();
            for stack in recipe.inputs {
                let item = tree.node_of(stack.item);
                tree.edges.push((item, TreeNode::Recipe(place)));
            }
            for stack in recipe.outputs {
                let item = tree.node_of(stack.item);
                tree.edges.push((TreeNode::Recipe(place), item));
            }
        }
        tree
    }

    /// The node `item` is drawn as, adding it to the tree if no step has mentioned it yet.
    fn node_of(&mut self, item: Item) -> TreeNode {
        let place = self.items.iter().position(|held| *held == item);
        TreeNode::Item(place.unwrap_or_else(|| {
            self.items.push(item);
            self.items.len() - 1
        }))
    }

    fn nodes(&self) -> impl Iterator<Item = TreeNode> + '_ {
        (0..self.items.len())
            .map(TreeNode::Item)
            .chain((0..self.recipes.len()).map(TreeNode::Recipe))
    }

    fn neighbours(&self, node: TreeNode, way: Direction) -> impl Iterator<Item = TreeNode> + '_ {
        self.edges.iter().filter_map(move |(from, to)| match way {
            Direction::Feeds if *from == node => Some(*to),
            Direction::Needs if *to == node => Some(*from),
            _ => None,
        })
    }

    /// The chain `focus` names: everything it needs, everything that needs it, and itself.
    ///
    /// Both walks carry what they have already reached, which is what lets a chain that loops back
    /// on itself — oxygen feeds the step that makes the carbon dioxide another step splits back
    /// into oxygen — be walked at all.
    fn chain_from(&self, focus: TreeNode) -> Vec<TreeNode> {
        let mut reached = vec![focus];
        for way in [Direction::Needs, Direction::Feeds] {
            let mut walking = VecDeque::from([focus]);
            while let Some(node) = walking.pop_front() {
                for next in self.neighbours(node, way).collect::<Vec<TreeNode>>() {
                    if reached.contains(&next) {
                        continue;
                    }
                    reached.push(next);
                    walking.push_back(next);
                }
            }
        }
        reached
    }

    /// Which column each node is drawn in: a step one past the deepest good it takes, and a good
    /// one past the shallowest step that makes it.
    ///
    /// The shallowest rather than the deepest is what keeps a loop from carrying a good rightward
    /// without end, and the pass count bounds it whatever the catalogue turns out to be shaped
    /// like: a column only ever rises, so a pass that moves nothing has settled.
    fn columns(&self) -> PerNode<usize> {
        let mut columns = PerNode::filled(self, 0);
        for _ in 0..self.nodes().count() {
            let mut moved = false;
            for place in 0..self.recipes.len() {
                let node = TreeNode::Recipe(place);
                let deepest = self.column_of(&columns, node, Ord::max);
                moved |= columns.raise(node, deepest.unwrap_or(0));
            }
            for place in 0..self.items.len() {
                let node = TreeNode::Item(place);
                let Some(shallowest) = self.column_of(&columns, node, Ord::min) else {
                    continue;
                };
                moved |= columns.raise(node, shallowest);
            }
            if !moved {
                break;
            }
        }
        columns
    }

    /// Which column `node` sits in given what it needs, `pick` choosing among several.
    fn column_of(
        &self,
        columns: &PerNode<usize>,
        node: TreeNode,
        pick: fn(usize, usize) -> usize,
    ) -> Option<usize> {
        self.neighbours(node, Direction::Needs)
            .map(|needed| columns.of(needed) + 1)
            .reduce(pick)
    }

    /// Where each node is drawn, in logical pixels from the top left of the sheet.
    fn places(&self) -> PerNode<Vec2> {
        let columns = self.columns();
        let mut places = PerNode::filled(self, Vec2::ZERO);
        let mut taken: Vec<usize> = Vec::new();
        for node in self.nodes().collect::<Vec<TreeNode>>() {
            let column = columns.of(node);
            taken.resize(taken.len().max(column + 1), 0);
            places.set(
                node,
                Vec2::new(
                    column as f32 * (NODE_WIDTH + COLUMN_GAP),
                    taken[column] as f32 * (NODE_HEIGHT + NODE_GAP),
                ),
            );
            taken[column] += 1;
        }
        places
    }

    /// What a node is written with: a good by its name, a step by how long one run of it takes.
    ///
    /// A step's own edges already draw what it takes and what it makes, so its run time is the one
    /// fact about it the tree would not otherwise be saying.
    fn label_of(&self, node: TreeNode) -> String {
        match node {
            TreeNode::Item(place) => self.items[place].name().to_string(),
            TreeNode::Recipe(place) => format!("{} ticks", self.recipes[place].recipe().ticks),
        }
    }
}

impl<T: Copy> PerNode<T> {
    fn filled(tree: &Tree, value: T) -> Self {
        Self {
            items: vec![value; tree.items.len()],
            recipes: vec![value; tree.recipes.len()],
        }
    }

    fn of(&self, node: TreeNode) -> T {
        match node {
            TreeNode::Item(place) => self.items[place],
            TreeNode::Recipe(place) => self.recipes[place],
        }
    }

    fn set(&mut self, node: TreeNode, value: T) {
        match node {
            TreeNode::Item(place) => self.items[place] = value,
            TreeNode::Recipe(place) => self.recipes[place] = value,
        }
    }
}

impl PerNode<usize> {
    fn raise(&mut self, node: TreeNode, to: usize) -> bool {
        if to <= self.of(node) {
            return false;
        }
        self.set(node, to);
        true
    }
}

fn toggle_the_tree(
    mut commands: Commands,
    asked_for: Res<Requested<ShowTheTree>>,
    tree: Res<Tree>,
    focus: Res<Focus>,
    panels: Query<Entity, With<ProductionTree>>,
) {
    if !asked_for.asked(ShowTheTree) {
        return;
    }

    match panels.iter().next() {
        Some(panel) => {
            commands.entity(panel).despawn();
        }
        None => {
            commands
                .spawn((ProductionTree, overlay(Panel::ProductionTree)))
                .with_children(|sheet| fill_the_sheet(sheet, &tree, focus.0));
        }
    }
}

/// Walk the focus through the items, going round rather than stopping at either end.
fn step_the_focus(
    asked_for: Res<Requested<FocusStep>>,
    tree: Res<Tree>,
    mut focus: ResMut<Focus>,
    panels: Query<Entity, With<ProductionTree>>,
) {
    if panels.is_empty() {
        return;
    }
    let items = tree.items.len() as isize;
    if items == 0 {
        return;
    }

    for FocusStep(by) in asked_for.iter() {
        let stepped = match focus.0 {
            Some(place) => place as isize + by,
            None if by > 0 => 0,
            None => items - 1,
        };
        focus.0 = Some(stepped.rem_euclid(items) as usize);
    }
}

fn clear_the_focus(
    asked_for: Res<Requested<ClearTheFocus>>,
    mut focus: ResMut<Focus>,
    panels: Query<Entity, With<ProductionTree>>,
) {
    if panels.is_empty() || !asked_for.asked(ClearTheFocus) {
        return;
    }
    focus.0 = None;
}

/// Draw the tree again whenever the focus has moved under it.
///
/// The sheet keeps its entity and only what is on it is built again, so a tree already on screen
/// stays the one on screen rather than blinking out and back.
fn redraw_the_tree(
    mut commands: Commands,
    tree: Res<Tree>,
    focus: Res<Focus>,
    panels: Query<Entity, With<ProductionTree>>,
) {
    if !focus.is_changed() {
        return;
    }

    for standing in &panels {
        commands
            .entity(standing)
            .despawn_related::<Children>()
            .with_children(|sheet| fill_the_sheet(sheet, &tree, focus.0));
    }
}

/// Write the heading, then lay the tree out under it with the focused chain picked out.
fn fill_the_sheet(sheet: &mut ChildSpawnerCommands, tree: &Tree, focus: Option<usize>) {
    sheet.spawn(panel_text(HEADING.to_string(), HEADING_TEXT, Val::Auto));
    sheet.spawn(panel_text(focused_on(tree, focus), BODY_TEXT, Val::Auto));

    let chain = focus.map(|place| tree.chain_from(TreeNode::Item(place)));
    let lit = |node: TreeNode| chain.as_ref().is_none_or(|chain| chain.contains(&node));
    let places = tree.places();
    sheet
        .spawn(Node {
            flex_grow: 1.0,
            ..default()
        })
        .with_children(|canvas| {
            for (from, to) in &tree.edges {
                canvas.spawn(edge(
                    places.of(*from) + Vec2::new(NODE_WIDTH, NODE_HEIGHT / 2.0),
                    places.of(*to) + Vec2::new(0.0, NODE_HEIGHT / 2.0),
                    lit(*from) && lit(*to),
                ));
            }
            for node in tree.nodes().collect::<Vec<TreeNode>>() {
                canvas
                    .spawn(drawn_node(places.of(node), lit(node)))
                    .with_children(|box_of| {
                        box_of.spawn(panel_text(
                            tree.label_of(node),
                            text_colour(node, lit(node)),
                            Val::Auto,
                        ));
                    });
            }
        });
}

/// What the panel says the focus is on, which is what a chain the player is reading is named by.
fn focused_on(tree: &Tree, focus: Option<usize>) -> String {
    match focus.and_then(|place| tree.items.get(place)) {
        Some(item) => format!("Focused on {}", item.name()),
        None => NOTHING_FOCUSED.to_string(),
    }
}

/// One node of the tree, as a box standing at `place` on the sheet.
fn drawn_node(place: Vec2, lit: bool) -> impl Bundle {
    (
        DrawnNode,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(place.x),
            top: Val::Px(place.y),
            width: Val::Px(NODE_WIDTH),
            height: Val::Px(NODE_HEIGHT),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(NODE_FILL[usize::from(lit)]),
    )
}

/// One edge of the tree, as a hairline turned to span the gap from `from` to `to`.
fn edge(from: Vec2, to: Vec2, lit: bool) -> impl Bundle {
    let span = to - from;
    let middle = from.midpoint(to);
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(middle.x - span.length() / 2.0),
            top: Val::Px(middle.y - EDGE_THICKNESS / 2.0),
            width: Val::Px(span.length()),
            height: Val::Px(EDGE_THICKNESS),
            ..default()
        },
        UiTransform::from_rotation(Rot2::radians(span.to_angle())),
        BackgroundColor(EDGE_FILL[usize::from(lit)]),
    )
}

fn text_colour(node: TreeNode, lit: bool) -> Color {
    match (node, lit) {
        (TreeNode::Item(_), true) => KEYED_TEXT,
        _ => BODY_TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building::{Flow, Holding, Port, Recipe, Stack};
    use crate::map::RawMaterial;
    use crate::production::ProductionPlugin;
    use crate::simulation::SimulationPlugin;
    use crate::testing::{ask_for, headless_app, tick};

    const ICE: Item = Item::Raw(RawMaterial::Ice);
    const SILICON: Item = Item::Raw(RawMaterial::Silicon);

    const fn stack(count: u32, item: Item) -> Stack {
        Stack { item, count }
    }

    const fn assembler(
        inputs: &'static [Stack],
        outputs: &'static [Stack],
        ticks: u32,
    ) -> BuildingType {
        BuildingType::Assembler(Recipe {
            inputs,
            outputs,
            ticks,
        })
    }

    /// A catalogue shaped the way the real one is: a chain that splits, and one off to the side.
    ///
    /// The ice branch runs extractor, melter, electrolyser, and the electrolyser is the step that
    /// makes two things at once. The silicon branch shares nothing with it, which is what a focus
    /// on one branch has to leave out.
    const A_CATALOGUE: [BuildingType; 5] = [
        BuildingType::Extractor(RawMaterial::Ice),
        assembler(&[stack(1, ICE)], &[stack(1, Item::Water)], 32),
        assembler(
            &[stack(1, Item::Water)],
            &[stack(2, Item::Hydrogen), stack(1, Item::Oxygen)],
            64,
        ),
        BuildingType::Extractor(RawMaterial::Silicon),
        assembler(&[stack(1, SILICON)], &[stack(1, Item::SiliconWafer)], 128),
    ];

    /// The same catalogue with one more step written into it, and nothing else touched.
    const A_LONGER_CATALOGUE: [BuildingType; 6] = [
        A_CATALOGUE[0],
        A_CATALOGUE[1],
        A_CATALOGUE[2],
        A_CATALOGUE[3],
        A_CATALOGUE[4],
        assembler(&[stack(3, Item::Hydrogen)], &[stack(1, Item::Ammonia)], 64),
    ];

    /// Where the electrolyser sits in [`A_CATALOGUE`], being the step that makes two things.
    const ELECTROLYSER: usize = 2;

    /// How many ticks a measured run of the production tick lasts, longer than several runs.
    const TICKS_MEASURED: u32 = 200;

    /// How much stock a measured run is given to work through.
    const A_SUPPLY: u32 = 6;

    fn tree_app() -> App {
        let mut app = headless_app();
        app.add_plugins(ProductionTreePlugin);
        app
    }

    fn item_at(tree: &Tree, item: Item) -> TreeNode {
        TreeNode::Item(
            tree.items
                .iter()
                .position(|held| *held == item)
                .expect("the tree holds that item"),
        )
    }

    fn chain_around(tree: &Tree, item: Item) -> Vec<TreeNode> {
        tree.chain_from(item_at(tree, item))
    }

    fn trees(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<ProductionTree>>()
            .iter(app.world())
            .count()
    }

    fn sheet_lines(app: &mut App) -> Vec<String> {
        let mut lines = Vec::new();
        for text in app.world_mut().query::<&Text>().iter(app.world()) {
            lines.push(text.0.clone());
        }
        lines
    }

    fn says(app: &mut App, wanted: &str) -> bool {
        sheet_lines(app).iter().any(|line| line.contains(wanted))
    }

    /// What every node of the tree is drawn in, which is how the focused chain reads off the sheet.
    fn node_fills(app: &mut App) -> Vec<Color> {
        app.world_mut()
            .query_filtered::<&BackgroundColor, With<DrawnNode>>()
            .iter(app.world())
            .map(|fill| fill.0)
            .collect()
    }

    fn open_the_tree(app: &mut App) {
        ask_for(app, ShowTheTree);
        tick(app);
    }

    fn step_focus(app: &mut App, by: isize) {
        ask_for(app, FocusStep(by));
        tick(app);
    }

    #[test]
    fn the_tree_holds_a_node_for_every_step_and_every_good_those_steps_move() {
        let tree = Tree::of(&A_CATALOGUE);

        assert_eq!(tree.recipes.len(), A_CATALOGUE.len());
        assert_eq!(
            tree.items,
            vec![
                ICE,
                Item::Water,
                Item::Hydrogen,
                Item::Oxygen,
                SILICON,
                Item::SiliconWafer,
            ]
        );
    }

    #[test]
    fn a_step_that_makes_two_things_is_one_node_with_both_coming_off_it() {
        let tree = Tree::of(&A_CATALOGUE);
        let splitting = TreeNode::Recipe(ELECTROLYSER);

        let made = tree
            .neighbours(splitting, Direction::Feeds)
            .collect::<Vec<TreeNode>>();

        assert_eq!(
            made,
            vec![item_at(&tree, Item::Hydrogen), item_at(&tree, Item::Oxygen)]
        );
        for product in made {
            assert_eq!(
                tree.neighbours(product, Direction::Needs)
                    .collect::<Vec<TreeNode>>(),
                vec![splitting]
            );
        }
    }

    #[test]
    fn a_step_added_to_the_catalogue_is_in_the_tree_with_no_second_edit() {
        let before = Tree::of(&A_CATALOGUE);
        let after = Tree::of(&A_LONGER_CATALOGUE);

        assert_eq!(after.recipes.len(), before.recipes.len() + 1);
        assert!(!before.items.contains(&Item::Ammonia));
        assert!(after.items.contains(&Item::Ammonia));
        assert!(after
            .neighbours(
                TreeNode::Recipe(A_LONGER_CATALOGUE.len() - 1),
                Direction::Needs
            )
            .eq([item_at(&after, Item::Hydrogen)]));
    }

    #[test]
    fn the_tree_is_drawn_from_the_catalogue_the_game_places_from() {
        let tree = Tree::of(&BuildingType::ALL);

        assert_eq!(tree.recipes, BuildingType::ALL.to_vec());
        assert!(tree.items.contains(&Item::Electronics));
    }

    #[test]
    fn focusing_an_item_selects_everything_it_needs() {
        let tree = Tree::of(&A_CATALOGUE);

        let chain = chain_around(&tree, Item::Oxygen);

        for needed in [ICE, Item::Water] {
            assert!(chain.contains(&item_at(&tree, needed)), "{needed:?}");
        }
        for step in [0, 1, ELECTROLYSER] {
            assert!(chain.contains(&TreeNode::Recipe(step)), "step {step}");
        }
    }

    #[test]
    fn focusing_an_item_selects_everything_that_needs_it() {
        let tree = Tree::of(&A_CATALOGUE);

        let chain = chain_around(&tree, ICE);

        for made in [Item::Water, Item::Hydrogen, Item::Oxygen] {
            assert!(chain.contains(&item_at(&tree, made)), "{made:?}");
        }
        assert!(chain.contains(&TreeNode::Recipe(ELECTROLYSER)));
    }

    #[test]
    fn focusing_an_item_leaves_out_what_is_neither_above_nor_below_it() {
        let tree = Tree::of(&A_CATALOGUE);

        let chain = chain_around(&tree, Item::Oxygen);

        for aside in [SILICON, Item::SiliconWafer] {
            assert!(!chain.contains(&item_at(&tree, aside)), "{aside:?}");
        }
        assert!(!chain.contains(&TreeNode::Recipe(A_CATALOGUE.len() - 1)));
    }

    #[test]
    fn a_chain_that_loops_back_on_itself_is_selected_once() {
        let tree = Tree::of(&BuildingType::ALL);

        let chain = chain_around(&tree, Item::Oxygen);

        let mut seen = chain.clone();
        seen.sort_unstable_by_key(|node| format!("{node:?}"));
        seen.dedup();
        assert_eq!(seen.len(), chain.len());
    }

    #[test]
    fn an_extractor_sits_in_the_first_column_and_a_step_past_every_good_it_takes() {
        let tree = Tree::of(&A_CATALOGUE);
        let columns = tree.columns();

        assert_eq!(columns.of(TreeNode::Recipe(0)), 0);
        for step in 0..A_CATALOGUE.len() {
            let node = TreeNode::Recipe(step);
            for needed in tree.neighbours(node, Direction::Needs) {
                assert!(columns.of(node) > columns.of(needed), "step {step}");
            }
        }
    }

    #[test]
    fn a_loop_still_leaves_every_step_to_the_right_of_what_it_takes() {
        let tree = Tree::of(&BuildingType::ALL);

        let columns = tree.columns();

        for place in 0..tree.recipes.len() {
            let node = TreeNode::Recipe(place);
            for needed in tree.neighbours(node, Direction::Needs) {
                assert!(
                    columns.of(node) > columns.of(needed),
                    "{} sits level with or left of the {} it takes",
                    tree.recipes[place].label(),
                    tree.label_of(needed)
                );
            }
        }
    }

    #[test]
    fn the_tree_goes_on_screen_and_off_again_on_the_key() {
        let mut app = tree_app();

        open_the_tree(&mut app);
        assert_eq!(trees(&mut app), 1);
        assert!(says(&mut app, HEADING), "{:?}", sheet_lines(&mut app));

        open_the_tree(&mut app);
        assert_eq!(trees(&mut app), 0);
    }

    #[test]
    fn stepping_moves_the_focus_to_the_next_item() {
        let mut app = tree_app();
        open_the_tree(&mut app);

        step_focus(&mut app, 1);

        let first = BuildingType::ALL[0].recipe().outputs[0].item;
        assert_eq!(app.world().resource::<Focus>().0, Some(0));
        assert!(
            says(&mut app, &format!("Focused on {}", first.name())),
            "{:?}",
            sheet_lines(&mut app)
        );
    }

    #[test]
    fn a_focused_tree_draws_what_is_off_the_chain_differently_from_what_is_on_it() {
        let mut app = tree_app();
        open_the_tree(&mut app);

        step_focus(&mut app, 1);

        let fills = node_fills(&mut app);
        assert!(fills.contains(&NODE_FILL[1]), "nothing is on the chain");
        assert!(fills.contains(&NODE_FILL[0]), "nothing is off the chain");
    }

    #[test]
    fn clearing_the_focus_draws_the_whole_tree_plain() {
        let mut app = tree_app();
        open_the_tree(&mut app);
        step_focus(&mut app, 1);

        ask_for(&mut app, ClearTheFocus);
        tick(&mut app);

        assert_eq!(app.world().resource::<Focus>().0, None);
        assert!(node_fills(&mut app)
            .iter()
            .all(|fill| *fill == NODE_FILL[1]));
        assert!(
            says(&mut app, NOTHING_FOCUSED),
            "{:?}",
            sheet_lines(&mut app)
        );
    }

    #[test]
    fn a_chain_makes_the_same_total_with_the_tree_open_as_with_it_shut() {
        let shut = what_a_melter_made(false);

        let open = what_a_melter_made(true);

        assert_eq!(open, shut);
        assert!(open > 0, "the melter made nothing either way");
    }

    /// How much water a melter under supply put out over [`TICKS_MEASURED`], with the tree up or
    /// not. The tree is opened and focused before the run, so every tick of it is measured with
    /// the panel on screen.
    fn what_a_melter_made(with_the_tree_up: bool) -> u32 {
        let mut app = tree_app();
        app.add_plugins((SimulationPlugin, ProductionPlugin));
        tick(&mut app);
        let melter = build(&mut app, A_CATALOGUE[1]);
        stand(&mut app, melter, Flow::Intake, ICE, A_SUPPLY);
        if with_the_tree_up {
            open_the_tree(&mut app);
            step_focus(&mut app, 1);
        }

        for _ in 0..TICKS_MEASURED {
            tick(&mut app);
        }
        stock(&app, melter, Flow::Outlet, Item::Water)
    }

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

    fn port_of(app: &App, building: Entity, flow: Flow, item: Item) -> Entity {
        let world = app.world();
        world
            .entity(building)
            .get::<Children>()
            .expect("a building stands its ports")
            .iter()
            .find(|child| world.entity(*child).get::<Port>() == Some(&Port { flow, item }))
            .expect("the building stands the port asked for")
    }

    fn stand(app: &mut App, building: Entity, flow: Flow, item: Item, quantity: u32) {
        let port = port_of(app, building, flow, item);
        app.world_mut()
            .entity_mut(port)
            .get_mut::<Holding>()
            .expect("a port holds what passes through it")
            .take_in(quantity);
    }

    fn stock(app: &App, building: Entity, flow: Flow, item: Item) -> u32 {
        app.world()
            .entity(port_of(app, building, flow, item))
            .get::<Holding>()
            .map_or(0, Holding::held)
    }
}
