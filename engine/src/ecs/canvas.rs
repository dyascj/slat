//! The Canvas layout (`[options] layout = "canvas"`).
//!
//! Every window in a strip gets a free, strip-relative desk position instead of
//! a packed column slot, and keeps its own size. The column list survives only
//! as an ordering, kept sorted by desk x, and the strip offset pans the whole
//! desk. New windows are placed next to their neighbours, a mouse drag moves a
//! window's desk position, swapping left/right exchanges horizontal slots, and
//! the desk is saved to `canvas.json` so it can be restored shortly after a
//! restart.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::app::AppExit;
use bevy::ecs::change_detection::DetectChangesMut;
use bevy::ecs::component::Component;
use bevy::ecs::entity::{Entity, EntityHashMap, EntityHashSet};
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::lifecycle::Add;
use bevy::ecs::message::MessageReader;
use bevy::ecs::observer::On;
use bevy::ecs::query::{Added, Changed, Has, Or, With, Without};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, Local, Populated, Query, Res, ResMut};
use bevy::math::{IRect, IVec2};
use bevy::time::{Real, Time};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::{Config, LayoutMode};
use crate::ecs::layout::{Column, LayoutStrip};
use crate::ecs::{
    Bounds, DockPosition, FocusedMarker, Initializing, LayoutPosition, Position, RepositionMarker,
    SpawnCommandsExt, Unmanaged,
};
use crate::manager::{Application, Display, Window};
use crate::platform::WorkspaceId;

/// Auto-placed windows keep at least this much of their height on screen.
const DESK_Y_MARGIN: i32 = 40;
/// How long after startup a saved desk may still be matched to windows.
const SAVED_CANVAS_TTL: Duration = Duration::from_secs(60);
const CANVAS_STATE_FILE: &str = "canvas.json";
const CANVAS_STATE_VERSION: u32 = 1;
/// A mouse-down/up pair moving the window further than this is a drag.
pub(crate) const DRAG_THRESHOLD: i32 = 2;
/// For this long after a drag, OS-side moves still update the desk position.
pub(crate) const DRAG_SETTLE: Duration = Duration::from_millis(600);

/// A window's free position on its strip's desk, relative to the strip origin.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeskPosition {
    pub strip: Entity,
    pub pos: IVec2,
}

/// Marks a window that appeared while in Canvas mode, so it is placed next to
/// its neighbours rather than where the OS opened it.
#[derive(Component)]
pub struct CanvasNewWindow;

/// The window position at mouse-down, kept on the `MouseHeldMarker` entity.
#[derive(Component)]
pub struct CanvasDragStart(pub IVec2);

/// A just-dragged window whose late OS moves still count as part of the drag.
#[derive(Component)]
pub struct CanvasDragSettle;

/// Where the canvas state file lives.
#[derive(Resource)]
pub struct CanvasStatePath(pub PathBuf);

