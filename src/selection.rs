// selection

// The box the user draws on the overlay: creating, moving, resizing and flipping it.
// Pure logic with no Win32, so it can be unit tested.

// How far the mouse has to move from the press before it counts as a drag, in pixels.
pub const DRAG_THRESHOLD: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
	pub x: i32,
	pub y: i32,
}

// A rectangle in screen pixels. `right` and `bottom` are exclusive, so the width is `right - left`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
	pub left: i32,
	pub top: i32,
	pub right: i32,
	pub bottom: i32,
}

impl Rect {
	pub fn width(&self) -> i32 {
		self.right - self.left
	}

	pub fn height(&self) -> i32 {
		self.bottom - self.top
	}

	pub fn contains(&self, p: Point) -> bool {
		p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
	}

	pub fn is_empty(&self) -> bool {
		self.width() <= 0 || self.height() <= 0
	}

	// The part of `self` that lies inside `other`, or `None` if they don't overlap.
	pub fn intersect(&self, other: &Rect) -> Option<Rect> {
		let r = Rect {
			left: self.left.max(other.left),
			top: self.top.max(other.top),
			right: self.right.min(other.right),
			bottom: self.bottom.min(other.bottom),
		};
		if r.is_empty() { None } else { Some(r) }
	}
}

// Which edges of the box a resize moves. Two adjacent edges make a corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grip {
	pub left: bool,
	pub top: bool,
	pub right: bool,
	pub bottom: bool,
}

impl Grip {
	pub fn is_none(&self) -> bool {
		!(self.left || self.top || self.right || self.bottom)
	}
}

// What a press at some point would do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
	// Start a new box (or clear the old one on a plain click).
	Outside,
	Move,
	Resize(Grip),
}

// Keys held during a drag that change how the box is sized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
	// Ctrl: size around the center (resize mirrors the opposite edge; a new box grows out from the press).
	pub center: bool,
	// Shift: keep the box's shape while resizing; a new box is a square.
	pub keep_shape: bool,
}

// One axis of a box being sized, in pixels: from `anchor` to `anchor + extent`, or centered on `anchor` and
// reaching `extent` each way.
#[derive(Clone, Copy, Debug)]
struct Span {
	anchor: f64,
	extent: f64,
	centered: bool,
}

impl Span {
	fn size(&self) -> f64 {
		self.extent.abs() * if self.centered { 2.0 } else { 1.0 }
	}

	// Keeps the direction. A zero extent counts as positive.
	fn set_size(&mut self, size: f64) {
		self.extent = self.extent.signum() * size / if self.centered { 2.0 } else { 1.0 };
	}

	// The largest size that keeps the span inside `0..limit`.
	fn max_size(&self, limit: f64) -> f64 {
		if self.centered {
			2.0 * self.anchor.min(limit - self.anchor)
		} else if self.extent >= 0.0 {
			limit - self.anchor
		} else {
			self.anchor
		}
	}

	fn edges(&self) -> (i32, i32) {
		let (a, b) = if self.centered {
			(self.anchor - self.extent.abs(), self.anchor + self.extent.abs())
		} else {
			(self.anchor, self.anchor + self.extent)
		};
		(a.min(b).round() as i32, a.max(b).round() as i32)
	}
}

#[derive(Clone, Copy, Debug)]
enum Drag {
	Idle,
	// The button is down but the mouse hasn't passed `DRAG_THRESHOLD` yet.
	Pressed {
		start: Point,
		hit: Hit,
	},
	Creating {
		anchor: Point,
	},
	Moving {
		start: Point,
		original: Rect,
	},
	// `active` is `grip` flipped to match the box once an edge crosses the opposite one.
	Resizing {
		start: Point,
		original: Rect,
		grip: Grip,
		active: Grip,
	},
}

pub struct Selection {
	bounds: Rect,
	inner_slop: i32,
	outer_slop: i32,
	rect: Option<Rect>,
	drag: Drag,
	modifiers: Modifiers,
	last: Point, // where the mouse was at the last drag update
	box_id: u32, // changes whenever a new box starts or the box is cleared
}

