use bevy::app::AppExit;
use bevy::ecs::change_detection::{DetectChanges, DetectChangesMut};
use bevy::ecs::entity::{Entity, EntityHashMap};
use bevy::ecs::hierarchy::{ChildOf, Children};
use bevy::ecs::message::{MessageReader, MessageWriter};
use bevy::ecs::query::{Added, Changed, Has, Or, With, Without};
use bevy::ecs::system::{
    Commands, Local, NonSend, NonSendMut, Populated, Query, Res, ResMut, Single,
};
use bevy::math::IRect;
use bevy::tasks::AsyncComputeTaskPool;
use bevy::tasks::futures_lite::future;
use bevy::time::Time;
use objc2_foundation::NSPoint;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tracing::{Level, debug, error, info, instrument, trace, warn};

use super::{
    ActiveDisplayMarker, BProcess, ExistingMarker, FreshMarker, RepositionMarker, ResizeMarker,
    RetryFrontSwitch, SpawnWindowTrigger, Timeout, VerifyResize, VerifyWindowPosition,
};

use crate::config::{Config, decorations::BorderRadiusOption};
use crate::ecs::display::FloatingLayer;
use crate::ecs::layout::{Column, LayoutStrip};
use crate::ecs::params::{ActiveDisplay, FrameActivity, Windows};
use crate::ecs::{
    ActiveWorkspaceMarker, Bounds, BruteforceWindows, FlashMessage, FocusedMarker, Initializing,
    LowPowerMode, MissionControlActive, Position, ReadDisplayProperties, RestoreWindowState,
    Scrolling, SendMessageTrigger, SpawnCommandsExt, SystemTheme, Unmanaged, WidthRatio,
    WindowProperties,
};
use crate::events::{Event, InputEvent};
use crate::manager::{
    Application, Display, Process, Window, WindowManager, WindowOS, bruteforce_windows,
};
use crate::overlay::{FlashMessageManager, OverlayManager};
use crate::platform::input::TapHealth;
use crate::platform::{PlatformCallbacks, WinID};

/// Processes and applications still inside their spawn grace period, with the
/// `FreshMarker` that says whether the spawn actually completed in time.
type TimedOutSpawns<'w, 's> = Populated<
    'w,
    's,
    (Entity, Has<FreshMarker>, &'static Timeout),
    Or<(With<BProcess>, With<Application>)>,
>;

/// Windows as [`window_moved_update_frame`] sees them: the element to re-read,
/// the origin to update, and the marker saying we are the ones moving it.
type MovableWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Window,
        &'static mut Position,
        &'static Bounds,
        Option<&'static Unmanaged>,
        Has<RepositionMarker>,
    ),
    Without<LayoutStrip>,
>;

/// Windows as the resize handler rewrites them: the OS handle to re-read the
/// frame from, the size to overwrite, and whether the window is ours to lay
/// out at all.
type ResizableWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Window,
        Entity,
        &'static Position,
        &'static mut Bounds,
        Option<&'static Unmanaged>,
        Has<ResizeMarker>,
    ),
    Without<LayoutStrip>,
>;

const ANIAMTE_SNAP_THRESHOLD: f32 = 5.0;
const LOOP_MAX_TIMEOUT_FRAME_ACTIVE_MS: u32 = 16;
const LOOP_MAX_TIMEOUT_LOWPOWER_MS: u32 = 2000;
// Real events (input, IPC, workspace changes, ...) wake the pump immediately
// via `EventLoopWaker`, so this only bounds how late the free-running 1s
// `on_timer` systems (`recover_lost_focus`, workspace refresh) can land, and
// how long a dead event tap can go unnoticed between the 30s health sweeps.
// Kept well under both: with no genuine work to do, this used to run the
// whole schedule 20 times a second.
const LOOP_MAX_TIMEOUT_MS: u32 = 500;
const TAP_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const LOOP_TIMEOUT_STEP: u32 = 1;

/// How long [`pump_events`] may spend draining the incoming channel before it
/// has to hand the frame back, and how many events it may take in one go.
///
/// The drain previously ran until a full millisecond passed with nothing
/// arriving, a condition a sustained burst (drag, resize animation, a churning
/// app) never satisfies — the loop never exited and the window manager stopped
/// responding until the burst let up. Nothing is dropped when a cap is hit:
/// leftover events stay in the channel for the next frame to pick up.
const PUMP_BUDGET: Duration = Duration::from_millis(4);
const PUMP_MAX_EVENTS: usize = 256;

/// Gathers all present displays and spawns them as entities in the Bevy world.
/// The currently active display (identified by `window_manager.active_display_id()`) is marked with `ActiveDisplayMarker`.
///
/// # Arguments
///
/// * `window_manager` - The `WindowManager` resource for querying display information.
/// * `commands` - Bevy commands to spawn entities.
pub fn gather_displays(window_manager: Res<WindowManager>, mut commands: Commands) {
    let Ok(active_display_id) = window_manager.active_display_id() else {
        error!("Unable to get active display id!");
        return;
    };
    for (display, workspaces) in window_manager.present_displays() {
        let origin = Position(display.bounds().min);
        let entity = if display.id() == active_display_id {
            commands.spawn((display, ActiveDisplayMarker))
        } else {
            commands.spawn(display)
        }
        .id();

        commands.trigger(ReadDisplayProperties(entity));

        let Ok(active_space) = window_manager.active_display_space(active_display_id) else {
            return;
        };

        for id in workspaces {
            let active = id == active_space;
            commands.spawn_layout_strip(LayoutStrip::new(id, 0), origin.0, entity, active);
            commands.spawn((FloatingLayer::new(id), ChildOf(entity)));
        }
    }
}

/// Pre-creates additional (empty) virtual workspaces on every physical space,
/// so that `config.default_workspaces()` virtual workspaces exist right after
/// startup instead of only being created on first use.
///
/// Must run after [`gather_displays`], which spawns the `virtual_index: 0`
/// strip for every physical space.
pub fn initialise_workspaces(
    strips: Query<(&LayoutStrip, &ChildOf, &Position)>,
    config: Res<Config>,
    mut commands: Commands,
) {
    let wanted = config.default_workspaces();
    if wanted <= 1 {
        return;
    }

    let mut seen = HashSet::new();
    for (strip, child_of, origin) in &strips {
        if strip.virtual_index != 0 || !seen.insert((strip.id(), child_of.parent())) {
            continue;
        }
        for virtual_index in 1..wanted {
            commands.spawn_layout_strip(
                LayoutStrip::new(strip.id(), virtual_index),
                origin.0,
                child_of.parent(),
                false,
            );
        }
    }
}

/// Adds an existing process to the window manager. This is used during initial setup for already running applications.
/// It attempts to create a new `Application` instance from the `BProcess` and attaches it as a child entity.
/// The `ExistingMarker` is then removed from the process entity.
///
/// # Arguments
///
/// * `window_manager` - The `WindowManager` resource for creating new application instances.
/// * `process_query` - A query for existing `BProcess` entities marked with `ExistingMarker`.
/// * `commands` - Bevy commands to spawn entities and manage components.
#[instrument(level = Level::DEBUG, skip_all)]
pub(crate) fn add_existing_process(
    window_manager: Res<WindowManager>,
    processes: Populated<(Entity, &BProcess), With<ExistingMarker>>,
    mut commands: Commands,
) {
    for (entity, process) in processes {
        let Ok(app) = window_manager.new_application(&*process.0) else {
            error!("creating aplication from process '{}'", process.name());
            return;
        };
        commands.spawn((app, ExistingMarker, ChildOf(entity)));
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.try_remove::<ExistingMarker>();
        }
    }
}

/// Adds an existing application to the window manager. This is used during initial setup.
/// It observes the application, adds its windows to the manager, and then triggers `SpawnWindowTrigger` events for newly found windows.
/// The `ExistingMarker` is removed from the application entity after processing.
///
/// # Arguments
///
/// * `window_manager` - The `WindowManager` resource for interacting with window management logic.
/// * `displays` - A query for all `Display` entities, used to gather all existing space IDs.
/// * `app_query` - A query for existing `Application` entities marked with `ExistingMarker`.
/// * `commands` - Bevy commands to spawn entities and manage components.
#[instrument(level = Level::DEBUG, skip_all)]
pub(crate) fn add_existing_application(
    window_manager: Res<WindowManager>,
    workspaces: Query<&LayoutStrip>,
    fresh_apps: Populated<(&mut Application, Entity), With<ExistingMarker>>,
    config: Res<Config>,
    mut commands: Commands,
) {
    let spaces = workspaces
        .into_iter()
        .map(LayoutStrip::id)
        .collect::<Vec<_>>();
    let thread_pool = AsyncComputeTaskPool::get();

    for (mut app, entity) in fresh_apps {
        let mut offscreen_windows = vec![];

        if app.observe().is_ok_and(|result| result)
            && let Ok((found_windows, offscreen)) = window_manager
                .find_existing_application_windows(&mut app, &spaces, &config)
                .inspect_err(|err| warn!("{err}"))
        {
            offscreen_windows.extend(offscreen);
            commands.trigger(SpawnWindowTrigger(found_windows));
        }
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.try_remove::<ExistingMarker>();
        }

        if !offscreen_windows.is_empty() {
            let pid = app.pid();
            let bundle_id = app.bundle_id();
            let config = config.clone();
            let bruteforce_task = thread_pool.spawn(async move {
                bruteforce_windows(pid, bundle_id.as_deref(), offscreen_windows, &config)
            });
            commands.spawn(BruteforceWindows(bruteforce_task));
        }
    }
}

