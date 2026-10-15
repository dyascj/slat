use std::collections::HashMap;
use std::time::Duration;

use bevy::math::{IRect, IVec2};
use bevy::prelude::*;

use crate::commands::{Command, Direction, Operation};
use crate::config::{Config, LayoutMode, MainOptions};
use crate::ecs::canvas::{DeskPosition, SavedCanvas, SavedCanvasWindow};
use crate::ecs::layout::LayoutStrip;
use crate::ecs::{LayoutPosition, Position, SpawnWindowTrigger};
use crate::events::Event;
use crate::manager::Window;

use super::*;

fn config(layout: LayoutMode) -> Config {
    (
        MainOptions {
            animation_speed: Some(10000.0),
            layout: Some(layout),
            ..Default::default()
        },
        vec![],
    )
        .into()
}

fn desk(world: &mut World, id: i32) -> DeskPosition {
    let entity = find_window_entity(id, world);
    *world
        .get::<DeskPosition>(entity)
        .unwrap_or_else(|| panic!("window {id} has no desk position"))
}

fn frame(world: &mut World, id: i32) -> IRect {
    let entity = find_window_entity(id, world);
    world.get::<Window>(entity).unwrap().frame()
}

fn strip_order(world: &mut World) -> Vec<i32> {
    let mut strips = world.query::<&LayoutStrip>();
    let entities = strips
        .iter(world)
        .find(|strip| !strip.all_windows().is_empty())
        .expect("a strip with windows")
        .all_windows();
    entities
        .into_iter()
        .map(|entity| world.get::<Window>(entity).unwrap().id())
        .collect()
}

fn step(command: Command) -> Event {
    Event::Command { command }
}

#[test]
fn switching_to_canvas_seeds_desks_from_the_columns() {
    TestHarness::new()
        .with_config(config(LayoutMode::Columns))
        .with_windows(3)
        .on_iteration(0, |world, _| {
            assert_eq!(frame(world, 1).min.x, TEST_WINDOW_WIDTH);
            world.insert_resource(config(LayoutMode::Canvas));
        })
        .on_iteration(1, |world, state| {
            for id in 0..3 {
                let entity = find_window_entity(id, world);
                let layout = world.get::<LayoutPosition>(entity).unwrap().0;
                assert_eq!(desk(world, id).pos, layout, "window {id}");
                assert_eq!(layout.x, id * TEST_WINDOW_WIDTH);
            }
            // A window opened now lands right of its neighbour, keeping its
            // own size.
            let window = state.spawn_window(
                TEST_PROCESS_ID,
                TEST_WORKSPACE_ID,
                3,
                IRect::new(50, 300, 350, 500),
            );
            world.trigger(SpawnWindowTrigger(vec![window]));
        })
        .on_iteration(2, |world, _| {
            // Inserted after the focused window 0, so it sits right of it
            // (no gap, overlapping window 1 is fine on a canvas).
            let new_desk = desk(world, 3).pos;
            assert_eq!(new_desk.x, desk(world, 0).pos.x + TEST_WINDOW_WIDTH);
            assert_eq!(strip_order(world), vec![0, 3, 1, 2]);
            // Y is kept inside the viewport.
            assert!(new_desk.y >= 0 && new_desk.y <= TEST_DISPLAY_HEIGHT);
            let entity = find_window_entity(3, world);
            assert_eq!(world.get::<LayoutPosition>(entity).unwrap().0, new_desk);
        })
        .run(vec![
            Event::MenuOpened { window_id: 0 },
            step(Command::PrintState),
            step(Command::PrintState),
        ]);
}