impl Selection {
	// `inner_slop` and `outer_slop` are how far each edge's grab zone reaches inside and outside the box, in pixels.
	pub fn new(width: i32, height: i32, inner_slop: i32, outer_slop: i32) -> Self {
		Self {
			bounds: Rect {
				left: 0,
				top: 0,
				right: width,
				bottom: height,
			},
			inner_slop,
			outer_slop,
			rect: None,
			drag: Drag::Idle,
			modifiers: Modifiers::default(),
			last: Point { x: 0, y: 0 },
			box_id: 0,
		}
	}

	pub fn rect(&self) -> Option<Rect> {
		self.rect
	}

	// Changes whenever a new box starts or the box is cleared, so animations can jump instead of following.
	pub fn box_id(&self) -> u32 {
		self.box_id
	}

	// The box as it was before the current move or resize.
	pub fn drag_origin(&self) -> Option<Rect> {
		match self.drag {
			Drag::Moving { original, .. } | Drag::Resizing { original, .. } => Some(original),
			_ => None,
		}
	}

	pub fn is_dragging(&self) -> bool {
		!matches!(self.drag, Drag::Idle)
	}

	// What a press at `p` would grab.
	//
	// Each edge's zone reaches `outer_slop` outside the box and up to `inner_slop` inside it. Inside, it shrinks to a
	// third of the box so small boxes always keep a middle area for moving. Corners win over edges because a point
	// in both zones sets both flags, and edges win over moving.
	pub fn hit_test(&self, p: Point) -> Hit {
		let Some(r) = self.rect else { return Hit::Outside };
		let slop = self.outer_slop;
		if p.x < r.left - slop || p.x >= r.right + slop || p.y < r.top - slop || p.y >= r.bottom + slop {
			return Hit::Outside;
		}

		let inner_x = self.inner_slop.min(r.width() / 3);
		let inner_y = self.inner_slop.min(r.height() / 3);
		let grip = Grip {
			left: p.x < r.left + inner_x,
			top: p.y < r.top + inner_y,
			right: p.x >= r.right - inner_x,
			bottom: p.y >= r.bottom - inner_y,
		};
		if grip.is_none() { Hit::Move } else { Hit::Resize(grip) }
	}

	// What the mouse is over or holding, for picking the cursor.
	pub fn active_hit(&self, hover: Point) -> Hit {
		match self.drag {
			Drag::Idle => self.hit_test(hover),
			Drag::Pressed { hit, .. } => hit,
			Drag::Creating { .. } => Hit::Outside,
			Drag::Moving { .. } => Hit::Move,
			Drag::Resizing { active, .. } => Hit::Resize(active),
		}
	}

	// The edges to highlight: the ones being dragged, or the ones under the mouse.
	pub fn highlight(&self, hover: Point) -> Option<Grip> {
		match self.drag {
			Drag::Idle | Drag::Pressed { .. } => match self.active_hit(hover) {
				Hit::Resize(grip) => Some(grip),
				_ => None,
			},
			Drag::Resizing { active, .. } => Some(active),
			Drag::Creating { .. } | Drag::Moving { .. } => None,
		}
	}

	// Moves the box by `(dx, dy)`, stopping at the screen edges. Does nothing mid-drag or without a box.
	pub fn nudge(&mut self, dx: i32, dy: i32) {
		if let (Drag::Idle, Some(rect)) = (self.drag, self.rect) {
			self.rect = Some(self.offset(rect, dx, dy));
		}
	}

	// Updates the held modifier keys, resizing the box right away if one changed mid-drag.
	pub fn set_modifiers(&mut self, modifiers: Modifiers) {
		if modifiers == self.modifiers {
			return;
		}
		self.modifiers = modifiers;
		if self.is_dragging() {
			self.drag_to(self.last);
		}
	}

	pub fn press(&mut self, p: Point) {
		self.drag = Drag::Pressed {
			start: p,
			hit: self.hit_test(p),
		};
	}