/// Finishes the initialization process once all initial windows are loaded.
/// This system refreshes displays, assigns the `FocusedMarker` to the first window of the active space,
/// and logs the total number of managed windows.
///
/// # Arguments
///
/// * `windows` - A mutable query for all `Window` components, their `Entity`, and `Has<Unmanaged>` status.
/// * `displays` - A query for all `Display` entities, including whether they have the `ActiveDisplayMarker`.
/// * `window_manager` - The `WindowManager` resource for refreshing displays and getting active space information.
/// * `commands` - Bevy commands to insert components like `FocusedMarker`.
#[instrument(level = Level::DEBUG, skip_all)]
pub(crate) fn finish_setup(
    process_query: Query<Entity, With<ExistingMarker>>,
    windows: Windows,
    applications: Query<&Application>,
    mut bruteforce_tasks: Query<(Entity, &mut BruteforceWindows)>,
    mut workspaces: Query<(&mut LayoutStrip, Has<ActiveWorkspaceMarker>, &ChildOf)>,
    window_manager: Res<WindowManager>,
    mut commands: Commands,
) {
    if !process_query.is_empty() {
        // The other two add_* functions are still running..
        return;
    }

    // Reap the bruteforced windows.
    if !bruteforce_tasks.is_empty() {
        for (entity, mut job) in &mut bruteforce_tasks {
            if let Some(found_windows) = future::block_on(future::poll_once(&mut job.0)) {
                commands.trigger(SpawnWindowTrigger(found_windows));
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.try_despawn();
                }
            }
        }
        // Wait for the next tick to finish initialization.
        return;
    }

    info!(
        "Initialization: found {:?} windows.",
        windows.iter().size_hint()
    );

    let mut focused_managed_window = false;
    for (mut strip, active_strip, _) in &mut workspaces {
        debug!("space {}: before refresh {strip:?}", strip.id());
        let workspace_windows = window_manager
            .windows_in_workspace(strip.id())
            .inspect_err(|err| {
                warn!("failed to get windows on workspace {}: {err}", strip.id());
            })
            .ok()
            .map(|workspace_windows| {
                workspace_windows
                    .into_iter()
                    .filter_map(|window_id| windows.find_managed(window_id))
                    .filter(|(window, entity)| {
                        if window.is_minimized() {
                            if let Ok(mut entity_commands) = commands.get_entity(*entity) {
                                entity_commands.try_insert(Unmanaged::Minimized);
                            }
                            false
                        } else {
                            true
                        }
                    })
                    .collect::<Vec<_>>()
            });
        let Some(workspace_windows) = workspace_windows else {
            continue;
        };

        // Preserve the order - do not flush existing windows.
        for entity in strip.all_windows() {
            if !workspace_windows.iter().any(|(_, e)| *e == entity) {
                strip.remove(entity);
            }
        }
        for (_, entity) in workspace_windows {
            if !strip.contains(entity) {
                strip.append(entity);
            }
        }
        debug!("space {}: after refresh {strip:?}", strip.id());

        if active_strip && let Some(entity) = strip.first().ok().and_then(|column| column.top()) {
            commands.focus_entity(entity, true);
            focused_managed_window = true;
        }
    }

    // An all-floating workspace has no strip member to receive the initial
    // focus marker. Mirror the frontmost app's AX focus so menu actions such as
    // Toggle Managed work immediately after launch.
    if !focused_managed_window
        && let Some(focused_window_id) = applications
            .iter()
            .find(|app| app.is_frontmost())
            .and_then(|app| app.focused_window_id().ok())
        && let Some((_, entity)) = windows.find(focused_window_id)
        && let Ok(mut entity_commands) = commands.get_entity(entity)
    {
        entity_commands.try_insert(FocusedMarker);
    }

    commands.remove_resource::<Initializing>();
    commands.trigger(RestoreWindowState);
}

/// Handles the event when a new application is launched. It creates a `Process` and `Application` object,
/// observes the application for events, and adds its windows to the manager.
/// This system processes `BProcess` entities marked with `FreshMarker`.
/// If the process is not yet ready, it continues observing it. If ready, it attempts to create and observe an `Application`.
/// A `Timeout` is added to the application if it takes too long to become observable.
///
/// # Arguments
///
/// * `window_manager` - The `WindowManager` resource for creating new application instances.
/// * `process_query` - A `Populated` query for `(Entity, &mut BProcess, Has<Children>)` with `With<FreshMarker>`.
/// * `commands` - Bevy commands to spawn entities and manage components.
pub(super) fn add_launched_process(
    window_manager: Res<WindowManager>,
    fresh_processes: Populated<(Entity, &mut BProcess, Has<Children>), With<FreshMarker>>,
    config: Res<Config>,
    mut commands: Commands,
) {
    const APP_OBSERVABLE_TIMEOUT: Duration = Duration::from_secs(10);
    let mut already_seen = HashSet::new();

    for (entity, mut process, children) in fresh_processes {
        let process = &mut *process.0;

        if !already_seen.insert(process.psn()) {
            continue;
        }

        if config.should_force_manage_process(process) {
            debug!(
                "Forcing management of launched process '{}' despite unobservable policy.",
                process.name()
            );
            process.force_manage(true);
        }

        if !process.ready() {
            continue;
        }

        if children {
            // Process already has an attached Application, so finish.
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<FreshMarker>();
            }
            continue;
        }

        let Ok(mut app) = window_manager.new_application(process) else {
            error!("creating aplication from process '{}'", process.name());
            return;
        };

        if app.observe().is_ok_and(|good| good) {
            let timeout = Timeout::new(
                APP_OBSERVABLE_TIMEOUT,
                Some(format!(
                    "{app} did not become observable in {}s.",
                    APP_OBSERVABLE_TIMEOUT.as_secs()
                )),
                &mut commands,
            );
            commands.spawn((app, FreshMarker, timeout, ChildOf(entity)));
        } else {
            debug!("failed to register some observers {}", process.name());
        }
    }
}

/// Adds windows for a newly launched application.
/// This system processes `Application` entities marked with `FreshMarker`.
/// It queries the application's window list, filters out already existing windows, and triggers `SpawnWindowTrigger` events for new windows.
/// The `FreshMarker` is removed from the application entity after processing.
///
/// # Arguments
///
/// * `app_query` - A `Populated` query for `(&mut Application, Entity)` with `With<FreshMarker>`.
/// * `windows` - A query for all `Window` components, used to check for existing windows.
/// * `commands` - Bevy commands to spawn entities and manage components.
pub(super) fn add_launched_application(
    app_query: Populated<(&mut Application, Entity, Has<Children>), With<FreshMarker>>,
    windows: Windows,
    config: Res<Config>,
    mut commands: Commands,
) {
    // TODO: maybe refactor this with add_existing_application_windows()
    let find_window = |window_id| windows.find(window_id);

    for (app, entity, has_children) in app_query {
        let mut create_windows = app.window_list(&config);
        // Retain the non-existing windows, so they can be created.
        create_windows.retain(|window| find_window(window.id()).is_none());

        if !create_windows.is_empty() {
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<FreshMarker>();
            }
            debug!(
                "spawn! (polling path found {} new windows for {entity})",
                create_windows.len(),
            );
            commands.trigger(SpawnWindowTrigger(create_windows));
        } else if has_children {
            // Windows were already created via AXCreated notification path.
            // Remove FreshMarker so the Timeout gets cleaned up.
            debug!("removing FreshMarker from {entity}: windows already created via AXCreated");
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<FreshMarker>();
            }
        }
    }
}

/// Cleans up entities which have been initializing for too long, specifically `BProcess` or `Application` entities.
/// This system removes the `Timeout` component from entities that are no longer `Fresh`.
///
/// This can be processes which are not yet observable or applications which keep failing to
/// register some of the observers.
///
/// # Arguments
///
/// * `cleanup` - A `Populated` query for `(Entity, Has<FreshMarker>, &Timeout)` components, targeting `BProcess` or `Application` entities.
/// * `commands` - Bevy commands to remove components.
pub(super) fn fresh_marker_cleanup(cleanup: TimedOutSpawns, mut commands: Commands) {
    for (entity, fresh, _) in cleanup {
        if !fresh && let Ok(mut entity_commands) = commands.get_entity(entity) {
            // Process was ready before the timer finished.
            entity_commands.try_remove::<Timeout>();
        }
    }
}

