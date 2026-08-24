use crate::legend::{Binding, BindingContext, BindingInput, DeclareBindings};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::input::InputSystems;
use bevy::prelude::*;

/// Ticks of game time in one second of real time, before any speed multiplier.
///
/// It is also the rate the camera's translation speed was settled against by play testing, while
/// the camera still ran on this clock. Nothing in gameplay measures in seconds, so moving the
/// simulation to another rate is a change to this constant and nothing else.
const TICK_RATE_HZ: f64 = 64.0;

/// The speeds the player steps through, slowest first, `0.0` stopping the world.
///
/// Four rungs topping out at 4x, settled by play testing: enough to skip a dull stretch of a
/// production chain, and not so much that a jam forms and clears between two frames.
const WARP_LADDER: [f32; 4] = [0.0, 1.0, 2.0, 4.0];

/// The rung of `WARP_LADDER` the game opens on, which is real time.
const NORMAL_WARP: usize = 1;

/// Key binding that steps the world down a rung
const WARP_SLOWER_KEY: KeyCode = KeyCode::Comma;
/// Key binding that steps the world up a rung
const WARP_FASTER_KEY: KeyCode = KeyCode::Period;

/// Ticks of game time the simulation carried through the last second of real time.
///
/// Measured rather than declared: a world asked for 4x reports fewer than 256 when the machine
/// cannot keep up, which is the difference between a warp that is working and one that is not.
pub const TICKS_PER_SECOND: DiagnosticPath = DiagnosticPath::const_new("sim/ticks_per_second");

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

/// The ticks of game time the world has run since it was started.
///
/// Counted rather than measured off a clock, so it is the same number on every machine at the
/// same point in a game. That is what lets a decision be made by the tick — which of two rovers
/// arriving at a junction at once goes first — rather than by the order the world happens to
/// store them in (invariant 2).
#[derive(Resource, Default)]
pub struct Ticks(pub u64);

/// How fast the player has asked the world to run, as a rung of `WARP_LADDER`.
#[derive(Resource)]
struct TimeWarp(usize);

/// The ticks the simulation has run since the tick rate was last measured.
#[derive(Resource, Default)]
struct TicksSinceMeasured(u32);

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE_HZ))
            .insert_resource(TimeWarp(NORMAL_WARP))
            .init_resource::<Ticks>()
            .init_resource::<TicksSinceMeasured>()
            .declare_bindings([
                Binding {
                    input: BindingInput::Key(WARP_FASTER_KEY),
                    action: "Run the world faster",
                    context: BindingContext::Always,
                },
                Binding {
                    input: BindingInput::Key(WARP_SLOWER_KEY),
                    action: "Run the world slower, down to stopped",
                    context: BindingContext::Always,
                },
            ])
            .register_diagnostic(Diagnostic::new(TICKS_PER_SECOND).with_suffix(" ticks/s"))
            .configure_sets(FixedUpdate, Simulation)
            .add_systems(FixedUpdate, count_the_tick.before(Simulation))
            .add_systems(
                PreUpdate,
                (step_the_warp, apply_the_warp).chain().after(InputSystems),
            )
            .add_systems(Update, measure_the_tick_rate);
    }
}

/// Move the player up or down the ladder, which stops at both ends.
fn step_the_warp(input: Res<ButtonInput<KeyCode>>, mut warp: ResMut<TimeWarp>) {
    let stepped = if input.just_pressed(WARP_FASTER_KEY) {
        warp.0 + 1
    } else if input.just_pressed(WARP_SLOWER_KEY) {
        warp.0.saturating_sub(1)
    } else {
        return;
    };

    warp.0 = stepped.min(WARP_LADDER.len() - 1);
}

/// Hand the rung the player is on to the clock the tick is counted off.
///
/// The speed multiplies how many ticks a second of real time carries; the timestep is untouched,
/// so the slowest rung is a world where no tick arrives rather than one taking very long ones.
fn apply_the_warp(warp: Res<TimeWarp>, mut time: ResMut<Time<Virtual>>) {
    if warp.is_changed() {
        time.set_relative_speed(WARP_LADDER[warp.0]);
    }
}

/// Count the tick before the world runs on it, so every system on it reads the same number.
fn count_the_tick(mut run: ResMut<Ticks>, mut ticks: ResMut<TicksSinceMeasured>) {
    run.0 += 1;
    ticks.0 += 1;
}