/// The desk loaded from disk, matched against windows until it expires.
#[derive(Resource, Debug)]
pub struct SavedCanvas {
    pub windows: Vec<SavedCanvasWindow>,
    /// Compared to `Time<Real>::elapsed()`.
    pub expires_at: Duration,
    pub strips: HashMap<(WorkspaceId, u32), i32>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CanvasStateFile {
    version: u32,
    windows: Vec<SavedCanvasWindow>,
    strips: Vec<SavedCanvasStrip>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCanvasWindow {
    pub window_id: i32,
    pub pid: Option<i32>,
    pub bundle_id: String,
    pub title: String,
    pub desk_x: i32,
    pub desk_y: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SavedCanvasStrip {
    workspace_id: u64,
    virtual_index: u32,
    offset_x: i32,
}

/// Run condition: the Canvas layout is selected.
pub fn canvas_enabled(config: Res<Config>) -> bool {
    config.canvas()
}

/// True (and logged) when `name` is a column-only command that Canvas mode
/// ignores.
pub fn canvas_blocks(config: &Config, name: &str) -> bool {
    let canvas = config.canvas();
    if canvas {
        debug!("{name}: not used in the Canvas layout, ignoring");
    }
    canvas
}

/// The default state file, `$XDG_STATE_HOME/paneru/canvas.json`.
pub fn default_state_file_path() -> Option<PathBuf> {
    xdg::BaseDirectories::with_prefix("paneru").get_state_file(CANVAS_STATE_FILE)
}

/// Loads a saved desk, or `None` if the file is missing, unreadable or from
/// another version.
pub fn load_saved_state(path: &Path) -> Option<SavedCanvas> {
    let file: CanvasStateFile = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    if file.version != CANVAS_STATE_VERSION {
        return None;
    }
    info!("loaded saved canvas state from {}", path.display());
    Some(SavedCanvas {
        strips: file
            .strips
            .iter()
            .map(|s| ((s.workspace_id, s.virtual_index), s.offset_x))
            .collect(),
        windows: file.windows,
        expires_at: SAVED_CANVAS_TTL,
    })
}

fn write_state_file(path: &Path, file: &CanvasStateFile) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(tmp, path)
}

type SaveStrips<'w, 's> = Query<'w, 's, (&'static LayoutStrip, &'static Position), Without<Window>>;
type SaveWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static Window,
        Option<&'static DeskPosition>,
        &'static ChildOf,
    ),
>;

fn save_canvas_state(
    path: &Path,
    strips: &SaveStrips,
    windows: &SaveWindows,
    apps: &Query<&Application>,
) {
    let mut file = CanvasStateFile {
        version: CANVAS_STATE_VERSION,
        windows: vec![],
        strips: vec![],
    };
    for (strip, strip_pos) in strips {
        if strip.is_fullscreen() {
            continue;
        }
        file.strips.push(SavedCanvasStrip {
            workspace_id: strip.id(),
            virtual_index: strip.virtual_index,
            offset_x: strip_pos.0.x,
        });
        for entity in strip.all_windows() {
            let Ok((window, Some(desk), parent)) = windows.get(entity) else {
                continue;
            };
            file.windows.push(SavedCanvasWindow {
                window_id: window.id(),
                pid: window.pid().ok(),
                bundle_id: apps
                    .get(parent.parent())
                    .ok()
                    .and_then(|app| app.bundle_id())
                    .unwrap_or_default(),
                title: window.title().unwrap_or_default(),
                desk_x: desk.pos.x,
                desk_y: desk.pos.y,
            });
        }
    }
    match write_state_file(path, &file) {
        Ok(()) => debug!("canvas state saved to {}", path.display()),
        Err(err) => warn!("failed to save canvas state to {}: {err}", path.display()),
    }
}

pub(crate) fn periodic_canvas_save(
    path: Option<Res<CanvasStatePath>>,
    config: Res<Config>,
    strips: SaveStrips,
    windows: SaveWindows,
    apps: Query<&Application>,
) {
    if let Some(path) = path
        && config.canvas()
    {
        save_canvas_state(&path.0, &strips, &windows, &apps);
    }
}

pub(crate) fn canvas_save_on_exit(
    mut exit: MessageReader<AppExit>,
    path: Option<Res<CanvasStatePath>>,
    config: Res<Config>,
    strips: SaveStrips,
    windows: SaveWindows,
    apps: Query<&Application>,
) {
    if exit.read().next().is_none() {
        return;
    }
    let Some(path) = path else { return };
    if !config.canvas() {
        return;
    }
    info!("Exiting, saving canvas state...");
    save_canvas_state(&path.0, &strips, &windows, &apps);
}

/// Tags windows that appear after startup, so Canvas mode places them next to
/// their neighbours. The first batch after initialisation is the startup
/// windows themselves and is skipped.
pub(crate) fn canvas_mark_new_windows(
    added: Populated<Entity, Added<Window>>,
    init: Option<Res<Initializing>>,
    config: Res<Config>,
    mut armed: Local<bool>,
    mut commands: Commands,
) {
    if init.is_some() {
        return;
    }
    if !*armed {
        *armed = true;
        return;
    }
    if !config.canvas() {
        return;
    }
    for entity in &added {
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.try_insert(CanvasNewWindow);
        }
    }
}