/// A Bevy system that ticks `Timeout` timers and despawns entities when their timers finish.
/// This system is responsible for cleaning up entities that have exceeded their allotted time for an operation.
///
/// # Arguments
///
/// * `timers` - A `Populated` query for `(Entity, &mut Timeout)` components.
/// * `clock` - The Bevy `Time` resource for getting the delta time.
/// * `commands` - Bevy commands to despawn entities.
pub(super) fn timeout_ticker(
    timers: Populated<(Entity, &mut Timeout)>,
    clock: Res<Time>,
    mut commands: Commands,
) {
    for (entity, mut timeout) in timers {
        if timeout.timer.is_finished() {
            if let Some(system_id) = timeout.system_id.take() {
                commands.run_system(system_id);
                commands.unregister_system(system_id);
            }
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                if let Some(on_expire) = timeout.on_expire {
                    trace!("Expiring component timer on {entity}.");
                    on_expire(&mut entity_commands);
                } else {
                    trace!("Despawning entity {entity} due to timeout.");
                    entity_commands.try_despawn();
                }
            }
        } else {
            timeout.timer.tick(clock.delta());
        }
    }
}

/// Retries querying the focused window for applications that had a transient AX error
/// during `ApplicationFrontSwitched`. Runs each frame until success or timeout.
pub(super) fn retry_front_switch(
    retries: Populated<(Entity, &RetryFrontSwitch)>,
    applications: Query<&Application>,
    mut commands: Commands,
) {
    for (entity, retry) in retries.iter() {
        let Ok(app) = applications.get(retry.0) else {
            // Application entity no longer exists, clean up.
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_despawn();
            }
            continue;
        };
        if !app.is_frontmost() {
            // App is no longer frontmost — this retry is stale.
            debug!("Discarding stale front switch retry (app no longer frontmost).");
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_despawn();
            }
            continue;
        }
        if let Ok(focused_id) = app.focused_window_id() {
            debug!("Front switch retry succeeded for window {focused_id}.");
            commands.trigger(SendMessageTrigger(Event::WindowFocused {
                window_id: focused_id,
            }));
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_despawn();
            }
        }
        // Otherwise, let timeout_ticker handle expiry.
    }
}

/// Upper bound on the timestep fed into [`ease_out_factor`].
///
/// When the event loop wakes from an idle sleep (up to 500ms, or 2s in low
/// power mode), `Time::delta_secs_f64()` reports the entire sleep duration on
/// the wakeup frame. Feeding that straight into the exponential decay would
/// consume almost the entire distance on Frame 0 and turn the animation into a
/// single-frame jump. Cap each frame's step to a 30fps tick (matching the
/// scroll integrator's `MAX_STEP_SECS`) so the wakeup frame steps smoothly.
const MAX_ANIMATION_STEP_SECS: f64 = 1.0 / 30.0;

/// Animates window movement.
/// Fraction of the remaining distance an exponential ease-out consumes in a
/// frame, given a decay `rate` (per second) and the frame's `delta` in seconds.
/// Shared by the reposition and resize animators so the two never drift out of step.
#[allow(
    clippy::cast_possible_truncation,
    reason = "clamped to [0, 1], well within f32's range; only sub-pixel precision is lost"
)]
fn ease_out_factor(rate: f64, delta: f64) -> f32 {
    let delta = delta.min(MAX_ANIMATION_STEP_SECS);
    (1.0 - (-rate * delta).exp()).clamp(0.0, 1.0) as f32
}

/// Paces window animation to the display's refresh rate.
///
/// Every reposition we make echoes back as a window-moved event, and any event
/// drops the pump's wait to 1 ms, so an unpaced animation feeds itself: it ran
/// at ~370 steps a second, sending each moving window a new position six times
/// per screen frame. The apps applied that flood late and unevenly, which read
/// as stutter when sliding the row across several apps. Steps now advance at
/// most once per display frame; input is still handled on every loop turn.
#[derive(bevy::ecs::resource::Resource, Default)]
pub(crate) struct AnimationClock {
    /// `Time::elapsed()` at the last step (engine time, so tests can drive it).
    last_step: Option<Duration>,
    interval: Duration,
    /// Seconds to advance this frame, or `None` when no step is due.
    step: Option<f64>,
}

impl AnimationClock {
    /// Decides whether a step is due at `now`; returns the seconds to advance.
    fn tick(&mut self, now: Duration, refresh_hz: impl FnOnce() -> f64) -> Option<f64> {
        let step = match self.last_step {
            None => {
                // First frame of an animation: move right away by one frame.
                self.interval = Duration::from_secs_f64(1.0 / refresh_hz().clamp(30.0, 240.0));
                Some(self.interval.as_secs_f64())
            }
            // Allow a quarter-frame of jitter so a 120 Hz display isn't
            // stepped at 60 Hz when the loop wakes a hair early.
            Some(last) if now.saturating_sub(last) >= self.interval.mul_f64(0.75) => {
                Some(now.saturating_sub(last).as_secs_f64())
            }
            Some(_) => None,
        };
        if step.is_some() {
            self.last_step = Some(now);
        }
        self.step = step;
        step
    }

    fn reset(&mut self) {
        self.last_step = None;
        self.step = None;
    }
}

fn display_refresh_hz() -> f64 {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return 60.0;
    };
    objc2_app_kit::NSScreen::mainScreen(mtm)
        .and_then(|screen| u32::try_from(screen.maximumFramesPerSecond()).ok())
        .map_or(60.0, f64::from)
}

type Animating = Or<(With<RepositionMarker>, With<ResizeMarker>)>;

/// Runs before the animators: decides whether this loop turn draws a frame.
pub(super) fn tick_animation_clock(
    mut clock: ResMut<AnimationClock>,
    time: Res<Time>,
    moving: Query<(), Animating>,
) {
    if moving.is_empty() {
        clock.reset();
    } else {
        clock.tick(time.elapsed(), display_refresh_hz);
    }
}

/// This is a Bevy system that runs on `Update`. It smoothly moves windows to their target
/// positions, as indicated by the `RepositionMarker` component.
/// Animation speed is controlled by the `animation_speed` in the `Config`.
/// When a window reaches its target position, the `RepositionMarker` is removed.
///
/// # Arguments
///
/// * `windows` - A `Populated` query for `(&mut Window, Entity, &RepositionMarker)` components.
/// * `displays` - A query for all `Display` entities, used to get display bounds and menubar height.
/// * `time` - The Bevy `Time` resource for calculating delta time.
/// * `config` - The `Config` resource, used for animation speed.
/// * `commands` - Bevy commands to remove the `RepositionMarker` when animation is complete.
#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn animate_entities(
    animate: Populated<(&mut Position, Entity, &RepositionMarker)>,
    clock: Res<AnimationClock>,
    config: Res<Config>,
    mut commands: Commands,
) {
    let Some(delta) = clock.step else { return };
    // Frame-rate-independent exponential smoothing (ease-out).
    // `animation_speed` is the decay rate (per second); higher = snappier.
    let t = ease_out_factor(config.animation_speed(), delta);

    animate
        .into_iter()
        .for_each(|(mut position, entity, RepositionMarker(origin))| {
            let target = origin.as_vec2();
            let current = position.0.as_vec2();
            let lerped = current.lerp(target, t);

            // Snap once we're within a pixel of the target (or after one effectively-
            // complete tick), so the marker is dropped promptly.
            let finished = (target - lerped).length() <= ANIAMTE_SNAP_THRESHOLD;
            let new_pos = if finished {
                *origin
            } else {
                lerped.round().as_ivec2()
            };

            trace!(
                "entity {entity} source {} dest {origin} t {t:.3} moving to {new_pos}",
                position.0,
            );
            position.0 = new_pos;
            if finished && let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<RepositionMarker>();
            }
        });
}

/// Animates window resizing.
/// This is a Bevy system that runs on `Update`. It resizes windows to their target
/// dimensions, as indicated by the `ResizeMarker` component.
/// When a window reaches its target size, the `ResizeMarker` is removed.
///
/// # Arguments
///
/// * `windows` - A `Populated` query for `(&mut Window, Entity, &ResizeMarker)` components.
/// * `active_display` - An `ActiveDisplay` system parameter providing immutable access to the active display.
/// * `commands` - Bevy commands to remove the `ResizeMarker` when resizing is complete.
#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn animate_resize_entities(
    animate: Populated<(&mut Bounds, Entity, &ResizeMarker)>,
    clock: Res<AnimationClock>,
    config: Res<Config>,
    mut commands: Commands,
) {
    let Some(delta) = clock.step else { return };
    // Matches animate_entities: exponential ease-out, frame-rate independent.
    let t = ease_out_factor(config.animation_speed(), delta);

    animate
        .into_iter()
        .for_each(|(mut bounds, entity, ResizeMarker(size))| {
            let target = size.as_vec2();
            let current = bounds.0.as_vec2();
            let lerped = current.lerp(target, t);

            let finished = (target - lerped).length() <= ANIAMTE_SNAP_THRESHOLD;
            let new_size = if finished {
                *size
            } else {
                lerped.round().as_ivec2()
            };

            trace!(
                "entity {entity} source {} dest {size} t {t:.3} resizing to {new_size}",
                bounds.0,
            );
            bounds.0 = new_size;
            if finished && let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<ResizeMarker>();
                entity_commands.try_insert(VerifyResize(VERIFY_RESIZE_ATTEMPTS));
            }
        });
}

