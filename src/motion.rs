// motion

// How the drawn box follows the real one. The springs are only visual: copying, saving, the size label and the grab
// zones always use the real box.

use crate::anim::Spring;
use crate::selection::Rect;

// Seconds for the box to (almost) arrive.
const MODIFIER_SETTLE: f32 = 0.25; // after Ctrl or Shift changes the box's shape
const DRAG_SETTLE: f32 = 0.15; // while following the mouse
// An edge at rest this close to its target jumps there, so small adjustments feel direct.
const SNAP_DISTANCE: f32 = 4.0;
const SNAP_SPEED: f32 = 200.0; // pixels per second; faster edges are still on their way
// Close enough to call an edge arrived.
const ARRIVED_DISTANCE: f32 = 0.5;
// Longest frame to simulate, so a stall doesn't make the box jump.
const MAX_STEP: f32 = 0.05;

#[derive(Clone, Copy, Default)]
pub struct Settings {
	pub modifier_spring: bool,
	pub drag_spring: bool,
}

#[derive(Default)]
pub struct BoxMotion {
	edges: Option<[Spring; 4]>, // left, top, right, bottom
	box_id: u32,
	reshaping: bool, // springing to a shape Ctrl or Shift just changed
}

impl BoxMotion {
	// The box's shape changed because Ctrl or Shift was pressed or released.
	pub fn modifiers_changed(&mut self) {
		self.reshaping = true;
	}

	// Moves the drawn box `dt` seconds toward `target`. A different `box_id` means a new box, which is jumped to.
	pub fn step(&mut self, target: Option<Rect>, box_id: u32, dt: f32, settings: Settings) {
		let Some(target) = target else {
			self.edges = None;
			return;
		};
		let goal = edges_of(target);
		let edges = match self.edges.as_mut() {
			Some(edges) if box_id == self.box_id => edges,
			_ => {
				self.box_id = box_id;
				self.reshaping = false;
				self.edges = Some(goal.map(Spring::new));
				return;
			}
		};

		let dt = dt.min(MAX_STEP);
		let reshaping = self.reshaping && settings.modifier_spring;
		for (spring, goal) in edges.iter_mut().zip(goal) {
			if reshaping {
				spring.step(goal, dt, MODIFIER_SETTLE);
			} else if settings.drag_spring {
				let resting = (spring.value - goal).abs() <= SNAP_DISTANCE && spring.velocity.abs() < SNAP_SPEED;
				if resting {
					spring.snap(goal);
				} else {
					spring.step(goal, dt, DRAG_SETTLE);
				}
			} else {
				spring.snap(goal);
			}
		}
		if self.reshaping
			&& edges
				.iter()
				.zip(goal)
				.all(|(spring, goal)| (spring.value - goal).abs() <= ARRIVED_DISTANCE)
		{
			self.reshaping = false;
			for (spring, goal) in edges.iter_mut().zip(goal) {
				spring.snap(goal);
			}
		}
	}

	// The box as drawn, on whole pixels.
	pub fn drawn(&self) -> Option<Rect> {
		let [left, top, right, bottom] = self.edges?.map(|spring| spring.value.round() as i32);
		Some(Rect {
			left,
			top,
			right,
			bottom,
		})
	}

	// How far the drawn box's furthest edge is from `target`, in pixels.
	pub fn lag(&self, target: Option<Rect>) -> f32 {
		match (self.edges, target) {
			(Some(edges), Some(target)) => edges
				.iter()
				.zip(edges_of(target))
				.map(|(spring, goal)| (spring.value - goal).abs())
				.fold(0.0, f32::max),
			_ => 0.0,
		}
	}

	pub fn is_settled(&self, target: Option<Rect>) -> bool {
		self.lag(target) == 0.0 && self.edges.is_some() == target.is_some()
	}
}

fn edges_of(rect: Rect) -> [f32; 4] {
	[rect.left, rect.top, rect.right, rect.bottom].map(|edge| edge as f32)
}

#[cfg(test)]
mod tests {
	use super::*;

	const ALL: Settings = Settings {
		modifier_spring: true,
		drag_spring: true,
	};
	const NONE: Settings = Settings {
		modifier_spring: false,
		drag_spring: false,
	};

	fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
		Rect {
			left,
			top,
			right,
			bottom,
		}
	}

	#[test]
	fn new_box_is_jumped_to() {
		let mut motion = BoxMotion::default();
		motion.step(Some(rect(0, 0, 10, 10)), 1, 0.016, ALL);
		assert_eq!(motion.drawn(), Some(rect(0, 0, 10, 10)));
		motion.step(Some(rect(500, 500, 600, 600)), 2, 0.016, ALL);
		assert_eq!(motion.drawn(), Some(rect(500, 500, 600, 600)));
	}

	#[test]
	fn small_moves_snap_and_big_moves_spring() {
		let mut motion = BoxMotion::default();
		motion.step(Some(rect(0, 0, 100, 100)), 1, 0.016, ALL);
		motion.step(Some(rect(3, 0, 103, 100)), 1, 0.016, ALL);
		assert_eq!(motion.drawn(), Some(rect(3, 0, 103, 100)));
		motion.step(Some(rect(203, 0, 303, 100)), 1, 0.016, ALL);
		let drawn = motion.drawn().unwrap();
		assert!(drawn.left > 3 && drawn.left < 203, "left at {}", drawn.left);
		assert!(!motion.is_settled(Some(rect(203, 0, 303, 100))));
	}

	#[test]
	fn reshaping_springs_until_it_arrives() {
		let mut motion = BoxMotion::default();
		let target = rect(80, 100, 320, 200);
		motion.step(Some(rect(100, 100, 300, 200)), 1, 0.016, ALL);
		motion.modifiers_changed();
		motion.step(Some(target), 1, 0.016, ALL);
		assert_ne!(motion.drawn(), Some(target));
		for _ in 0..20 {
			motion.step(Some(target), 1, 0.016, ALL);
		}
		assert!(motion.is_settled(Some(target)));
		assert_eq!(motion.drawn(), Some(target));
	}

	#[test]
	fn no_springs_when_turned_off() {
		let mut motion = BoxMotion::default();
		motion.step(Some(rect(0, 0, 100, 100)), 1, 0.016, NONE);
		motion.modifiers_changed();
		motion.step(Some(rect(300, 300, 500, 500)), 1, 0.016, NONE);
		assert_eq!(motion.drawn(), Some(rect(300, 300, 500, 500)));
	}
}