/// Where a window without a desk position on this strip lands.
///
/// A window entering the canvas (no desk yet, not new) keeps its on-screen
/// spot. A new window, or one arriving from another strip, sits right of its
/// nearest placed predecessor in strip order, else left of its nearest placed
/// successor, else where it is; its y is kept inside the viewport.
#[allow(clippy::too_many_arguments)]
pub(crate) fn auto_place(
    index: usize,
    all: &[Entity],
    occupied: &EntityHashMap<IRect>,
    size: IVec2,
    window_pos: IVec2,
    strip_pos: IVec2,
    previous_desk_y: Option<i32>,
    is_new: bool,
    viewport: IRect,
) -> IVec2 {
    if previous_desk_y.is_none() && !is_new {
        return window_pos - strip_pos;
    }
    let prev = all[..index].iter().rev().find_map(|e| occupied.get(e));
    let next = all[index + 1..].iter().find_map(|e| occupied.get(e));
    let x = if let Some(prev) = prev {
        prev.max.x
    } else if let Some(next) = next {
        next.min.x - size.x
    } else {
        window_pos.x - strip_pos.x
    };
    let y = previous_desk_y.unwrap_or(window_pos.y - viewport.min.y);
    let max_y = (viewport.height() - DESK_Y_MARGIN).max(0);
    IVec2::new(x, y.clamp(0, max_y))
}

/// Finds the saved entry for a window: by window id (and pid when both are
/// known), else by a bundle id and title pair that is unique in the file.
fn saved_match(
    saved: &[SavedCanvasWindow],
    window_id: i32,
    pid: Option<i32>,
    bundle_id: &str,
    title: &str,
) -> Option<usize> {
    let by_id = match pid {
        Some(pid) => saved
            .iter()
            .position(|e| e.window_id == window_id && e.pid.is_none_or(|p| p == pid)),
        None => saved
            .iter()
            .position(|e| e.window_id == window_id && e.pid.is_none()),
    };
    by_id.or_else(|| {
        if bundle_id.is_empty() {
            return None;
        }
        let mut it = saved
            .iter()
            .enumerate()
            .filter(|(_, e)| e.bundle_id == bundle_id && e.title == title);
        let first = it.next()?.0;
        it.next().is_none().then_some(first)
    })
}

type CanvasStrips<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut LayoutStrip,
        &'static mut Position,
        &'static ChildOf,
    ),
    Without<Window>,
>;

type CanvasWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static Window,
        &'static Position,
        &'static Bounds,
        Option<&'static DeskPosition>,
        Has<CanvasNewWindow>,
        &'static ChildOf,
    ),
    Without<LayoutStrip>,
>;