/// How many times [`verify_window_size`] re-applies a size the app ignored.
const VERIFY_RESIZE_ATTEMPTS: u8 = 3;

/// After an animated resize, re-reads the window's real size and applies the
/// wanted one again if the app did not take it, a few times at most.
#[instrument(level = Level::DEBUG, skip_all)]
pub(super) fn verify_window_size(
    windows: Populated<(Entity, &mut Window, &Bounds, &mut VerifyResize), Without<ResizeMarker>>,
    mut commands: Commands,
) {
    for (entity, mut window, Bounds(want), mut verify) in windows {
        if let Ok(frame) = window.update_frame() {
            let got = frame.size();
            if got.chebyshev_distance(*want) <= 1 {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.try_remove::<VerifyResize>();
                }
                continue;
            }
            debug!(
                "window {} is {got} after resizing to {want}, applying it again",
                window.id()
            );
            window.resize(*want);
        }
        let remaining = verify.0;
        verify.0 = remaining.saturating_sub(1);
        if remaining < 2
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.try_remove::<VerifyResize>();
        }
    }
}

/// Republishes the pointer and gesture events onto [`InputEvent`], so the input
/// systems do not have to sift the whole stream for them every frame.
///
/// Runs right after [`pump_events`], which is what puts this frame's events on
/// the stream in the first place.
pub(crate) fn demux_input_events(
    mut messages: MessageReader<Event>,
    mut input: MessageWriter<InputEvent>,
) {
    for event in messages.read() {
        if event.is_input() {
            input.write(InputEvent(event.clone()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pump_events(
    mut exit: MessageWriter<AppExit>,
    mut messages: MessageWriter<Event>,
    low_power_mode: Option<Res<LowPowerMode>>,
    incoming_events: Option<NonSend<Receiver<Event>>>,
    platform: Option<NonSendMut<Pin<Box<PlatformCallbacks>>>>,
    activity: FrameActivity,
    mut timeout: Local<u32>,
    mut last_tap_check: Local<Option<Instant>>,
) {
    let Some((ref mut platform, incoming_events)) = platform.zip(incoming_events) else {
        // No platform interface or incoming event pipe - probably executing in a unit test.
        return;
    };

    // Deliberately not paced to a frame period: waiting a full period put a
    // floor under how soon a pump could start, so an event landing right after
    // one returned had to wait out the rest of it. That latency is worse than
    // the redundant work skipping the wait costs.
    platform.pump_cocoa_event_loop(f64::from(*timeout) / 1000.0);

    let deadline = Instant::now() + PUMP_BUDGET;
    let mut received_events = Vec::new();
    let mut pending_mouse = None;
    let mut woke = false;

    // `true` when the channel went quiet, `false` when a cap sent us home with
    // events still queued. Only the quiet case may back the poll timeout off.
    let drained = loop {
        // Checked before the receive so a burst cannot keep extending the stay:
        // whatever is left stays in the channel for the next frame.
        if received_events.len() >= PUMP_MAX_EVENTS || Instant::now() >= deadline {
            trace!(
                "pump_events: yielding the frame with {} events taken",
                received_events.len()
            );
            break false;
        }

        // Non-blocking drain: the Cocoa pump above already did this frame's
        // sleeping, so an empty channel means we can proceed immediately
        // without paying an extra millisecond of wait latency.
        match incoming_events.try_recv() {
            Ok(Event::Exit) | Err(TryRecvError::Disconnected) => {
                exit.write(AppExit::Success);
                return;
            }
            Ok(event) => {
                woke |= matches!(event, Event::SystemWoke { .. });
                if matches!(event, Event::MouseMoved { .. }) {
                    pending_mouse = Some(event);
                } else {
                    received_events.extend(pending_mouse.take());
                    received_events.push(event);
                }
                *timeout = LOOP_TIMEOUT_STEP;
            }
            Err(TryRecvError::Empty) => break true,
        }
    };

    received_events.extend(pending_mouse.take());
    messages.write_batch(received_events);

    if drained {
        let frame_active = activity.mid_frame();
        let low_power = low_power_mode.is_some_and(|low_power| low_power.0);
        let timeout_limit = if frame_active {
            LOOP_MAX_TIMEOUT_FRAME_ACTIVE_MS
        } else if low_power {
            LOOP_MAX_TIMEOUT_LOWPOWER_MS
        } else {
            LOOP_MAX_TIMEOUT_MS
        };
        *timeout = timeout.min(timeout_limit) + LOOP_TIMEOUT_STEP;
    } else {
        // Still backed up: come straight back rather than sleeping on it.
        *timeout = LOOP_TIMEOUT_STEP;
    }

    // macOS can invalidate the event tap while the machine sleeps without ever
    // notifying the callback, which kills every keybinding, click and swipe
    // while the socket and the layout engine carry on as normal. Waking is the
    // usual trigger, so sweep on a slow timer too for the deaths without one.
    if woke || last_tap_check.is_none_or(|last| last.elapsed() >= TAP_HEALTH_CHECK_INTERVAL) {
        *last_tap_check = Some(Instant::now());
        match platform.ensure_input_tap_alive() {
            TapHealth::Healthy => {}
            health => warn!("input tap not healthy (woke={woke}), recovery: {health:?}"),
        }
    }
}

#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn window_resized_update_frame(
    mut messages: MessageReader<Event>,
    mut windows: ResizableWindows,
    mut workspaces: Query<(&LayoutStrip, &mut Position)>,
) {
    for event in messages.read() {
        let Event::WindowResized { window_id } = event else {
            continue;
        };

        let Some((mut window, entity, position, mut bounds, unmanaged, resizing)) = windows
            .iter_mut()
            .find(|window| window.0.id() == *window_id)
        else {
            continue;
        };
        if matches!(unmanaged, Some(Unmanaged::Minimized | Unmanaged::Hidden)) {
            continue;
        }
        // Our own resize, echoed back: `commit_window_size` requested this size
        // and `animate_resize_entities` is still stepping toward it, so reading
        // the echo in here would fight the animation producing that difference.
        // Only a resize we did not initiate is new information.
        if resizing {
            continue;
        }
        let Ok(new_frame) = window.update_frame() else {
            continue;
        };
        let active_strip = workspaces
            .iter_mut()
            .find(|(strip, _)| strip.contains(entity));
        let tabbed = active_strip
            .as_ref()
            .is_some_and(|strip| strip.0.tabbed(entity));

        let old_frame = IRect::from_corners(position.0, position.0 + bounds.0);
        if old_frame.size() != new_frame.size() {
            if tabbed {
                bounds.bypass_change_detection().0 = new_frame.size();
            } else {
                bounds.0 = new_frame.size();
            }
        }

        // If the window was resized, shift LayoutStrip slightly to avoid moving right corner.
        let Some((strip, mut strip_position)) = active_strip else {
            // Floating window, don't nudge the strip.
            continue;
        };
        if tabbed {
            // Native tabs share a single layout slot. Keep the strip anchored
            // and let the tab sync/layout systems propagate the new size.
            continue;
        }

        if old_frame.min.x != new_frame.min.x {
            let shift = (old_frame.size() - new_frame.size()).with_y(0);
            // Search marke: reposition_entity - Updating position directly to reduce jitter.
            strip_position.0.x += shift.x;
        }

        // When the user drags the top edge of a stacked window, we adjust the window above to
        // accomodate.
        let diff = old_frame.min.y - new_frame.min.y;
        if diff.abs() > 0
            && let Some(above_entity) = strip.above(entity)
            && let Ok((_, _, _, mut above_bounds, _, _)) = windows.get_mut(above_entity)
            && above_bounds.0.y - diff > 200
        {
            above_bounds.0.y -= diff;
        }
    }
}

#[instrument(level = Level::TRACE, skip_all)]
pub(crate) fn window_moved_update_frame(
    mut messages: MessageReader<Event>,
    mut windows: MovableWindows,
) {
    for event in messages.read() {
        let Event::WindowMoved { window_id } = event else {
            continue;
        };

        let Some((mut window, mut position, bounds, unmanaged, repositioning)) = windows
            .iter_mut()
            .find(|window| window.0.id() == *window_id)
        else {
            continue;
        };
        if matches!(unmanaged, Some(Unmanaged::Minimized | Unmanaged::Hidden)) {
            continue;
        }
        // Our own move, echoed back: `animate_entities` lerps from the current
        // `Position`, so overwriting it with the echoed frame mid-animation
        // restarts each step from behind, and the two chase each other.
        if repositioning {
            continue;
        }
        let Ok(new_frame) = window.update_frame() else {
            continue;
        };

        let old_frame = IRect::from_corners(position.0, position.0 + bounds.0);
        if old_frame.min != new_frame.min {
            position.0 = new_frame.min;
        }
    }
}

pub(crate) fn gather_initial_processes(
    receiver: Option<NonSendMut<Receiver<Event>>>,
    existing_config: Option<Res<Config>>,
    mut displays: Query<&mut Display>,
    mut commands: Commands,
) {
    let Some(receiver) = receiver else {
        // Probably running in a mock environment, ignore.
        return;
    };
    let mut initial_processes: Vec<BProcess> = Vec::new();
    let mut toml_config = None;
    loop {
        match receiver.recv().expect("error reading initial processes") {
            Event::ProcessesLoaded | Event::Exit => break,
            Event::ApplicationLaunched { psn, observer } => {
                let process: BProcess = Process::new(&psn, observer.clone()).into();
                if process.pid() != 0 {
                    initial_processes.push(process);
                } else {
                    debug!("Skipping process with PID 0 (likely kernel_task).");
                }
            }
            Event::InitialConfig(config) => {
                toml_config = Some(config);
            }
            event => warn!("Stray event during initial process gathering: {event:?}"),
        }
    }

    // A Lua `paneru.setup{...}` config is inserted at build time and wins; the
    // TOML config drained from the channel is only the fallback. Use whichever
    // is authoritative for the force-manage and menubar decisions below.
    let effective = existing_config
        .as_deref()
        .cloned()
        .or_else(|| toml_config.clone());

    if let Some(config) = &effective {
        let height = config.menubar_height();
        for mut display in &mut displays {
            display.set_menubar_height_override(height);
        }
    }

    while let Some(mut process) = initial_processes.pop() {
        let forced = effective
            .as_ref()
            .is_some_and(|c| c.should_force_manage_process(&**process));

        if process.is_observable() || forced {
            if forced {
                debug!(
                    "Forcing management of existing process '{}' despite unobservable policy.",
                    process.name()
                );
                process.force_manage(true);
            } else {
                debug!("Adding existing process {}", process.name());
            }
            commands.spawn((ExistingMarker, process));
        } else {
            debug!(
                "Existing application '{}' is not observable, ignoring it.",
                process.name(),
            );
        }
    }

    // The input event tap holds its own clone of the `Config` handle from
    // `InitialConfig` and reads swipe/scroll settings off it per event. A Lua
    // `paneru.setup{...}` builds a fresh handle, so its settings must be
    // published into the tap's existing handle rather than replacing it, or
    // gestures would keep reading stale settings.
    match (existing_config.as_deref(), toml_config) {
        #[cfg(feature = "lua")]
        (Some(lua_config), Some(shared)) => {
            shared.replace_inner_from(lua_config);
            commands.insert_resource(shared);
        }
        (None, Some(config)) => commands.insert_resource(config),
        _ => {}
    }
}

#[derive(Default)]
pub(super) struct OverlayWindowConfigCache {
    window_id: Option<WinID>,
    focused_border_radius: Option<f64>,
    detected_border_radius: Option<f64>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_overlays(
    active_workspace: Populated<(Has<Scrolling>, &LayoutStrip), With<ActiveWorkspaceMarker>>,
    windows: Windows,
    applications: Query<&Application>,
    overlay_mgr: Option<NonSendMut<OverlayManager>>,
    mission_control_active: Res<MissionControlActive>,
    config: Res<Config>,
    theme: Option<Res<SystemTheme>>,
    mut window_config_cache: Local<OverlayWindowConfigCache>,
    window_manager: Res<WindowManager>,
) {
    use crate::overlay::BorderParams;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let Some(mut overlay_mgr) = overlay_mgr else {
        return;
    };

    let dim_opacity = config.dim_inactive_opacity();
    let border_enabled = config.border_active_window();

    // Hide overlays during swipe, mission control, native fullscreen spaces,
    // or briefly after a space change (macOS space-switch animation).
    let Some((swiping, active_strip)) = active_workspace.iter().next() else {
        return;
    };

    if swiping || mission_control_active.0 || active_strip.is_fullscreen() {
        overlay_mgr.hide_all();
        return;
    }

    if dim_opacity == 0.0 && !border_enabled {
        overlay_mgr.remove_all();
        return;
    }

    let Some((window, entity)) = windows.focused() else {
        return;
    };
    let focused_window_id = window.id();
    let show_overlay = !window.is_full_screen()
        && (active_strip.contains(entity)
            // if the window is floating, check whether it's present in the workspace.
            || window_manager
                .windows_in_workspace(active_strip.id())
                .is_ok_and(|ids| ids.contains(&focused_window_id)));

    if !show_overlay {
        // No managed window on the active workspace has focus — hide the overlay rather than
        // dimming everything or drawing a ghost border around an off-screen window.
        overlay_mgr.hide_all();
        return;
    }

    // Find the focused managed window's absolute CG frame.
    let focused_abs_cg = {
        let frame = window.frame();
        let h_pad = window.horizontal_padding();
        let v_pad = window.vertical_padding();
        Some(NSRect::new(
            NSPoint::new(
                f64::from(frame.min.x + h_pad),
                f64::from(frame.min.y + v_pad),
            ),
            NSSize::new(
                f64::from(frame.width() - 2 * h_pad),
                f64::from(frame.height() - 2 * v_pad),
            ),
        ))
    };

    let border_params = if border_enabled {
        if window_config_cache.window_id != Some(focused_window_id) || config.is_changed() {
            let Some((window, _, parent)) = windows.find_parent(focused_window_id) else {
                return;
            };
            let Ok(app) = applications.get(parent) else {
                return;
            };
            let properties = WindowProperties::new(app, window, &config);
            window_config_cache.window_id = Some(focused_window_id);
            window_config_cache.focused_border_radius = properties.border_radius();
            window_config_cache.detected_border_radius = window.border_radius();
        }

        let calculated_radius = match config.border_radius() {
            BorderRadiusOption::Auto => window_config_cache.detected_border_radius.unwrap_or(10.0),
            BorderRadiusOption::Value(value) => value.max(0.0),
        };

        Some(BorderParams {
            color: config.border_color(theme.is_some_and(|theme| theme.is_dark)),
            opacity: config.border_opacity(),
            width: config.border_width(),
            radius: window_config_cache
                .focused_border_radius
                .unwrap_or(calculated_radius),
        })
    } else {
        window_config_cache.window_id = None;
        None
    };

    let dim_color = config.dim_inactive_color();
    overlay_mgr.update(
        dim_opacity,
        dim_color,
        focused_abs_cg,
        border_params.as_ref(),
    );
}

#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn commit_window_position(
    mut moved_windows: Populated<(&mut Window, &Position), Changed<Position>>,
) {
    // `par_iter_mut` runs on `ComputeTaskPool` worker threads, which have no
    // `CFRunLoop` of their own to drain thread-local autorelease pools.
    moved_windows
        .par_iter_mut()
        .for_each(|(mut window, position)| {
            objc2::rc::autoreleasepool(|_| window.reposition(position.0));
        });
}

#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn verify_window_position(
    mut windows: Populated<(Entity, &mut Window, &Position, &mut VerifyWindowPosition)>,
    mut commands: Commands,
) {
    for (entity, mut window, position, mut verification) in &mut windows {
        if window
            .update_frame()
            .is_ok_and(|frame| frame.min == position.0)
        {
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_remove::<VerifyWindowPosition>();
            }
            continue;
        }

        window.reposition(position.0);
        if verification.tick()
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.try_remove::<VerifyWindowPosition>();
        }
    }
}

#[instrument(level = Level::TRACE, skip_all)]
pub(super) fn commit_window_size(
    active_display: ActiveDisplay,
    mut resized_windows: Populated<(&mut Window, &Bounds, &mut WidthRatio), Changed<Bounds>>,
) {
    let display_bounds = active_display.bounds();
    resized_windows
        .par_iter_mut()
        .for_each(|(mut window, size, mut width_ratio)| {
            width_ratio.0 = f64::from(size.0.x) / f64::from(display_bounds.width());
            objc2::rc::autoreleasepool(|_| window.resize(size.0));
        });
}

/// Restores user-visible window state before the engine shuts down: clears any
/// brightness dim, removes the dim/border overlay window, and brings windows
/// left (partly) off screen back onto a display. Windows already fully on a
/// display and fullscreen windows are left alone.
pub(super) fn cleanup_on_exit(
    mut exit_events: MessageReader<AppExit>,
    mut all_windows: Query<&mut Window>,
    displays: Query<&Display>,
    window_manager: Res<WindowManager>,
    mut overlay_mgr: Option<NonSendMut<OverlayManager>>,
) {
    for _ in exit_events.read() {
        let ids = all_windows.iter().map(|w| w.id()).collect::<Vec<_>>();
        info!("exit cleanup: releasing {} window(s)", ids.len());
        window_manager.dim_windows(&ids, 0.0);

        if let Some(ref mut overlay_mgr) = overlay_mgr {
            overlay_mgr.remove_all();
        }

        let display_bounds = displays.iter().map(Display::bounds).collect::<Vec<_>>();
        if display_bounds.is_empty() {
            return;
        }

        let mut cascade = vec![0; display_bounds.len()];
        for mut window in &mut all_windows {
            if window.is_full_screen() {
                continue;
            }
            let frame = window.frame();
            let Some((display_index, target)) =
                exit_placement(frame, &display_bounds, &mut cascade)
            else {
                continue;
            };
            info!(
                "exit cleanup: window {} on display #{display_index}: {frame:?} -> {target:?}",
                window.id()
            );
            if target.size() != frame.size() {
                window.resize(target.size());
            }
            window.reposition(target.min);
        }
    }
}

/// Whether any decoration needs the overlay windows: a dim colour or an
/// active window border.
pub(crate) fn overlay_decorations_enabled(config: Option<Res<Config>>) -> bool {
    config.is_some_and(|config| config.has_dim_inactive_color() || config.border_active_window())
}

/// Drops the overlay windows once a config reload turned every decoration
/// off; `update_overlays` no longer runs then and would leave them stale.
pub(super) fn release_disabled_overlays(overlays: Option<NonSendMut<OverlayManager>>) {
    let Some(mut overlays) = overlays else {
        return;
    };
    if overlays.is_empty() {
        return;
    }
    debug!("decorations are off; removing the overlay windows");
    overlays.remove_all();
}

/// Spacing of the diagonal cascade [`exit_placement`] lays windows out in.
const EXIT_CASCADE_STEP: i32 = 30;

fn contains_rect(outer: IRect, inner: IRect) -> bool {
    outer.min.cmple(inner.min).all() && inner.max.cmple(outer.max).all()
}

/// Where exit cleanup puts a window with `frame`, and on which display:
/// `None` when it already sits fully inside one. Otherwise the display it
/// overlaps most (ties go to the nearest centre), shrunk to 90% if it does
/// not fit, and centred with a per-display diagonal cascade so released
/// windows do not land exactly on top of each other.
fn exit_placement(frame: IRect, displays: &[IRect], cascade: &mut [i32]) -> Option<(usize, IRect)> {
    if displays
        .iter()
        .any(|display| contains_rect(*display, frame))
    {
        return None;
    }
    let overlap = |display: &IRect| {
        let common = display.intersect(frame);
        if common.is_empty() {
            0
        } else {
            i64::from(common.width()) * i64::from(common.height())
        }
    };
    let distance = |display: &IRect| (display.center() - frame.center()).abs().element_sum();
    let index = displays
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            overlap(a)
                .cmp(&overlap(b))
                .then_with(|| distance(b).cmp(&distance(a)))
        })
        .map(|(index, _)| index)?;
    let display = displays[index];

    let mut size = frame.size();
    if size.x > display.width() || size.y > display.height() {
        size.x = size.x.min(display.width() * 9 / 10);
        size.y = size.y.min(display.height() * 9 / 10);
    }
    let step = cascade[index];
    cascade[index] += 1;
    let free = display.size() - size;
    let slack = free.min_element();
    let offset = if slack >= 1 {
        (step % (slack / EXIT_CASCADE_STEP + 1)) * EXIT_CASCADE_STEP - slack / 2
    } else {
        0
    };
    let origin = (display.min + free / 2 + offset).clamp(display.min, display.max - size);
    Some((index, IRect::from_corners(origin, origin + size)))
}

