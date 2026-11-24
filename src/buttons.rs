// buttons

// The copy, save and close buttons under the box: white icons drawn as shapes, 50% white and fully white on hover,
// each clickable across its whole square. The icons are made once, at 100% display scaling, and scaled up when drawn.

use windows::Win32::Graphics::Direct2D::Common::{
	D2D_RECT_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
};
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1Factory, ID2D1PathGeometry, ID2D1RenderTarget,
	ID2D1SolidColorBrush,
};
use windows::core::Result;
use windows_numerics::{Matrix3x2, Vector2};

use crate::render::scaled;
use crate::selection::{Point, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
	Copy,
	Save,
	Close,
}

// Left to right.
const BUTTONS: [Button; 3] = [Button::Copy, Button::Save, Button::Close];

// Sizes in pixels at 100% display scaling. The icons are drawn in a `SIZE` square.
const SIZE: i32 = 24;
const GAP: i32 = 4; // between buttons
const STROKE: f32 = 1.0;

const OPACITY: f32 = 0.5;
const HOVER_OPACITY: f32 = 1.0;

pub struct Buttons {
	scale: f32,
	size: i32,
	gap: i32,
	file: ID2D1PathGeometry, // the save icon's page with its folded corner
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

	// Draws the row at `row` at `opacity`, with `hovered` lit up. Leaves per-primitive antialiasing on and the
	// transform reset.
	pub fn draw(
		&self,
		render_target: &ID2D1RenderTarget,
		white: &ID2D1SolidColorBrush,
		row: Rect,
		hovered: Option<Button>,
		opacity: f32,
	) {
		if opacity <= 0.0 {
			return;
		}
		unsafe {
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
			for (button, rect) in place(row, self.size, self.gap) {
				// the button's square in units of 100% scaling, so the icons below scale with it
				render_target.SetTransform(&Matrix3x2 {
					M11: self.scale,
					M12: 0.0,
					M21: 0.0,
					M22: self.scale,
					M31: rect.left as f32,
					M32: rect.top as f32,
				});
				let shown = if hovered == Some(button) {
					HOVER_OPACITY
				} else {
					OPACITY
				};
				white.SetOpacity(shown * opacity);
				match button {
					// a clipboard: the board, with its clip over the top edge
					Button::Copy => {
						render_target.DrawRoundedRectangle(&rounded(7.5, 6.5, 16.5, 18.5, 1.5), white, STROKE, None);
						render_target.FillRoundedRectangle(&rounded(9.5, 5.0, 14.5, 8.5, 1.0), white);
					}
					Button::Save => render_target.DrawGeometry(&self.file, white, STROKE, None),
					// an x
					Button::Close => {
						render_target.DrawLine(v(8.0, 8.0), v(16.0, 16.0), white, STROKE * 1.5, None);
						render_target.DrawLine(v(16.0, 8.0), v(8.0, 16.0), white, STROKE * 1.5, None);
					}
				}
			}
			render_target.SetTransform(&Matrix3x2::identity());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn buttons_are_copy_save_close_left_to_right() {
		assert_eq!(row_width(24, 4), 80);
		let row = Rect {
			left: 100,
			top: 100,
			right: 180,
			bottom: 124,
		};
		let hit = |p| button_at(row, 24, 4, p);
		assert_eq!(hit(Point { x: 100, y: 100 }), Some(Button::Copy));
		assert_eq!(hit(Point { x: 130, y: 110 }), Some(Button::Save));
		assert_eq!(hit(Point { x: 179, y: 123 }), Some(Button::Close));
		// the gap between buttons
		assert_eq!(hit(Point { x: 125, y: 110 }), None);
	}
}
