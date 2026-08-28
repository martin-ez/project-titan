//! What the game puts on screen for the player to read, and the layer it is drawn with.
//!
//! Panels here are `bevy_ui` nodes styled by this project, chosen over Bevy's `bevy_feathers`:
//! Feathers is an editor and inspector widget set, aimed at a future Bevy editor's look and
//! documented as deliberately not for a game's own interface, so adopting it would buy a theme,
//! an embedded font and a material stack in exchange for a look this game does not want. Where a
//! control needs behaviour rather than a look, `bevy_ui_widgets` is the styling-free upstream it
//! comes from — `EditableText` and the rest are `bevy_ui`'s, not Feathers'.
//!
//! A panel reads the world to decide what to draw and never writes it: the only way the player
//! changes the game is the action a press already goes through (invariant 4).

pub mod legend;