pub(crate) fn update_flash_messages(
    messages: Populated<(Entity, &FlashMessage, &Timeout)>,
    active_display: Single<(&Display, Entity), With<ActiveDisplayMarker>>,
    flash_mgr: Option<NonSendMut<FlashMessageManager>>,
    mut commands: Commands,
) {
    let Some(mut flash_manager) = flash_mgr else {
        return;
    };

    if messages.is_empty() {
        flash_manager.remove();
        return;
    }

    let (display, _) = *active_display;
    let bounds = display.bounds();
    let top_right = NSPoint::new(f64::from(bounds.max.x), f64::from(bounds.min.y));

    // When several FlashMessages coexist (rapid keypresses spawn a fresh
    // one per workspace switch before the previous timer expires), the
    // naïve loop would call `show()` for every one of them in arbitrary
    // order — the OSD ends up flickering between strings, and the moment
    // any one of them expires its `is_finished()` branch calls
    // `flash_manager.remove()` even though the newer ones are still
    // alive. Keep the newest (most time remaining), despawn the rest,
    // and render exactly once.
    let mut alive: Option<(Entity, &str, &Timeout)> = None;
    let mut stale: Vec<Entity> = Vec::new();
    for (entity, FlashMessage(flash), timeout) in messages {
        if timeout.timer.is_finished() {
            stale.push(entity);
            continue;
        }
        match alive {
            None => alive = Some((entity, flash, timeout)),
            Some((prev_entity, _, prev_timeout)) => {
                if timeout.timer.remaining() > prev_timeout.timer.remaining() {
                    stale.push(prev_entity);
                    alive = Some((entity, flash, timeout));
                } else {
                    stale.push(entity);
                }
            }
        }
    }

    for entity in stale {
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.try_despawn();
        }
    }

    if let Some((_, flash, timeout)) = alive {
        let opacity = timeout.timer.fraction_remaining();
        flash_manager.show(flash, opacity, top_right);
    } else {
        flash_manager.remove();
    }
}