/// Gives every window in a strip a desk position and keeps the columns
/// sorted by desk x. Runs every frame in Canvas mode.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn canvas_assign_desks(
    mut strips: CanvasStrips,
    windows: CanvasWindows,
    changed_desk: Query<Entity, (With<Window>, Changed<DeskPosition>)>,
    apps: Query<&Application>,
    displays: Query<(&Display, Option<&DockPosition>)>,
    config: Res<Config>,
    mut saved: Option<ResMut<SavedCanvas>>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    if saved
        .as_ref()
        .is_some_and(|saved| time.elapsed() > saved.expires_at)
    {
        debug!("saved canvas state expired");
        commands.remove_resource::<SavedCanvas>();
        saved = None;
    }
    let changed: EntityHashSet = changed_desk.iter().collect();

    for (strip_entity, mut strip, mut strip_pos, child_of) in &mut strips {
        if strip.is_fullscreen() {
            continue;
        }
        let Ok((display, dock)) = displays.get(child_of.parent()) else {
            continue;
        };
        let viewport = display.actual_display_bounds(dock, &config);
        let all = strip.all_windows();
        if all.is_empty() {
            continue;
        }

        let mut occupied = EntityHashMap::<IRect>::default();
        let mut unplaced = vec![];
        for &entity in &all {
            let Ok((_, _, bounds, desk, _, _)) = windows.get(entity) else {
                continue;
            };
            match desk {
                Some(desk) if desk.strip == strip_entity => {
                    occupied.insert(entity, IRect::from_corners(desk.pos, desk.pos + bounds.0));
                }
                _ => unplaced.push(entity),
            }
        }

        let mut dirty = !unplaced.is_empty();
        let mut restore_offset = None;
        for &entity in &unplaced {
            let Ok((window, pos, bounds, desk, is_new, window_parent)) = windows.get(entity) else {
                continue;
            };
            let mut placed = None;
            if let Some(saved) = saved.as_deref_mut() {
                let bundle_id = apps
                    .get(window_parent.parent())
                    .ok()
                    .and_then(|app| app.bundle_id())
                    .unwrap_or_default();
                let title = window.title().unwrap_or_default();
                if let Some(index) = saved_match(
                    &saved.windows,
                    window.id(),
                    window.pid().ok(),
                    &bundle_id,
                    &title,
                ) {
                    let entry = saved.windows.swap_remove(index);
                    placed = Some(IVec2::new(entry.desk_x, entry.desk_y));
                    if let Some(&offset) = saved.strips.get(&(strip.id(), strip.virtual_index)) {
                        restore_offset = Some(offset);
                    }
                }
            }
            let desk_pos = placed.unwrap_or_else(|| {
                let index = all.iter().position(|&e| e == entity).unwrap_or(0);
                auto_place(
                    index,
                    &all,
                    &occupied,
                    bounds.0,
                    pos.0,
                    strip_pos.0,
                    desk.map(|desk| desk.pos.y),
                    is_new,
                    viewport,
                )
            });
            debug!("canvas: placing {entity} at desk {desk_pos} on strip {strip_entity}");
            occupied.insert(entity, IRect::from_corners(desk_pos, desk_pos + bounds.0));
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_insert(DeskPosition {
                    strip: strip_entity,
                    pos: desk_pos,
                });
                entity_commands.try_remove::<CanvasNewWindow>();
            }
        }

        if let Some(offset) = restore_offset
            && strip_pos.0.x != offset
        {
            debug!("canvas: restoring offset {offset} on strip {strip_entity}");
            strip_pos.0.x = offset;
        }

        for &entity in &all {
            if !changed.contains(&entity) {
                continue;
            }
            dirty = true;
            if let Ok((_, _, bounds, Some(desk), _, _)) = windows.get(entity)
                && desk.strip == strip_entity
            {
                occupied.insert(entity, IRect::from_corners(desk.pos, desk.pos + bounds.0));
            }
        }

        let key = |column: &Column| {
            column
                .top()
                .and_then(|e| occupied.get(&e))
                .map_or(i32::MAX, |rect| rect.min.x)
        };
        if !strip.columns().map(key).is_sorted() {
            debug!("canvas: re-sorting strip {strip_entity} by desk position");
            strip.sort_columns_by_key(key);
        } else if dirty {
            strip.set_changed();
        }
    }
}

/// Just-dragged windows the OS moved this frame, not by us.
type SettlingWindows<'w, 's> = Populated<
    'w,
    's,
    (&'static Position, &'static mut DeskPosition),
    (
        With<CanvasDragSettle>,
        Changed<Position>,
        Without<RepositionMarker>,
        Without<LayoutStrip>,
    ),
>;

/// For a short while after a drag, OS-side moves of the dragged window keep
/// updating its desk position.
pub(crate) fn canvas_drag_settle(
    windows: SettlingWindows,
    strips: Query<&Position, With<LayoutStrip>>,
) {
    for (pos, mut desk) in windows {
        let Ok(strip_pos) = strips.get(desk.strip) else {
            continue;
        };
        let offset = pos.0 - strip_pos.0;
        if desk.pos != offset {
            desk.pos = offset;
        }
    }
}

/// Follows `[options] layout` across config reloads. Entering Canvas seeds
/// every desk from the current packed layout; leaving it drops all desk state
/// and repacks the columns.
#[allow(clippy::type_complexity)]
pub(crate) fn canvas_layout_mode_switch(
    config: Res<Config>,
    mut last: Local<Option<LayoutMode>>,
    mut strips: Query<(Entity, &mut LayoutStrip)>,
    layout_positions: Query<&LayoutPosition>,
    canvas_windows: Query<
        Entity,
        Or<(
            With<DeskPosition>,
            With<CanvasNewWindow>,
            With<CanvasDragSettle>,
        )>,
    >,
    focused: Query<Entity, With<FocusedMarker>>,
    mut commands: Commands,
) {
    let now = config.layout_mode();
    let Some(previous) = last.replace(now) else {
        return;
    };
    if previous == now {
        return;
    }
    info!("layout mode changed: {previous} -> {now}");
    match now {
        LayoutMode::Canvas => {
            for (strip_entity, mut strip) in &mut strips {
                if strip.is_fullscreen() {
                    continue;
                }
                for entity in strip.all_windows() {
                    if let Ok(layout_position) = layout_positions.get(entity)
                        && let Ok(mut entity_commands) = commands.get_entity(entity)
                    {
                        entity_commands.try_insert(DeskPosition {
                            strip: strip_entity,
                            pos: layout_position.0,
                        });
                    }
                }
                strip.set_changed();
            }
        }
        LayoutMode::Columns => {
            for entity in &canvas_windows {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands
                        .try_remove::<(DeskPosition, CanvasNewWindow, CanvasDragSettle)>();
                }
            }
            for (_, mut strip) in &mut strips {
                strip.set_changed();
            }
            if let Ok(focused) = focused.single() {
                commands.reshuffle_around(focused);
            }
        }
    }
}