	// Mouse moved while the button is down.
	pub fn drag_to(&mut self, p: Point) {
		let p = self.clamp_point(p);
		self.last = p;
		if let Drag::Pressed { start, hit } = self.drag {
			let moved = (p.x - start.x).abs().max((p.y - start.y).abs());
			if moved < DRAG_THRESHOLD {
				return;
			}
			self.drag = match (hit, self.rect) {
				(Hit::Move, Some(original)) => Drag::Moving { start, original },
				(Hit::Resize(grip), Some(original)) => Drag::Resizing {
					start,
					original,
					grip,
					active: grip,
				},
				_ => {
					self.box_id = self.box_id.wrapping_add(1);
					Drag::Creating { anchor: start }
				}
			};
		}

		match self.drag {
			Drag::Idle | Drag::Pressed { .. } => {}
			Drag::Creating { anchor } => {
				let m = self.modifiers;
				let mut x = Span {
					anchor: anchor.x as f64,
					extent: (p.x - anchor.x) as f64,
					centered: m.center,
				};
				let mut y = Span {
					anchor: anchor.y as f64,
					extent: (p.y - anchor.y) as f64,
					centered: m.center,
				};
				if m.keep_shape {
					let side = x.size().max(y.size());
					x.set_size(side);
					y.set_size(side);
				}
				self.rect = Some(self.fit(x, y, m.keep_shape.then_some((1.0, 1.0))).0);
			}
			Drag::Moving { start, original } => {
				self.rect = Some(self.offset(original, p.x - start.x, p.y - start.y));
			}
			Drag::Resizing {
				start, original, grip, ..
			} => {
				let active = self.resize(original, grip, p.x - start.x, p.y - start.y);
				self.drag = Drag::Resizing {
					start,
					original,
					grip,
					active,
				};
			}
		}
	}

	// Resizes `original` by dragging the `grip` edges by `(dx, dy)`, honoring the modifiers. Returns the grip
	// flipped to match the box if an edge crossed the opposite one.
	fn resize(&mut self, original: Rect, grip: Grip, dx: i32, dy: i32) -> Grip {
		let m = self.modifiers;
		let (width, height) = (original.width() as f64, original.height() as f64);
		let center_x = (original.left + original.right) as f64 / 2.0;
		let center_y = (original.top + original.bottom) as f64 / 2.0;
		// A dragged axis runs from the fixed edge (or the center, with Ctrl) to the dragged edge. An axis that
		// isn't dragged is held centered, so with Shift it grows evenly on both sides.
		let axis = |low: bool, high: bool, low_edge: i32, high_edge: i32, delta: i32, center: f64, size: f64| {
			if !(low || high) {
				return Span {
					anchor: center,
					extent: size / 2.0,
					centered: true,
				};
			}
			let dragged = (if low { low_edge } else { high_edge } + delta) as f64;
			let anchor = if m.center {
				center
			} else if low {
				high_edge as f64
			} else {
				low_edge as f64
			};
			Span {
				anchor,
				extent: dragged - anchor,
				centered: m.center,
			}
		};
		let mut x = axis(
			grip.left,
			grip.right,
			original.left,
			original.right,
			dx,
			center_x,
			width,
		);
		let mut y = axis(
			grip.top,
			grip.bottom,
			original.top,
			original.bottom,
			dy,
			center_y,
			height,
		);

		if m.keep_shape {
			// scale by the dragged axis, or by whichever dragged axis grew more relative to its size
			let scale = match (grip.left || grip.right, grip.top || grip.bottom) {
				(true, true) => (x.size() / width).max(y.size() / height),
				(true, false) => x.size() / width,
				_ => y.size() / height,
			};
			x.set_size(scale * width);
			y.set_size(scale * height);
		}
		let (rect, x, y) = self.fit(x, y, m.keep_shape.then_some((width, height)));
		self.rect = Some(rect);

		// the dragged edge has crossed the anchor if its extent points the other way now
		let flip_x = (grip.left && x.extent > 0.0) || (grip.right && x.extent < 0.0);
		let flip_y = (grip.top && y.extent > 0.0) || (grip.bottom && y.extent < 0.0);
		Grip {
			left: if flip_x { grip.right } else { grip.left },
			right: if flip_x { grip.left } else { grip.right },
			top: if flip_y { grip.bottom } else { grip.top },
			bottom: if flip_y { grip.top } else { grip.bottom },
		}
	}