pub(crate) fn update_low_power_state(low_power_mode: Option<ResMut<LowPowerMode>>) {
    let Some(mut state) = low_power_mode else {
        return;
    };
    let process_info = objc2_foundation::NSProcessInfo::processInfo();
    state.0 = process_info.isLowPowerModeEnabled();
}

#[instrument(level = Level::DEBUG, skip_all)]
pub(crate) fn window_creation_event(mut messages: MessageReader<Event>, mut commands: Commands) {
    for event in messages.read() {
        let Event::WindowCreated { element } = event else {
            continue;
        };

        if let Ok(window) = WindowOS::new(element)
            .inspect_err(|err| {
                trace!("not adding window {element:?}: {err}");
            })
            .map(|window| Window::new(Box::new(window)))
        {
            commands.trigger(SpawnWindowTrigger(vec![window]));
        }
    }
}

/// Managed windows whose geometry has settled: nothing in flight, so two of them
/// sharing a frame really do share it.
type SettledWindows<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Window,
        &'static Position,
        &'static Bounds,
        &'static ChildOf,
    ),
    (
        Without<Unmanaged>,
        Without<RepositionMarker>,
        Without<ResizeMarker>,
    ),
>;

/// Whether the app is showing `window_id` right now.
///
/// Two signals, because neither alone covers a native tab: the window server
/// stops listing a window it has ordered out, and an app drops a background tab
/// from its accessibility list while the window server still calls that tab on
/// screen. Ghostty does the latter. An empty accessibility list is an app that
/// did not answer, not an app showing nothing.
pub(crate) fn app_shows_window(
    ax_window_ids: &[WinID],
    on_screen: &[WinID],
    window_id: WinID,
) -> bool {
    on_screen.contains(&window_id)
        && (ax_window_ids.is_empty() || ax_window_ids.contains(&window_id))
}

/// Re-applies native dimming when a config reload changes the dim level.
/// The first run only records the level; theme changes are handled by
/// `theme_change_trigger`.
#[allow(
    clippy::option_option,
    reason = "the applied level is: not yet seen, dimming off, or a level"
)]
pub(crate) fn sync_native_dim(
    windows: Windows,
    window_manager: Res<WindowManager>,
    config: Option<Res<Config>>,
    theme: Option<Res<SystemTheme>>,
    mut applied: Local<Option<Option<f32>>>,
) {
    let Some(config) = config else {
        return;
    };
    let level = config.window_dim_ratio(theme.is_some_and(|theme| theme.is_dark));
    let Some(previous) = applied.replace(level) else {
        return;
    };
    if previous == level {
        return;
    }
    let focused = windows.focused().map(|(window, _)| window.id());
    let (ids, value) = match level {
        Some(value) => (
            windows
                .iter()
                .map(|(window, _)| window.id())
                .filter(|id| Some(*id) != focused)
                .collect::<Vec<_>>(),
            value,
        ),
        // Dimming turned off: undim everything.
        None => (windows.iter().map(|(window, _)| window.id()).collect(), 0.0),
    };
    debug!(
        "native dim level changed to {value}; applying it to {} window(s)",
        ids.len()
    );
    if !ids.is_empty() {
        window_manager.dim_windows(&ids, value);
    }
}

/// Two same-app windows whose sizes differ by less than this (on either axis)
/// may be tabs of one group.
const TAB_SIZE_TOLERANCE: u32 = 5;
/// ...and they share a frame when their origins are at most this far apart.
const TAB_ORIGIN_TOLERANCE: u32 = 4;

/// How a stray tab was matched to the tab that is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabMatch {
    /// Same app, same size and same origin.
    Frame,
    /// Same app and size, and the only such window showing.
    Size,
}

/// What [`stray_native_tabs`] needs to know about a window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TabProbe {
    pub entity: Entity,
    pub app: Entity,
    pub frame: IRect,
    /// See [`app_shows_window`].
    pub showing: bool,
}