/// A window leaving the tiling layout leaves the desk too.
pub(crate) fn canvas_window_unmanaged(trigger: On<Add, Unmanaged>, mut commands: Commands) {
    if let Ok(mut entity_commands) = commands.get_entity(trigger.entity) {
        entity_commands.try_remove::<(DeskPosition, CanvasDragSettle)>();
    }
}

/// The strip x offset that keeps a window at desk x `layout_x` (width
/// `width`) visible, starting from the strip's `target_x`. Too wide windows
/// and `auto_center` center it; otherwise the strip pans only when the
/// window is hidden past `hidden_ratio`, and only far enough to expose it.
pub(crate) fn canvas_pan_x(
    layout_x: i32,
    width: i32,
    target_x: i32,
    viewport: IRect,
    auto_center: bool,
    hidden_ratio: f64,
) -> i32 {
    let display_width = viewport.width();
    if display_width - width < 0 || auto_center {
        return viewport.min.x + (display_width - width) / 2 - layout_x;
    }
    let left = layout_x + target_x;
    let right = left + width;
    if left >= viewport.min.x && right <= viewport.max.x {
        return target_x;
    }
    let expose = || {
        if left >= viewport.min.x {
            target_x + viewport.max.x - right
        } else {
            viewport.min.x - layout_x
        }
    };
    if hidden_ratio > 0.0 {
        let visible = (viewport.max.x.min(right) - viewport.min.x.max(left)).max(0);
        let hidden = 1.0 - f64::from(visible) / f64::from(width.max(1));
        if hidden <= hidden_ratio {
            return target_x;
        }
    }
    expose()
}

