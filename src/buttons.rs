// buttons

// The undo, redo, copy, save and clear buttons under the box, and the close and settings buttons in the screen's
// top-right corner: white icons drawn as shapes, 50% white and fully white on hover, each clickable across its whole
// square. Undo and redo dim with nothing to undo or redo. The icons are made once, at 100% display scaling, and scaled
// up when drawn.

use windows::Win32::Graphics::Direct2D::Common::{
	D2D_RECT_F, D2D_SIZE_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
};
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_ELLIPSE, D2D1_ROUNDED_RECT,
	D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, ID2D1Factory, ID2D1PathGeometry,
	ID2D1RenderTarget,
};
use windows::core::Result;
use windows_numerics::{Matrix3x2, Vector2};

use crate::gpu::Paint;
use crate::render::scaled;
use crate::selection::{Point, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
	Undo,
	Redo,
	Copy,
	Save,
	Clear, // the box
	Close, // the overlay, in the top-right corner
	Gear,  // the settings menu, left of the close button
}

// The row under the box, left to right.
const BUTTONS: [Button; 5] = [Button::Undo, Button::Redo, Button::Copy, Button::Save, Button::Clear];

// Sizes in pixels at 100% display scaling. The icons are drawn in a `SIZE` square.
const SIZE: i32 = 24;
const GAP: i32 = 4; // between buttons
const CLOSE_MARGIN: i32 = 8; // from the close button to the screen's top and right edges
const STROKE: f32 = 1.0;
const OUTLINE: f32 = 1.5; // how far the black stroke under each icon reaches past its lines, so it shows on bright boxes

const OPACITY: f32 = 0.5;
const HOVER_OPACITY: f32 = 1.0;
const DISABLED_OPACITY: f32 = 0.2; // undo or redo with nothing to undo or redo

// Whether there's anything to undo and to redo.
#[derive(Clone, Copy, Debug, Default)]
pub struct History {
	pub undo: bool,
	pub redo: bool,
}

impl History {
	fn allows(self, button: Button) -> bool {
		match button {
			Button::Undo => self.undo,
			Button::Redo => self.redo,
			_ => true,
		}
	}
}

pub struct Buttons {
	scale: f32,
	size: i32,
	gap: i32,
	file: ID2D1PathGeometry, // the save icon's page with its folded corner
	undo: ID2D1PathGeometry, // an arrow pointing left that curves back around under itself
	redo: ID2D1PathGeometry, // the same, mirrored
}

fn v(x: f32, y: f32) -> Vector2 {
	Vector2 { X: x, Y: y }
}