/// Background tabs sitting in a column of their own, each paired with the
/// showing tab whose column it belongs in.
pub(crate) fn stray_native_tabs(
    strip: &LayoutStrip,
    probes: &[TabProbe],
) -> Vec<(Entity, Entity, TabMatch)> {
    strip
        .columns()
        .filter_map(|column| match column {
            Column::Single(entity) => Some(*entity),
            Column::Stack(_) | Column::Tabs(_) | Column::Fullscren(_) => None,
        })
        .filter_map(|stray| {
            let probe = probes
                .iter()
                .find(|probe| probe.entity == stray && !probe.showing)?;
            let candidates = probes
                .iter()
                .filter(|candidate| {
                    candidate.showing
                        && candidate.entity != stray
                        && candidate.app == probe.app
                        && candidate
                            .frame
                            .size()
                            .chebyshev_distance(probe.frame.size())
                            < TAB_SIZE_TOLERANCE
                })
                .collect::<Vec<_>>();
            if let Some(leader) = candidates.iter().find(|candidate| {
                candidate.frame.min.chebyshev_distance(probe.frame.min) <= TAB_ORIGIN_TOLERANCE
            }) {
                Some((stray, leader.entity, TabMatch::Frame))
            } else if let [leader] = candidates.as_slice() {
                Some((stray, leader.entity, TabMatch::Size))
            } else {
                None
            }
        })
        .collect()
}

/// Folds `follower` into the column of `leader` as a native tab, keeping the
/// leader as the visible head of the group.
pub(crate) fn fold_native_tab(strip: &mut LayoutStrip, leader: Entity, follower: Entity) -> bool {
    if let Err(err) = strip.convert_to_tabs(leader, follower) {
        error!("Failed to convert to tabs: {err}");
        return false;
    }
    if let Ok(index) = strip.index_of(leader)
        && let Some(column) = strip.get_column_mut(index)
    {
        column.move_to_front(leader);
    }
    true
}

/// Folds a background native tab that ended up in a column of its own back into
/// the column of the tab that is actually showing.
///
/// [`detect_tabbed_windows`] catches this when the tab window is created, but
/// only when the app has already stopped showing the sibling by then. Ghostty
/// does not always oblige, and the leftover column is a slot in the strip that
/// can never show anything: focus lands in it, the strip scrolls to it, and
/// there is nothing there.
///
/// Deliberately narrow. Two managed windows of one app share a frame exactly
/// only when they share a column, which is what this is repairing, and
/// [`app_shows_window`] is what tells a background tab from a window the user
/// can see.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
pub(crate) fn regroup_stray_native_tabs(
    windows: SettledWindows,
    apps: Query<&Application>,
    mut workspaces: Query<(&mut LayoutStrip, Has<ActiveWorkspaceMarker>)>,
    window_manager: Res<WindowManager>,
    mission_control: Res<MissionControlActive>,
    mut pending: Local<HashSet<(Entity, Entity)>>,
    mut commands: Commands,
) {
    if mission_control.0 {
        return;
    }
    let Some(mut strip) = workspaces
        .iter_mut()
        .find_map(|(strip, active)| active.then_some(strip))
    else {
        return;
    };
    let Some(on_screen) = window_manager.windows_on_screen() else {
        return;
    };

    // One accessibility round trip per app, not per window: this runs on a timer
    // and the call crosses into the app.
    let mut ax_windows: EntityHashMap<Vec<WinID>> = EntityHashMap::default();
    let probes = strip
        .all_windows()
        .into_iter()
        .filter_map(|entity| windows.get(entity).ok())
        .map(|(entity, window, position, bounds, child)| {
            let app = child.parent();
            let ax_window_ids = ax_windows.entry(app).or_insert_with(|| {
                apps.get(app)
                    .map(|app| app.ax_window_ids())
                    .unwrap_or_default()
            });
            TabProbe {
                entity,
                app,
                frame: IRect::from_corners(position.0, position.0 + bounds.0),
                showing: app_shows_window(ax_window_ids, &on_screen, window.id()),
            }
        })
        .collect::<Vec<_>>();

    // A match on size alone is weaker evidence than a shared frame, so it has
    // to be seen on two consecutive runs before it folds.
    let mut next = HashSet::new();
    let mut regrouped = Vec::new();
    for (stray, leader, matched) in stray_native_tabs(&strip, &probes) {
        if matched == TabMatch::Size && !pending.contains(&(stray, leader)) {
            next.insert((stray, leader));
            continue;
        }
        debug!("stray native tab {stray} folded into the column of {leader} ({matched:?})");
        if fold_native_tab(&mut strip, leader, stray) {
            regrouped.push(leader);
        }
    }
    *pending = next;

    if let Some(leader) = regrouped.first() {
        commands.reshuffle_around(*leader);
    }
}

/// When a native tab switch brings up a window that sits in a column of its
/// own, folds it into the column of the hidden tab it replaced.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn fold_tab_switch_on_focus(
    focused: Single<Entity, Added<FocusedMarker>>,
    mut windows: Query<(&mut Window, &Position, &Bounds, &ChildOf), Without<Unmanaged>>,
    apps: Query<&Application>,
    mut strips: Query<&mut LayoutStrip, With<ActiveWorkspaceMarker>>,
    window_manager: Res<WindowManager>,
    mission_control: Res<MissionControlActive>,
    mut commands: Commands,
) {
    if mission_control.0 {
        return;
    }
    let focused = *focused;
    let Some(mut strip) = strips.iter_mut().find(|strip| strip.contains(focused)) else {
        return;
    };
    if !strip
        .index_of(focused)
        .and_then(|index| strip.get(index))
        .is_ok_and(|column| matches!(column, Column::Single(_)))
    {
        return;
    }
    let Ok((mut window, position, bounds, child)) = windows.get_mut(focused) else {
        return;
    };
    let app = child.parent();
    let frame = window
        .update_frame()
        .unwrap_or_else(|_| IRect::from_corners(position.0, position.0 + bounds.0));
    let window_id = window.id();
    let Some(on_screen) = window_manager.windows_on_screen() else {
        return;
    };
    let ax_window_ids = apps
        .get(app)
        .map(|app| app.ax_window_ids())
        .unwrap_or_default();
    if !app_shows_window(&ax_window_ids, &on_screen, window_id) {
        return;
    }

    let probes = strip
        .all_windows()
        .into_iter()
        .filter(|&entity| entity != focused)
        .filter_map(|entity| {
            let (window, position, bounds, child) = windows.get(entity).ok()?;
            (child.parent() == app).then(|| TabProbe {
                entity,
                app,
                frame: IRect::from_corners(position.0, position.0 + bounds.0),
                showing: app_shows_window(&ax_window_ids, &on_screen, window.id()),
            })
        })
        .filter(|probe| probe.frame.size().chebyshev_distance(frame.size()) < TAB_SIZE_TOLERANCE)
        .collect::<Vec<_>>();
    let any_showing = probes.iter().any(|probe| probe.showing);
    let shown = |entity: Entity| {
        probes
            .iter()
            .find(|probe| probe.entity == entity)
            .is_some_and(|probe| probe.showing)
    };
    // Hidden candidates whose whole tab group is hidden too.
    let hidden = probes
        .iter()
        .filter(|probe| {
            !probe.showing
                && strip
                    .tab_group(probe.entity)
                    .unwrap_or_else(|| vec![probe.entity])
                    .into_iter()
                    .all(|entity| !shown(entity))
        })
        .collect::<Vec<_>>();
    let Some(target) = hidden
        .iter()
        .find(|probe| probe.frame.min.chebyshev_distance(frame.min) <= TAB_ORIGIN_TOLERANCE)
        .or_else(|| (hidden.len() == 1 && !any_showing).then(|| &hidden[0]))
        .map(|probe| probe.entity)
    else {
        return;
    };

    debug!("tab switch: {focused} shown in place of {target}; joining its column");
    if strip
        .convert_to_tabs(target, focused)
        .inspect_err(|err| error!("Failed to convert to tabs: {err}"))
        .is_ok()
    {
        commands.reshuffle_around(target);
    }
}