/// The new desk x of the focused window (`current`) and its swap partner
/// (`other`) when they trade horizontal slots: the one moving right lines up
/// with the partner's old right edge, the one moving left takes the partner's
/// old left edge.
pub(crate) fn swap_slots(
    current_x: i32,
    current_width: i32,
    other_x: i32,
    other_width: i32,
) -> (i32, i32) {
    let (a0, a1) = (
        current_x.min(current_x + current_width),
        current_x.max(current_x + current_width),
    );
    let (b0, b1) = (
        other_x.min(other_x + other_width),
        other_x.max(other_x + other_width),
    );
    if a0 > b0 {
        (b0, a1 - other_width)
    } else {
        (b1 - current_width, a0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_trades_slots() {
        // Current (300 wide at 500) swaps with a 200 wide window at 100.
        assert_eq!(swap_slots(500, 300, 100, 200), (100, 600));
        // And back: the partner is now on the right.
        assert_eq!(swap_slots(100, 300, 600, 200), (500, 100));
        // A gap between the two is preserved on the outer edges.
        let (cur, other) = swap_slots(0, 100, 400, 200);
        assert_eq!((cur, other), (500, 0));
    }

    #[test]
    fn pan_only_when_needed() {
        let viewport = IRect::new(0, 25, 1000, 800);
        // Fully visible: the strip stays.
        assert_eq!(canvas_pan_x(100, 300, 0, viewport, false, 0.0), 0);
        // Off the right edge: right-align it.
        assert_eq!(canvas_pan_x(900, 300, 0, viewport, false, 0.0), -200);
        // Off the left edge: left-align it.
        assert_eq!(canvas_pan_x(100, 300, -300, viewport, false, 0.0), -100);
        // Within the tolerated hidden ratio (2/3 hidden): stay.
        assert_eq!(canvas_pan_x(900, 300, 0, viewport, false, 0.7), 0);
        assert_eq!(canvas_pan_x(900, 300, 0, viewport, false, 0.5), -200);
        // Wider than the display, or auto-center: center it.
        assert_eq!(canvas_pan_x(100, 1200, 0, viewport, false, 0.0), -200);
        assert_eq!(canvas_pan_x(100, 400, 0, viewport, true, 0.0), 200);
    }

    #[test]
    fn auto_place_chains_off_neighbours() {
        let mut world = bevy::ecs::world::World::new();
        let all = (0..3).map(|_| world.spawn_empty().id()).collect::<Vec<_>>();
        let viewport = IRect::new(0, 25, 1000, 800);
        let mut occupied = EntityHashMap::default();
        occupied.insert(all[0], IRect::new(50, 0, 450, 600));

        // A new window after a placed one sits right of it, y clamped.
        let pos = auto_place(
            1,
            &all,
            &occupied,
            IVec2::new(300, 500),
            IVec2::new(0, 2000),
            IVec2::ZERO,
            None,
            true,
            viewport,
        );
        assert_eq!(pos, IVec2::new(450, 775 - DESK_Y_MARGIN));

        // With only a placed successor, it sits left of it.
        let pos = auto_place(
            0,
            &all[..],
            &[(all[1], IRect::new(500, 0, 900, 600))]
                .into_iter()
                .collect(),
            IVec2::new(300, 500),
            IVec2::new(0, 100),
            IVec2::ZERO,
            Some(10),
            false,
            viewport,
        );
        assert_eq!(pos, IVec2::new(200, 10));

        // A window entering the canvas keeps its on-screen spot.
        let pos = auto_place(
            2,
            &all,
            &occupied,
            IVec2::new(300, 500),
            IVec2::new(120, 60),
            IVec2::new(-100, 25),
            None,
            false,
            viewport,
        );
        assert_eq!(pos, IVec2::new(220, 35));
    }

    #[test]
    fn saved_windows_match_by_id_then_unique_title() {
        let entry = |window_id, pid, title: &str| SavedCanvasWindow {
            window_id,
            pid,
            bundle_id: "com.example".into(),
            title: title.into(),
            desk_x: 0,
            desk_y: 0,
        };
        let saved = vec![
            entry(1, Some(10), "A"),
            entry(2, None, "B"),
            entry(3, Some(10), "C"),
            entry(4, Some(10), "C"),
        ];
        assert_eq!(saved_match(&saved, 1, Some(10), "", ""), Some(0));
        assert_eq!(saved_match(&saved, 1, Some(11), "", ""), None);
        assert_eq!(saved_match(&saved, 2, Some(99), "", ""), Some(1));
        assert_eq!(saved_match(&saved, 2, None, "", ""), Some(1));
        // New window id, but a unique bundle id and title.
        assert_eq!(
            saved_match(&saved, 9, Some(10), "com.example", "A"),
            Some(0)
        );
        // Ambiguous titles never match.
        assert_eq!(saved_match(&saved, 9, Some(10), "com.example", "C"), None);
    }

    #[test]
    fn state_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("slat-canvas-test-{}", std::process::id()));
        let path = dir.join("nested").join(CANVAS_STATE_FILE);
        let file = CanvasStateFile {
            version: CANVAS_STATE_VERSION,
            windows: vec![SavedCanvasWindow {
                window_id: 1234,
                pid: None,
                bundle_id: "com.apple.Safari".into(),
                title: "Docs".into(),
                desk_x: 10,
                desk_y: 0,
            }],
            strips: vec![SavedCanvasStrip {
                workspace_id: 3,
                virtual_index: 0,
                offset_x: -400,
            }],
        };
        write_state_file(&path, &file).expect("state file is written");
        assert!(!path.with_extension("json.tmp").exists());

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"pid\": null"),
            "pid serializes as null: {text}"
        );
        let loaded = load_saved_state(&path).expect("state file loads");
        assert_eq!(loaded.windows, file.windows);
        assert_eq!(loaded.strips.get(&(3, 0)), Some(&-400));
        assert_eq!(loaded.expires_at, SAVED_CANVAS_TTL);

        // Another version, or a missing field, is ignored.
        fs::write(&path, text.replace("\"version\": 1", "\"version\": 2")).unwrap();
        assert!(load_saved_state(&path).is_none());
        fs::write(&path, r#"{"version": 1, "windows": []}"#).unwrap();
        assert!(load_saved_state(&path).is_none());
        _ = fs::remove_dir_all(dir);
    }
}
