use bevy::prelude::*;
pub mod cleanup;
pub mod cursor;
pub mod initialize;

pub struct CommonPlugin;

impl Plugin for CommonPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(cleanup::CleanupPlugin);
    }
}