fn rounded(left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> D2D1_ROUNDED_RECT {
	D2D1_ROUNDED_RECT {
		rect: D2D_RECT_F {
			left,
			top,
			right,
			bottom,
		},
		radiusX: radius,
		radiusY: radius,
	}
}

// Each button with where it is, for a row at `row` of `size` squares `gap` apart.
fn place(row: Rect, size: i32, gap: i32) -> impl Iterator<Item = (Button, Rect)> {
	BUTTONS.into_iter().enumerate().map(move |(i, button)| {
		let left = row.left + i as i32 * (size + gap);
		(
			button,
			Rect {
				left,
				top: row.top,
				right: left + size,
				bottom: row.top + size,
			},
		)
	})
}

// The button under `p`, for a row at `row` of `size` squares `gap` apart.
fn button_at(row: Rect, size: i32, gap: i32, p: Point) -> Option<Button> {
	place(row, size, gap)
		.find(|(_, rect)| rect.contains(p))
		.map(|(button, _)| button)
}

// The width of a row of `size` squares `gap` apart.
fn row_width(size: i32, gap: i32) -> i32 {
	BUTTONS.len() as i32 * size + (BUTTONS.len() as i32 - 1) * gap
}

// The undo arrow: pointing left along the top, then curving down around the right and back under itself. `mirror`
// flips it into the redo arrow.
fn arrow(d2d: &ID2D1Factory, mirror: bool) -> Result<ID2D1PathGeometry> {
	let x = |x: f32| if mirror { SIZE as f32 - x } else { x };
	unsafe {
		let geometry = d2d.CreatePathGeometry()?;
		let sink = geometry.Open()?;
		// the head
		sink.BeginFigure(v(x(10.5), 6.5), D2D1_FIGURE_BEGIN_HOLLOW);
		sink.AddLines(&[v(x(7.5), 9.5), v(x(10.5), 12.5)]);
		sink.EndFigure(D2D1_FIGURE_END_OPEN);
		// the shaft
		sink.BeginFigure(v(x(7.5), 9.5), D2D1_FIGURE_BEGIN_HOLLOW);
		sink.AddLine(v(x(12.5), 9.5));
		sink.AddArc(&D2D1_ARC_SEGMENT {
			point: v(x(12.5), 17.5),
			size: D2D_SIZE_F {
				width: 4.0,
				height: 4.0,
			},
			rotationAngle: 0.0,
			sweepDirection: if mirror {
				D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
			} else {
				D2D1_SWEEP_DIRECTION_CLOCKWISE
			},
			arcSize: D2D1_ARC_SIZE_SMALL,
		});
		sink.AddLine(v(x(9.5), 17.5));
		sink.EndFigure(D2D1_FIGURE_END_OPEN);
		sink.Close()?;
		Ok(geometry)
	}
}

impl Buttons {
	pub fn new(d2d: &ID2D1Factory, scale: f32) -> Result<Self> {
		// a page with its top-right corner folded down, and the fold
		let file = unsafe {
			let geometry = d2d.CreatePathGeometry()?;
			let sink = geometry.Open()?;
			sink.BeginFigure(v(7.5, 5.5), D2D1_FIGURE_BEGIN_HOLLOW);
			sink.AddLines(&[v(13.5, 5.5), v(16.5, 8.5), v(16.5, 18.5), v(7.5, 18.5)]);
			sink.EndFigure(D2D1_FIGURE_END_CLOSED);
			sink.BeginFigure(v(13.5, 5.5), D2D1_FIGURE_BEGIN_HOLLOW);
			sink.AddLines(&[v(13.5, 8.5), v(16.5, 8.5)]);
			sink.EndFigure(D2D1_FIGURE_END_OPEN);
			sink.Close()?;
			geometry
		};
		Ok(Self {
			scale,
			size: scaled(SIZE, scale),
			gap: scaled(GAP, scale),
			file,
			undo: arrow(d2d, false)?,
			redo: arrow(d2d, true)?,
		})
	}

	// The size of the whole row, in pixels.
	pub fn width(&self) -> i32 {
		row_width(self.size, self.gap)
	}

	pub fn height(&self) -> i32 {
		self.size
	}

	// The button under `p`, for a row placed at `row`.
	pub fn hit(&self, row: Rect, p: Point) -> Option<Button> {
		button_at(row, self.size, self.gap, p)
	}

	// Where the close button is drawn on a screen `screen_width` wide.
	pub fn close_rect(&self, screen_width: i32) -> Rect {
		let margin = scaled(CLOSE_MARGIN, self.scale);
		Rect {
			left: screen_width - margin - self.size,
			top: margin,
			right: screen_width - margin,
			bottom: margin + self.size,
		}
	}

	// Where the close button takes clicks: its square stretched up and right to the screen's edges, so the very
	// corner pixel hits it.
	pub fn close_hit_rect(&self, screen_width: i32) -> Rect {
		Rect {
			top: 0,
			right: screen_width,
			..self.close_rect(screen_width)
		}
	}

	// Where the settings button is drawn on a screen `screen_width` wide: left of the close button.
	pub fn gear_rect(&self, screen_width: i32) -> Rect {
		let close = self.close_rect(screen_width);
		Rect {
			left: close.left - self.gap - self.size,
			right: close.left - self.gap,
			..close
		}
	}

	// Where the settings button takes clicks: its square stretched up to the screen's edge.
	pub fn gear_hit_rect(&self, screen_width: i32) -> Rect {
		Rect {
			top: 0,
			..self.gear_rect(screen_width)
		}
	}

	// Draws the row at `row` at `opacity`, with `hovered` lit up and undo and redo dimmed if `history` has nothing for
	// them, each icon over a black stroke. Leaves per-primitive antialiasing on, the transform reset and the brushes at
	// full opacity.
	pub fn draw(
		&self,
		render_target: &ID2D1RenderTarget,
		paint: &Paint,
		row: Rect,
		hovered: Option<Button>,
		history: History,
		opacity: f32,
	) {
		if opacity <= 0.0 {
			return;
		}
		for (button, rect) in place(row, self.size, self.gap) {
			let enabled = history.allows(button);
			self.draw_one(
				render_target,
				paint,
				button,
				rect,
				hovered == Some(button),
				enabled,
				opacity,
			);
		}
	}

	// Draws `button` in `rect` at `opacity`, lit up if `hovered` or dimmed if not `enabled`, over a black stroke. Leaves
	// per-primitive antialiasing on, the transform reset and the brushes at full opacity.
	#[allow(clippy::too_many_arguments)]
	pub fn draw_one(
		&self,
		render_target: &ID2D1RenderTarget,
		paint: &Paint,
		button: Button,
		rect: Rect,
		hovered: bool,
		enabled: bool,
		opacity: f32,
	) {
		if opacity <= 0.0 {
			return;
		}
		unsafe {
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
			// the button's square in units of 100% scaling, so the icons below scale with it
			render_target.SetTransform(&Matrix3x2 {
				M11: self.scale,
				M12: 0.0,
				M21: 0.0,
				M22: self.scale,
				M31: rect.left as f32,
				M32: rect.top as f32,
			});
			let shown = match (enabled, hovered) {
				(false, _) => DISABLED_OPACITY,
				(true, true) => HOVER_OPACITY,
				(true, false) => OPACITY,
			};
			let square = D2D_RECT_F {
				left: 0.0,
				top: 0.0,
				right: SIZE as f32,
				bottom: SIZE as f32,
			};
			paint.white.SetOpacity(1.0);
			paint.black.SetOpacity(1.0);
			paint.with_opacity(render_target, square, shown * opacity, || {
				// the black stroke first: the same lines `2 * OUTLINE` wider, and filled shapes outlined
				for (brush, extra, style) in [
					(&paint.black, 2.0 * OUTLINE, Some(&paint.round)),
					(&paint.white, 0.0, None),
				] {
					match button {
						// a clipboard: the board, with its clip over the top edge
						Button::Copy => {
							let clip = rounded(9.5, 5.0, 14.5, 8.5, 1.0);
							let board = rounded(7.5, 6.5, 16.5, 18.5, 1.5);
							render_target.DrawRoundedRectangle(&board, brush, STROKE + extra, style);
							if extra > 0.0 {
								render_target.DrawRoundedRectangle(&clip, brush, extra, style);
							}
							render_target.FillRoundedRectangle(&clip, brush);
						}
						Button::Save => render_target.DrawGeometry(&self.file, brush, STROKE + extra, style),
						Button::Undo => render_target.DrawGeometry(&self.undo, brush, STROKE + extra, style),
						Button::Redo => render_target.DrawGeometry(&self.redo, brush, STROKE + extra, style),
						// a ring with eight teeth around a hole
						Button::Gear => {
							let center = v(12.0, 12.0);
							let circle = |radius: f32| D2D1_ELLIPSE {
								point: center,
								radiusX: radius,
								radiusY: radius,
							};
							render_target.DrawEllipse(&circle(4.5), brush, STROKE + extra, style);
							render_target.DrawEllipse(&circle(1.5), brush, STROKE + extra, style);
							for tooth in 0..8 {
								let angle = tooth as f32 * std::f32::consts::FRAC_PI_4;
								let at = |radius: f32| v(12.0 + radius * angle.cos(), 12.0 + radius * angle.sin());
								render_target.DrawLine(at(4.5), at(7.0), brush, STROKE * 2.0 + extra, style);
							}
						}
						// an x
						Button::Clear | Button::Close => {
							render_target.DrawLine(v(8.0, 8.0), v(16.0, 16.0), brush, STROKE * 1.5 + extra, style);
							render_target.DrawLine(v(16.0, 8.0), v(8.0, 16.0), brush, STROKE * 1.5 + extra, style);
						}
					}
				}
			});
			render_target.SetTransform(&Matrix3x2::identity());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn buttons_are_undo_redo_copy_save_clear_left_to_right() {
		assert_eq!(row_width(24, 4), 136);
		let row = Rect {
			left: 100,
			top: 100,
			right: 236,
			bottom: 124,
		};
		let hit = |p| button_at(row, 24, 4, p);
		assert_eq!(hit(Point { x: 100, y: 100 }), Some(Button::Undo));
		assert_eq!(hit(Point { x: 130, y: 110 }), Some(Button::Redo));
		assert_eq!(hit(Point { x: 160, y: 110 }), Some(Button::Copy));
		assert_eq!(hit(Point { x: 190, y: 110 }), Some(Button::Save));
		assert_eq!(hit(Point { x: 235, y: 123 }), Some(Button::Clear));
		// the gap between buttons
		assert_eq!(hit(Point { x: 125, y: 110 }), None);
	}
}