	// Shrinks the spans until they fit on screen and returns the box they make. With `shape` (a width and height),
	// both shrink together so the box keeps that shape.
	fn fit(&self, mut x: Span, mut y: Span, shape: Option<(f64, f64)>) -> (Rect, Span, Span) {
		let max_x = x.max_size(self.bounds.right as f64);
		let max_y = y.max_size(self.bounds.bottom as f64);
		match shape {
			None => {
				x.set_size(x.size().min(max_x));
				y.set_size(y.size().min(max_y));
			}
			Some((width, height)) => {
				let scale = (x.size() / width).min(max_x / width).min(max_y / height);
				x.set_size(scale * width);
				y.set_size(scale * height);
			}
		}
		let ((left, right), (top, bottom)) = (x.edges(), y.edges());
		(
			Rect {
				left,
				top,
				right,
				bottom,
			},
			x,
			y,
		)
	}

	// Button released at `p`.
	pub fn release(&mut self, p: Point) {
		self.drag_to(p);
		match self.drag {
			// a click without dragging clears the box, but only outside it, so a click on the box can't lose it
			Drag::Pressed { hit: Hit::Outside, .. } => self.clear(),
			_ => {
				if self.rect.is_some_and(|r| r.is_empty()) {
					self.clear();
				}
			}
		}
		self.drag = Drag::Idle;
	}

	fn clear(&mut self) {
		self.rect = None;
		self.box_id = self.box_id.wrapping_add(1);
	}

	// `rect` moved by `(dx, dy)`, kept on screen.
	fn offset(&self, rect: Rect, dx: i32, dy: i32) -> Rect {
		let left = (rect.left + dx).clamp(0, self.bounds.right - rect.width());
		let top = (rect.top + dy).clamp(0, self.bounds.bottom - rect.height());
		Rect {
			left,
			top,
			right: left + rect.width(),
			bottom: top + rect.height(),
		}
	}

	fn clamp_point(&self, p: Point) -> Point {
		Point {
			x: p.x.clamp(self.bounds.left, self.bounds.right),
			y: p.y.clamp(self.bounds.top, self.bounds.bottom),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pt(x: i32, y: i32) -> Point {
		Point { x, y }
	}

	fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
		Rect {
			left,
			top,
			right,
			bottom,
		}
	}

	fn drag(sel: &mut Selection, from: Point, to: Point) {
		sel.press(from);
		sel.drag_to(to);
		sel.release(to);
	}

	// a 1000x800 screen with a 100..300 x 100..200 box
	fn with_box() -> Selection {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag(&mut sel, pt(100, 100), pt(300, 200));
		sel
	}

	const GRIP_LEFT: Grip = Grip {
		left: true,
		top: false,
		right: false,
		bottom: false,
	};
	const GRIP_RIGHT: Grip = Grip {
		left: false,
		top: false,
		right: true,
		bottom: false,
	};
	const GRIP_TOP_LEFT: Grip = Grip {
		left: true,
		top: true,
		right: false,
		bottom: false,
	};

	#[test]
	fn drag_creates_box_in_any_direction() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag(&mut sel, pt(300, 200), pt(100, 100));
		assert_eq!(sel.rect(), Some(rect(100, 100, 300, 200)));
	}

