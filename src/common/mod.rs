use bevy::prelude::*;
mod cleanup;
pub mod initialize;

pub struct CommonPlugin;

impl Plugin for CommonPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(cleanup::CleanupPlugin);
    }
}
