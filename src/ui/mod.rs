//! What the game puts on screen for the player to read, and the layer it is drawn with.
//!
//! Panels here are `bevy_ui` nodes styled by this project, chosen over Bevy's `bevy_feathers`:
//! Feathers is an editor and inspector widget set, aimed at a future Bevy editor's look and
//! documented as deliberately not for a game's own interface, so adopting it would buy a theme,
//! an embedded font and a material stack in exchange for a look this game does not want. Where a
//! control needs behaviour rather than a look, `bevy_ui_widgets` is the styling-free upstream it
//! comes from — `EditableText` and the rest are `bevy_ui`'s, not Feathers'.
//!
//! A panel that reads the game out never writes it: the only way the player changes the game is
//! the action a press already goes through (invariant 4). A settings panel is the one that does
//! write, and only ever the settings resource it is an editor of — a sensitivity is a setting on
//! the player's own input, not a fact a rover could observe. What every panel is drawn in is
//! here, so two of them cannot drift into two looks.

use bevy::prelude::*;

pub mod building_panel;
pub mod legend;
pub mod production_tree;
pub mod selection;
pub mod settings_panel;

/// How far a panel sits from the corner of the screen it is pinned to, in logical pixels
const PANEL_INSET: f32 = 8.0;
/// How much space a panel keeps between its edge and its rows, in logical pixels
const PANEL_PADDING: f32 = 10.0;
/// How round a panel's corners are, in logical pixels
const PANEL_RADIUS: f32 = 4.0;
/// What a panel is drawn on, dark enough to read text over whatever the world puts behind it
const PANEL_BACKGROUND: Color = Color::srgba(0.04, 0.04, 0.06, 0.85);
/// What a full-screen sheet is drawn on, darker than a panel because it covers the game rather
/// than sitting in a corner of it
const SHEET_BACKGROUND: Color = Color::srgba(0.03, 0.03, 0.05, 0.96);
/// How much space sits between one row of a panel and the next, in logical pixels
const ROW_GAP: f32 = 2.0;
/// How large a panel's text is, in logical pixels
const TEXT_SIZE: f32 = 12.0;

/// The colour a heading is written in
pub const HEADING_TEXT: Color = Color::srgb(0.62, 0.78, 0.98);
/// The colour the part of a row the player presses a key to change is written in
pub const KEYED_TEXT: Color = Color::srgb(0.98, 0.90, 0.66);
/// The colour the body of a row is written in
pub const BODY_TEXT: Color = Color::srgb(0.86, 0.87, 0.90);

/// Which panel this is, carried by the panel while it is on screen.
///
/// A binding that only answers while its panel is up says which one it belongs to, and the legend
/// answers that by looking for the panel rather than by being told a panel opened. One list of
/// what is on screen, kept by the panels themselves, cannot fall out of step with a second.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    /// The legend of commands.
    Legend,
    /// The camera settings the player edits.
    Settings,
    /// The reading of whatever the player picked out.
    Building,
    /// The tree of everything the game can make.
    ProductionTree,
}

/// The corner of the screen a panel is pinned to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelCorner {
    /// Against the top and left edges.
    TopLeft,
    /// Against the top and right edges.
    TopRight,
    /// Against the bottom and right edges.
    BottomRight,
}

/// The frame `which` panel is drawn in: pinned to `corner`, `width` wide, stacking rows downward.
pub fn panel(which: Panel, corner: PanelCorner, width: Val) -> impl Bundle {
    let inset = Val::Px(PANEL_INSET);
    let (top, bottom, left, right) = match corner {
        PanelCorner::TopLeft => (inset, Val::Auto, inset, Val::Auto),
        PanelCorner::TopRight => (inset, Val::Auto, Val::Auto, inset),
        PanelCorner::BottomRight => (Val::Auto, inset, Val::Auto, inset),
    };
    (
        which,
        Node {
            position_type: PositionType::Absolute,
            top,
            bottom,
            left,
            right,
            width,
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(PANEL_PADDING)),
            row_gap: Val::Px(ROW_GAP),
            border_radius: BorderRadius::all(Val::Px(PANEL_RADIUS)),
            ..default()
        },
        BackgroundColor(PANEL_BACKGROUND),
    )
}

/// The full-screen sheet `which` panel is drawn on, for one too large to pin to a corner.
///
/// It stacks downward like a panel, so a heading sits above whatever the panel lays out under it.
pub fn overlay(which: Panel) -> impl Bundle {
    (
        which,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(PANEL_PADDING)),
            row_gap: Val::Px(ROW_GAP),
            ..default()
        },
        BackgroundColor(SHEET_BACKGROUND),
    )
}

/// One piece of text on a panel, in `colour` and taking `width` of the row it sits in.
pub fn panel_text(text: String, colour: Color, width: Val) -> impl Bundle {
    (
        Node { width, ..default() },
        Text(text),
        panel_font(),
        TextColor(colour),
    )
}

/// The type every panel is set in, for a caller composing a node `panel_text` cannot.
pub fn panel_font() -> TextFont {
    TextFont {
        font_size: FontSize::Px(TEXT_SIZE),
        ..default()
    }
}

/// A row of a panel, laying its cells out left to right.
pub fn panel_row() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        ..default()
    }
}