	#[test]
	fn small_movement_is_a_click() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag(&mut sel, pt(100, 100), pt(102, 102));
		assert_eq!(sel.rect(), None);
	}

	#[test]
	fn click_outside_clears_box() {
		let mut sel = with_box();
		drag(&mut sel, pt(600, 600), pt(600, 600));
		assert_eq!(sel.rect(), None);
	}

	#[test]
	fn click_inside_keeps_box() {
		let mut sel = with_box();
		drag(&mut sel, pt(200, 150), pt(201, 150));
		assert_eq!(sel.rect(), Some(rect(100, 100, 300, 200)));
	}

	#[test]
	fn drag_outside_replaces_box() {
		let mut sel = with_box();
		drag(&mut sel, pt(500, 500), pt(600, 550));
		assert_eq!(sel.rect(), Some(rect(500, 500, 600, 550)));
	}

	#[test]
	fn drag_inside_moves_box() {
		let mut sel = with_box();
		drag(&mut sel, pt(200, 150), pt(250, 170));
		assert_eq!(sel.rect(), Some(rect(150, 120, 350, 220)));
	}

	#[test]
	fn move_stops_at_screen_edge() {
		let mut sel = with_box();
		drag(&mut sel, pt(200, 150), pt(0, 0));
		assert_eq!(sel.rect(), Some(rect(0, 0, 200, 100)));
		drag(&mut sel, pt(100, 50), pt(999, 799));
		assert_eq!(sel.rect(), Some(rect(800, 700, 1000, 800)));
	}

	#[test]
	fn edge_zones_reach_both_sides() {
		let sel = with_box();
		assert_eq!(sel.hit_test(pt(80, 150)), Hit::Resize(GRIP_LEFT));
		assert_eq!(sel.hit_test(pt(109, 150)), Hit::Resize(GRIP_LEFT));
		assert_eq!(sel.hit_test(pt(110, 150)), Hit::Move);
		assert_eq!(sel.hit_test(pt(79, 150)), Hit::Outside);
		assert_eq!(sel.hit_test(pt(290, 150)), Hit::Resize(GRIP_RIGHT));
		assert_eq!(sel.hit_test(pt(289, 150)), Hit::Move);
		assert_eq!(sel.hit_test(pt(319, 150)), Hit::Resize(GRIP_RIGHT));
		assert_eq!(sel.hit_test(pt(320, 150)), Hit::Outside);
	}

	#[test]
	fn corners_win_over_edges() {
		let sel = with_box();
		assert_eq!(sel.hit_test(pt(95, 95)), Hit::Resize(GRIP_TOP_LEFT));
		assert_eq!(sel.hit_test(pt(105, 105)), Hit::Resize(GRIP_TOP_LEFT));
	}

	#[test]
	fn small_box_keeps_a_middle_to_move() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag(&mut sel, pt(100, 100), pt(106, 106));
		// inner zones shrink to 6 / 3 = 2 px, leaving 102..104 for moving
		assert_eq!(sel.hit_test(pt(103, 103)), Hit::Move);
		assert_eq!(sel.hit_test(pt(101, 103)), Hit::Resize(GRIP_LEFT));
		assert_eq!(sel.hit_test(pt(95, 95)), Hit::Resize(GRIP_TOP_LEFT));
	}

	#[test]
	fn resize_edge() {
		let mut sel = with_box();
		drag(&mut sel, pt(300, 150), pt(400, 160));
		assert_eq!(sel.rect(), Some(rect(100, 100, 400, 200)));
	}

	#[test]
	fn resize_corner() {
		let mut sel = with_box();
		drag(&mut sel, pt(100, 100), pt(50, 40));
		assert_eq!(sel.rect(), Some(rect(50, 40, 300, 200)));
	}

	#[test]
	fn resize_past_opposite_edge_flips() {
		let mut sel = with_box();
		sel.press(pt(100, 150));
		sel.drag_to(pt(400, 150));
		assert_eq!(sel.rect(), Some(rect(300, 100, 400, 200)));
		// the dragged edge is now the right one
		assert_eq!(sel.highlight(pt(400, 150)), Some(GRIP_RIGHT));
		sel.release(pt(400, 150));
		assert_eq!(sel.rect(), Some(rect(300, 100, 400, 200)));
	}

	#[test]
	fn resize_stops_at_screen_edge() {
		let mut sel = with_box();
		drag(&mut sel, pt(300, 150), pt(5000, 150));
		assert_eq!(sel.rect(), Some(rect(100, 100, 1000, 200)));
	}

	#[test]
	fn zero_size_box_is_cleared() {
		let mut sel = with_box();
		drag(&mut sel, pt(100, 150), pt(300, 150));
		assert_eq!(sel.rect(), None);
	}

	#[test]
	fn nudge_moves_box_and_stops_at_edge() {
		let mut sel = with_box();
		sel.nudge(-1, 1);
		assert_eq!(sel.rect(), Some(rect(99, 101, 299, 201)));
		sel.nudge(-500, 0);
		assert_eq!(sel.rect(), Some(rect(0, 101, 200, 201)));
	}

	#[test]
	fn nudge_ignored_mid_drag() {
		let mut sel = with_box();
		sel.press(pt(200, 150));
		sel.nudge(1, 0);
		sel.release(pt(200, 150));
		assert_eq!(sel.rect(), Some(rect(100, 100, 300, 200)));
	}

	const CTRL: Modifiers = Modifiers {
		center: true,
		keep_shape: false,
	};
	const SHIFT: Modifiers = Modifiers {
		center: false,
		keep_shape: true,
	};
	const CTRL_SHIFT: Modifiers = Modifiers {
		center: true,
		keep_shape: true,
	};

	fn drag_with(sel: &mut Selection, modifiers: Modifiers, from: Point, to: Point) {
		sel.set_modifiers(modifiers);
		drag(sel, from, to);
	}

	#[test]
	fn ctrl_draws_from_center() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag_with(&mut sel, CTRL, pt(500, 400), pt(550, 420));
		assert_eq!(sel.rect(), Some(rect(450, 380, 550, 420)));
	}

	#[test]
	fn shift_draws_square_in_drag_direction() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag_with(&mut sel, SHIFT, pt(100, 100), pt(150, 120));
		assert_eq!(sel.rect(), Some(rect(100, 100, 150, 150)));
		drag_with(&mut sel, SHIFT, pt(500, 500), pt(460, 490));
		assert_eq!(sel.rect(), Some(rect(460, 460, 500, 500)));
	}

	#[test]
	fn shift_square_stays_square_at_screen_edge() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		drag_with(&mut sel, SHIFT, pt(980, 100), pt(1100, 160));
		assert_eq!(sel.rect(), Some(rect(980, 100, 1000, 120)));
	}

	#[test]
	fn ctrl_resize_mirrors_opposite_edge() {
		let mut sel = with_box();
		drag_with(&mut sel, CTRL, pt(300, 150), pt(320, 150));
		assert_eq!(sel.rect(), Some(rect(80, 100, 320, 200)));
	}

	#[test]
	fn ctrl_resize_stops_both_sides_at_screen_edge() {
		let mut sel = with_box();
		drag_with(&mut sel, CTRL, pt(300, 150), pt(900, 150));
		assert_eq!(sel.rect(), Some(rect(0, 100, 400, 200)));
	}

	#[test]
	fn shift_corner_keeps_shape() {
		let mut sel = with_box();
		drag_with(&mut sel, SHIFT, pt(300, 200), pt(500, 220));
		assert_eq!(sel.rect(), Some(rect(100, 100, 500, 300)));
	}

	#[test]
	fn shift_edge_grows_other_axis_evenly() {
		let mut sel = with_box();
		drag_with(&mut sel, SHIFT, pt(300, 150), pt(500, 150));
		assert_eq!(sel.rect(), Some(rect(100, 50, 500, 250)));
	}

	#[test]
	fn ctrl_shift_corner_keeps_shape_around_center() {
		let mut sel = with_box();
		drag_with(&mut sel, CTRL_SHIFT, pt(300, 200), pt(400, 200));
		assert_eq!(sel.rect(), Some(rect(0, 50, 400, 250)));
	}

	#[test]
	fn modifier_change_applies_mid_drag() {
		let mut sel = Selection::new(1000, 800, 10, 20);
		sel.press(pt(500, 400));
		sel.drag_to(pt(550, 420));
		assert_eq!(sel.rect(), Some(rect(500, 400, 550, 420)));
		sel.set_modifiers(CTRL);
		assert_eq!(sel.rect(), Some(rect(450, 380, 550, 420)));
		sel.set_modifiers(Modifiers::default());
		assert_eq!(sel.rect(), Some(rect(500, 400, 550, 420)));
	}

	#[test]
	fn box_id_changes_for_new_boxes_only() {
		let mut sel = with_box();
		let id = sel.box_id();
		drag(&mut sel, pt(200, 150), pt(250, 170)); // move
		drag(&mut sel, pt(300, 150), pt(320, 150)); // resize
		assert_eq!(sel.box_id(), id);
		drag(&mut sel, pt(600, 600), pt(700, 700)); // new box
		assert_ne!(sel.box_id(), id);
	}

	#[test]
	fn drag_origin_is_the_box_before_the_drag() {
		let mut sel = with_box();
		sel.press(pt(200, 150));
		sel.drag_to(pt(250, 170));
		assert_eq!(sel.drag_origin(), Some(rect(100, 100, 300, 200)));
		sel.release(pt(250, 170));
		assert_eq!(sel.drag_origin(), None);
	}
}
