use bevy::prelude::*;
mod cleanup;
mod initialize;

pub use cleanup::{Destroy, DestroyOnStateChange};
pub use initialize::{initialize_system, Initialize, NeedsInitialization};

pub struct CommonPlugin;

impl Plugin for CommonPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(cleanup::CleanupPlugin);
    }
}