fn measure_the_tick_rate(
    mut diagnostics: Diagnostics,
    mut ticks: ResMut<TicksSinceMeasured>,
    time: Res<Time<Real>>,
) {
    let seconds = time.delta_secs_f64();
    if seconds == 0.0 {
        return;
    }

    let counted = f64::from(std::mem::take(&mut ticks.0));
    diagnostics.add_measurement(&TICKS_PER_SECOND, || counted / seconds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{advance, headless_app, press_key, release_key};
    use bevy::diagnostic::DiagnosticsStore;
    use std::time::Duration;

    /// One tick of the 64 Hz clock, exactly, so hundreds of frames accumulate no float drift.
    const FRAME: Duration = Duration::from_micros(15_625);

    /// How far the stand-in rover moves in one tick.
    const STEP_PER_TICK: f32 = 0.25;

    /// The rate the app's clock is left on before the plugin is added, so a test of the rate the
    /// simulation runs at cannot pass by inheriting whatever `Time<Fixed>` already said.
    const INHERITED_TICK_RATE_HZ: f64 = 10.0;

    #[derive(Resource, Clone, Default, Debug, PartialEq)]
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
        advanced_over(app, frames).ticks
    }

    /// How far the stand-in rover got over `frames` frames, whatever it had already done.
    fn advanced_over(app: &mut App, frames: u32) -> Rover {
        let before = rover(app).clone();
        for _ in 0..frames {
            advance(app, FRAME);
        }
        let after = rover(app);
        Rover {
            ticks: after.ticks - before.ticks,
            distance: after.distance - before.distance,
        }
    }

    /// Step the warp `rungs` rungs with `key`, letting each press be seen and then let go.
    fn press_warp(app: &mut App, key: KeyCode, rungs: u32) {
        for _ in 0..rungs {
            press_key(app, key);
            advance(app, FRAME);
            release_key(app, key);
            advance(app, FRAME);
        }
    }

    fn ticks_a_second(app: &App) -> Option<f64> {
        app.world()
            .resource::<DiagnosticsStore>()
            .get(&TICKS_PER_SECOND)?
            .value()
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

    #[test]
    fn warping_up_runs_more_ticks_in_the_same_real_time() {
        let mut app = simulation_app();

        press_warp(&mut app, WARP_FASTER_KEY, 1);

        assert_eq!(ticks_over(&mut app, 64), 128);
    }

    #[test]
    fn warping_up_past_the_fastest_rung_leaves_the_world_there() {
        let mut app = simulation_app();

        press_warp(&mut app, WARP_FASTER_KEY, 5);

        assert_eq!(ticks_over(&mut app, 64), 256);
    }

    #[test]
    fn warping_down_from_real_time_stops_the_world() {
        let mut app = simulation_app();

        press_warp(&mut app, WARP_SLOWER_KEY, 1);

        assert_eq!(ticks_over(&mut app, 64), 0);
    }

    #[test]
    fn warping_up_from_a_stopped_world_starts_it_again_at_real_time() {
        let mut app = simulation_app();
        press_warp(&mut app, WARP_SLOWER_KEY, 3);

        press_warp(&mut app, WARP_FASTER_KEY, 1);

        assert_eq!(ticks_over(&mut app, 64), 64);
    }

    #[test]
    fn the_tick_is_the_same_length_at_every_rung_of_the_ladder() {
        let mut app = simulation_app();
        let timestep = app.world().resource::<Time<Fixed>>().timestep();

        press_warp(&mut app, WARP_FASTER_KEY, 2);
        ticks_over(&mut app, 64);

        assert_eq!(app.world().resource::<Time<Fixed>>().timestep(), timestep);
    }

    #[test]
    fn a_warped_world_gets_as_far_as_one_that_ran_four_times_as_long() {
        let mut real_time = simulation_app();
        let mut warped = simulation_app();
        press_warp(&mut warped, WARP_FASTER_KEY, 2);

        let ran = advanced_over(&mut real_time, 64 * 4);

        assert_eq!(advanced_over(&mut warped, 64), ran);
        assert_eq!(ran.ticks, 256);
    }

    #[test]
    fn the_tick_rate_reports_the_ticks_a_second_of_real_time_carried() {
        let mut app = simulation_app();

        advance(&mut app, FRAME);

        assert_eq!(ticks_a_second(&app), Some(TICK_RATE_HZ));
    }

    #[test]
    fn the_tick_rate_doubles_when_the_world_runs_twice_as_fast() {
        let mut app = simulation_app();

        press_warp(&mut app, WARP_FASTER_KEY, 1);

        assert_eq!(ticks_a_second(&app), Some(TICK_RATE_HZ * 2.0));
    }

    #[test]
    fn a_stopped_world_reports_no_ticks_a_second() {
        let mut app = simulation_app();

        press_warp(&mut app, WARP_SLOWER_KEY, 1);

        assert_eq!(ticks_a_second(&app), Some(0.0));
    }

    #[test]
    fn the_world_counts_every_tick_it_has_run() {
        let mut app = simulation_app();
        let before = app.world().resource::<Ticks>().0;

        ticks_over(&mut app, 4);

        assert_eq!(app.world().resource::<Ticks>().0, before + 4);
    }

    #[test]
    fn a_stopped_world_counts_no_tick() {
        let mut app = simulation_app();
        press_warp(&mut app, WARP_SLOWER_KEY, 1);
        let before = app.world().resource::<Ticks>().0;

        ticks_over(&mut app, 4);

        assert_eq!(app.world().resource::<Ticks>().0, before);
    }
}
