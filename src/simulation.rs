use bevy::prelude::*;

/// Ticks of game time in one second of real time, before any speed multiplier.
///
/// It is also the rate the camera's translation speed was settled against by play testing, while
/// the camera still ran on this clock. Nothing in gameplay measures in seconds, so moving the
/// simulation to another rate is a change to this constant and nothing else.
const TICK_RATE_HZ: f64 = 64.0;

/// The clock gameplay runs on, at a rate the rest of the app does not get to set under it.
///
/// Running the world faster means running more ticks, never longer ones: `Time<Virtual>`'s
/// relative speed multiplies how many ticks a second of real time carries while the timestep
/// stays where this put it. A chain that jams therefore jams the same way fast-forwarded as at
/// real time.
pub struct SimulationPlugin;

/// The set a gameplay system joins to run on the simulation tick, in `FixedUpdate`.
///
/// Everything a rover could observe belongs in it. Presentation — easing, smoothing, a mesh
/// catching up to the tile it stands on — stays in `Update` and out of it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Simulation;

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
            .configure_sets(FixedUpdate, Simulation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{advance, headless_app};
    use std::time::Duration;

    /// One tick of the 64 Hz clock, exactly, so hundreds of frames accumulate no float drift.
    const FRAME: Duration = Duration::from_micros(15_625);

    /// How far the stand-in rover moves in one tick.
    const STEP_PER_TICK: f32 = 0.25;

    /// The rate the app's clock is left on before the plugin is added, so a test of the rate the
    /// simulation runs at cannot pass by inheriting whatever `Time<Fixed>` already said.
    const INHERITED_TICK_RATE_HZ: f64 = 10.0;

    #[derive(Resource, Default, Debug, PartialEq)]
    struct Rover {
        ticks: u32,
        distance: f32,
    }

    fn drive_the_rover(mut rover: ResMut<Rover>) {
        rover.ticks += 1;
        rover.distance += STEP_PER_TICK;
    }

    fn simulation_app() -> App {
        let mut app = headless_app();
        app.insert_resource(Time::<Fixed>::from_hz(INHERITED_TICK_RATE_HZ))
            .add_plugins(SimulationPlugin)
            .init_resource::<Rover>()
            .add_systems(FixedUpdate, drive_the_rover.in_set(Simulation));
        advance(&mut app, FRAME);
        app
    }

    fn run_fast(app: &mut App, speed: f32) {
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(speed);
    }

    fn rover(app: &App) -> &Rover {
        app.world().resource::<Rover>()
    }

    fn ticks_over(app: &mut App, frames: u32) -> u32 {
        let before = rover(app).ticks;
        for _ in 0..frames {
            advance(app, FRAME);
        }
        rover(app).ticks - before
    }

    #[test]
    fn a_simulation_system_does_not_run_on_a_frame_that_carries_no_tick() {
        let mut app = simulation_app();

        let ticks = {
            let before = rover(&app).ticks;
            advance(&mut app, Duration::from_micros(100));
            rover(&app).ticks - before
        };

        assert_eq!(ticks, 0);
    }

    #[test]
    fn sixty_four_ticks_fill_a_second_of_real_time() {
        let mut app = simulation_app();

        assert_eq!(ticks_over(&mut app, 64), 64);
    }

    #[test]
    fn a_fast_world_runs_more_ticks_in_the_same_real_time() {
        let mut app = simulation_app();
        run_fast(&mut app, 8.0);

        assert_eq!(ticks_over(&mut app, 64), 512);
    }

    #[test]
    fn the_tick_is_the_same_length_however_fast_the_world_runs() {
        let mut app = simulation_app();
        let timestep = app.world().resource::<Time<Fixed>>().timestep();
        run_fast(&mut app, 8.0);

        ticks_over(&mut app, 64);

        assert_eq!(app.world().resource::<Time<Fixed>>().timestep(), timestep);
    }

    #[test]
    fn a_world_run_fast_reaches_the_same_state_in_less_real_time() {
        let mut real_time = simulation_app();
        let mut fast = simulation_app();
        run_fast(&mut fast, 8.0);

        let ticked = ticks_over(&mut real_time, 64 * 8);

        assert_eq!(ticks_over(&mut fast, 64), ticked);
        assert_eq!(rover(&real_time), rover(&fast));
    }
}