pub(crate) fn detect_tabbed_windows(
    created: Populated<(Entity, &Position, &Bounds, &ChildOf), Added<Window>>,
    windows: Query<(Entity, &Window, &Position, &Bounds, &ChildOf), With<Window>>,
    apps: Query<(Entity, &Application)>,
    mut workspaces: Query<(&mut LayoutStrip, Has<ActiveWorkspaceMarker>)>,
    window_manager: Res<WindowManager>,
    active_display: Single<&Display, With<ActiveDisplayMarker>>,
    mut commands: Commands,
) {
    let display_bounds = active_display.bounds();
    let Some(workspace_entities) = workspaces
        .iter()
        .find_map(|(strip, active)| active.then_some(strip.all_windows()))
    else {
        return;
    };

    for (entity, Position(position), Bounds(bounds), child) in created {
        let Ok((app_entity, app)) = apps.get(child.parent()) else {
            continue;
        };

        // First find all the windows which have the same size and the same parent app.
        // .. and in the same workspace.
        let mut same_size = workspace_entities
            .iter()
            .filter_map(|e| windows.get(*e).ok())
            .filter(|(leader, _, _, Bounds(leader_bounds), child)| {
                *leader != entity
                    && child.parent() == app_entity
                    && leader_bounds.chebyshev_distance(*bounds) <= 1
            })
            .collect::<Vec<_>>();
        if same_size.is_empty() {
            continue;
        }

        let on_screen = window_manager.windows_on_screen().unwrap_or_default();
        let ax_window_ids = app.ax_window_ids();
        // A leader the app has stopped showing is the tab this one replaced.
        let hidden_leader = |leader_id| !app_shows_window(&ax_window_ids, &on_screen, leader_id);

        // Now check whether any of these found windows have the same position?
        let tabbed = same_size
            .iter()
            .find_map(|(leader, window, Position(leader_position), _, _)| {
                // If the window has a positional match, it's tabbed! The hidden
                // test belongs here rather than after the search: a group the
                // app is already showing one member of would otherwise be
                // picked and then rejected, hiding the sibling that matches.
                (leader_position.chebyshev_distance(*position) <= 1 && hidden_leader(window.id()))
                    .then_some((*leader, window.id()))
            })
            .or_else(|| {
                // Otherwise if no windows were found by position, sort all the windows by distance
                // and then pick the one which is currently offscreen.
                // This heuristic relaxes the position matching, because the window is bumped into view.
                same_size.sort_by_key(|(_, _, Position(candidate_position), _, _)| {
                    position.x.abs_diff(candidate_position.x)
                });
                same_size.into_iter().find_map(
                    |(leader, window, Position(leader_position), Bounds(leader_bounds), _)| {
                        let offscreen = !display_bounds.contains(*leader_position)
                            || !display_bounds.contains(*leader_position + leader_bounds);
                        offscreen.then_some((leader, window.id()))
                    },
                )
            });

        if let Some((leader, leader_id)) = tabbed
            && hidden_leader(leader_id)
            && let Some((mut strip, _)) =
                workspaces.iter_mut().find(|strip| strip.0.contains(leader))
            && strip.contains(leader)
        {
            debug!("Tabbed window detected: adding {entity} to leader {leader}");
            if strip
                .convert_to_tabs(leader, entity)
                .inspect_err(|err| error!("Failed to convert to tabs: {err}"))
                .is_ok()
            {
                commands.focus_entity(entity, false);
            }
        }
    }
}

/// Listens for focus events for unknown window IDs and attempts to auto-discover
/// and manage them on the fly (e.g., when a user clicks an unmanaged tab or window).
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn auto_discover_unmanaged_focused_windows(
    mut messages: MessageReader<Event>,
    windows: Query<&Window>,
    apps: Query<&Application>,
    config: Res<Config>,
    mut cache: Local<HashSet<WinID>>,
    mut commands: Commands,
) {
    const CACHE_CLEANUP_SIZE: usize = 1000;
    if cache.len() > CACHE_CLEANUP_SIZE {
        cache.clear();
    }

    for event in messages.read() {
        let Event::WindowFocused { window_id } = *event else {
            continue;
        };

        if windows.iter().any(|w| w.id() == window_id) || cache.contains(&window_id) {
            continue;
        }

        trace!("Focus event for unknown window id {window_id}; attempting on-the-fly discovery.");

        let mut discovered = None;
        let (frontmost, non_frontmost): (Vec<_>, Vec<_>) =
            apps.iter().partition(|app| app.is_frontmost());

        for app in frontmost.into_iter().chain(non_frontmost) {
            if let Some(window) = app.focused_window(&config)
                && window.id() == window_id
            {
                debug!("Discovered unknown focused window {window_id} via focused_window.");
                discovered = Some(window);
                break;
            }

            let window_list = app.window_list(&config);
            if let Some(window) = window_list.into_iter().find(|w| w.id() == window_id) {
                debug!("Discovered unknown focused window {window_id} via window_list scan.");
                discovered = Some(window);
                break;
            }
        }

        if let Some(window) = discovered {
            commands.trigger(SpawnWindowTrigger(vec![window]));
        } else {
            trace!(
                "Failed to discover manageable window for focused ID {window_id}; caching as unmanageable."
            );
            cache.insert(window_id);
        }
    }
}

#[cfg(all(test, feature = "lua"))]
mod tests {
    use std::sync::mpsc::channel;

    use bevy::prelude::*;

    use super::gather_initial_processes;
    use crate::config::Config;
    use crate::events::Event;

    /// The input event tap keeps the handle it received on `InitialConfig` and
    /// reads swipe settings off it per event, so the Lua config must land in
    /// *that* handle rather than in a fresh one only the ECS can see.
    #[test]
    fn lua_config_reaches_the_handle_the_event_tap_holds() {
        let lua_config: Config = "[options]\n[swipe.gesture]\nfingers_count = 3\n"
            .try_into()
            .expect("config should parse");
        let tap_config = Config::defaults().expect("defaults should parse");
        assert_eq!(tap_config.swipe_gesture_fingers(), None);

        let (sender, receiver) = channel();
        sender
            .send(Event::InitialConfig(tap_config.clone()))
            .expect("send initial config");
        sender.send(Event::ProcessesLoaded).expect("send loaded");

        let mut app = App::new();
        app.insert_resource(lua_config);
        app.insert_non_send(receiver);
        app.add_systems(Update, gather_initial_processes);
        app.update();

        assert_eq!(tap_config.swipe_gesture_fingers(), Some(3));
    }
}

#[cfg(test)]
mod fork_tests {
    use super::*;
    use bevy::math::IVec2;

    #[test]
    fn visible_windows_stay_and_others_cascade() {
        let displays = [IRect::new(0, 0, 1000, 800), IRect::new(1000, 0, 2000, 800)];
        let mut cascade = vec![0; displays.len()];
        // Fully on a display: untouched.
        assert_eq!(
            exit_placement(IRect::new(10, 10, 410, 410), &displays, &mut cascade),
            None
        );
        // Mostly on the second display: centred there, then cascaded.
        let frame = IRect::new(900, 100, 1300, 500);
        let first = exit_placement(frame, &displays, &mut cascade).unwrap();
        let second = exit_placement(frame, &displays, &mut cascade).unwrap();
        assert_eq!(first.0, 1);
        assert_eq!(first.1.size(), frame.size());
        assert_eq!(second.1.min - first.1.min, IVec2::splat(EXIT_CASCADE_STEP));
        assert!(contains_rect(displays[1], first.1) && contains_rect(displays[1], second.1));
        // Too big: shrunk to 90% of the display it overlaps most.
        let (index, big) =
            exit_placement(IRect::new(-100, -100, 1200, 900), &displays, &mut cascade).unwrap();
        assert_eq!(index, 0);
        assert_eq!(big.size(), IVec2::new(900, 720));
    }

    #[test]
    fn stray_tabs_match_by_frame_then_unique_size() {
        let mut world = bevy::ecs::world::World::new();
        let e = (0..5).map(|_| world.spawn_empty().id()).collect::<Vec<_>>();
        let (app, other_app) = (e[3], e[4]);
        let mut strip = LayoutStrip::default();
        for &entity in &e[..3] {
            strip.append(entity);
        }
        let probe = |entity, app, x, showing| TabProbe {
            entity,
            app,
            frame: IRect::new(x, 20, x + 400, 720),
            showing,
        };

        // A hidden window 4 px off a showing sibling's frame: same group.
        let probes = [probe(e[0], app, 0, true), probe(e[1], app, 4, false)];
        assert_eq!(
            stray_native_tabs(&strip, &probes),
            vec![(e[1], e[0], TabMatch::Frame)]
        );

        // Elsewhere, but the only showing window of that size: a size match.
        let probes = [probe(e[0], app, 0, true), probe(e[1], app, 800, false)];
        assert_eq!(
            stray_native_tabs(&strip, &probes),
            vec![(e[1], e[0], TabMatch::Size)]
        );

        // Two showing candidates and no frame match, or another app: nothing.
        let probes = [
            probe(e[0], app, 0, true),
            probe(e[1], app, 800, false),
            probe(e[2], app, 400, true),
        ];
        assert_eq!(stray_native_tabs(&strip, &probes), vec![]);
        let probes = [probe(e[0], other_app, 0, true), probe(e[1], app, 0, false)];
        assert_eq!(stray_native_tabs(&strip, &probes), vec![]);

        // Folding keeps the leader in front.
        assert!(fold_native_tab(&mut strip, e[0], e[1]));
        assert_eq!(strip.tab_group(e[0]), Some(vec![e[0], e[1]]));
    }
}

#[cfg(test)]
mod animation_clock_tests {
    use super::AnimationClock;
    use std::time::Duration;

    #[test]
    fn steps_once_per_display_frame() {
        let mut clock = AnimationClock::default();
        let t0 = Duration::from_secs(5);
        // The first frame of an animation moves immediately by one frame.
        let first = clock.tick(t0, || 120.0).unwrap();
        assert!((first - 1.0 / 120.0).abs() < 1e-9);
        // Loop turns in between (echo events, ~1-3 ms apart) don't step.
        for ms in [1, 3, 5] {
            assert_eq!(
                clock.tick(t0 + Duration::from_millis(ms), || unreachable!()),
                None
            );
        }
        // Once a frame is due it steps by the real elapsed time.
        let step = clock
            .tick(t0 + Duration::from_millis(9), || unreachable!())
            .unwrap();
        assert!((step - 0.009).abs() < 1e-9);
        // A new animation after a reset starts immediately again.
        clock.reset();
        assert!(
            clock
                .tick(t0 + Duration::from_millis(10), || 60.0)
                .is_some()
        );
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(20), || unreachable!()),
            None
        );
    }
}