#[test]
fn canvas_swap_trades_horizontal_slots() {
    TestHarness::new()
        .with_config(config(LayoutMode::Columns))
        .with_windows(3)
        .on_iteration(0, |world, _| {
            world.insert_resource(config(LayoutMode::Canvas));
        })
        .on_iteration(2, |world, _| {
            // Window 0 (x 0) and window 1 (x 400) traded places; y and the
            // third window are untouched.
            assert_eq!(desk(world, 0).pos.x, TEST_WINDOW_WIDTH);
            assert_eq!(desk(world, 1).pos.x, 0);
            assert_eq!(desk(world, 2).pos.x, 2 * TEST_WINDOW_WIDTH);
            assert_eq!(desk(world, 0).pos.y, desk(world, 1).pos.y);
            assert_eq!(strip_order(world), vec![1, 0, 2]);
            assert_eq!(frame(world, 0).min.x, TEST_WINDOW_WIDTH);
            assert_eq!(frame(world, 1).min.x, 0);
        })
        .run(vec![
            Event::MenuOpened { window_id: 0 },
            step(Command::Window(Operation::Focus(Direction::First))),
            step(Command::Window(Operation::Swap(Direction::East))),
        ]);
}

#[test]
fn canvas_ignores_column_only_commands() {
    TestHarness::new()
        .with_config(config(LayoutMode::Columns))
        .with_windows(3)
        .on_iteration(0, |world, _| {
            world.insert_resource(config(LayoutMode::Canvas));
        })
        .on_iteration(4, |world, _| {
            let mut strips = world.query::<&LayoutStrip>();
            let strip = strips.iter(world).find(|s| s.len() > 0).unwrap();
            assert_eq!(strip.len(), 3, "stack is ignored in Canvas mode");
            for id in 0..3 {
                assert_eq!(frame(world, id).width(), TEST_WINDOW_WIDTH);
            }
        })
        .run(vec![
            Event::MenuOpened { window_id: 0 },
            step(Command::Window(Operation::Focus(Direction::Last))),
            step(Command::Window(Operation::Stack(true))),
            step(Command::Window(Operation::StackEast)),
            step(Command::Window(Operation::FullWidth)),
        ]);
}

#[test]
fn leaving_canvas_drops_desks_and_repacks() {
    TestHarness::new()
        .with_config(config(LayoutMode::Columns))
        .with_windows(2)
        .on_iteration(0, |world, _| {
            world.insert_resource(config(LayoutMode::Canvas));
        })
        .on_iteration(1, |world, _| {
            // Pull window 1 away from its packed slot, then go back.
            let entity = find_window_entity(1, world);
            world.get_mut::<DeskPosition>(entity).unwrap().pos.x = 600;
        })
        .on_iteration(2, |world, _| {
            assert_eq!(frame(world, 1).min.x, 600);
            world.insert_resource(config(LayoutMode::Columns));
        })
        .on_iteration(3, |world, _| {
            let entity = find_window_entity(1, world);
            assert!(world.get::<DeskPosition>(entity).is_none());
            assert_eq!(frame(world, 1).min.x, TEST_WINDOW_WIDTH);
        })
        .run(vec![
            Event::MenuOpened { window_id: 0 },
            step(Command::PrintState),
            step(Command::PrintState),
            step(Command::PrintState),
        ]);
}

#[test]
fn saved_canvas_places_matching_windows() {
    let saved = SavedCanvas {
        windows: vec![SavedCanvasWindow {
            window_id: 1,
            pid: Some(TEST_PROCESS_ID),
            bundle_id: String::new(),
            title: String::new(),
            desk_x: 700,
            desk_y: 50,
        }],
        expires_at: Duration::from_secs(60),
        strips: HashMap::from([((TEST_WORKSPACE_ID, 0), 0)]),
    };
    let mut harness = TestHarness::new()
        .with_config(config(LayoutMode::Canvas))
        .with_windows(2)
        .on_iteration(0, |world, _| {
            assert_eq!(desk(world, 1).pos, IVec2::new(700, 50));
            let frame = frame(world, 1);
            let strip_x = world
                .query_filtered::<&Position, With<LayoutStrip>>()
                .iter(world)
                .next()
                .unwrap()
                .0
                .x;
            assert_eq!(frame.min.x, 700 + strip_x);
            // Consumed, so a later window with the same id is not moved.
            assert_eq!(world.resource::<SavedCanvas>().windows, vec![]);
        });
    harness.world().insert_resource(saved);
    harness.run(vec![Event::MenuOpened { window_id: 0 }]);
}
