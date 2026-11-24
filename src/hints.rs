// hints

// The control hints in the bottom-left corner of the overlay: key icons next to what they do, in 50% white with no
// background. Laid out once when the overlay opens as a list of shapes that each frame draws.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT, ID2D1RenderTarget,
	ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextLayout};
use windows::core::Result;
use windows_numerics::Vector2;

use crate::gpu::{text_format, text_layout};
use crate::selection::Rect;

enum Key {
	Mouse,
	Text(&'static str),
}

const ROWS: &[(&[Key], &str)] = &[
	(&[Key::Mouse], "Select an area"),
	(&[Key::Text("Ctrl")], "Size from the center"),
	(&[Key::Text("Shift")], "Keep the shape (square)"),
	(
		&[Key::Text("←"), Key::Text("↑"), Key::Text("↓"), Key::Text("→")],
		"Nudge 1 px",
	),
	(&[Key::Text("Ctrl"), Key::Text("C")], "Copy"),
	(&[Key::Text("Ctrl"), Key::Text("S")], "Save"),
	(&[Key::Text("PrtSc")], "Copy the whole screen"),
	(&[Key::Text("Esc")], "Close"),
];

// Sizes in pixels at 100% display scaling.
const MARGIN: f32 = 16.0; // from the screen's bottom-left corner
const PADDING: f32 = 10.0; // around the hints, for fading them when the mouse is near
const KEY_HEIGHT: f32 = 20.0;
const KEY_PADDING: f32 = 6.0; // between a key's label and its sides
const KEY_RADIUS: f32 = 4.0;
const KEY_GAP: f32 = 3.0; // between keys in a row
const ROW_GAP: f32 = 5.0;
const TEXT_GAP: f32 = 10.0; // between the keys and the description
const KEY_FONT_SIZE: f32 = 11.0;
const TEXT_FONT_SIZE: f32 = 13.0;
const MOUSE_WIDTH: f32 = 12.0;
const MOUSE_HEIGHT: f32 = 18.0;
const MOUSE_WHEEL_WIDTH: f32 = 2.0;
const MOUSE_WHEEL_HEIGHT: f32 = 4.0;
const MOUSE_WHEEL_TOP: f32 = 3.0;

const OPACITY: f32 = 0.5; // the icons and text are 50% white
const KEY_FILL_OPACITY: f32 = 0.15; // relative to `OPACITY`

enum Shape {
	Fill(D2D1_ROUNDED_RECT, f32),    // with its opacity
	Outline(D2D1_ROUNDED_RECT, f32), // with its stroke width
	Text(Vector2, IDWriteTextLayout),
}

pub struct Hints {
	pub rect: Rect, // the hints plus some padding, in screen pixels
	shapes: Vec<Shape>,
}

fn rounded(left: f32, top: f32, width: f32, height: f32, radius: f32) -> D2D1_ROUNDED_RECT {
	D2D1_ROUNDED_RECT {
		rect: D2D_RECT_F {
			left,
			top,
			right: left + width,
			bottom: top + height,
		},
		radiusX: radius,
		radiusY: radius,
	}
}

// A shape's outline is centered on its edge, so pulling it in by half the stroke keeps the line inside the shape and
// on whole pixels.
fn inset(shape: D2D1_ROUNDED_RECT, by: f32) -> D2D1_ROUNDED_RECT {
	let r = shape.rect;
	D2D1_ROUNDED_RECT {
		rect: D2D_RECT_F {
			left: r.left + by,
			top: r.top + by,
			right: r.right - by,
			bottom: r.bottom - by,
		},
		..shape
	}
}

impl Hints {
	pub fn new(write: &IDWriteFactory, scale: f32, screen_height: i32) -> Result<Self> {
		let s = |size: f32| (size * scale).round();
		let key_format = text_format(write, s(KEY_FONT_SIZE))?;
		let text_format = text_format(write, s(TEXT_FONT_SIZE))?;
		let key_height = s(KEY_HEIGHT);
		let stroke = s(1.0).max(1.0);

		// each key as its width and label (none for the mouse), then each row's description
		let mut rows = Vec::new();
		for (keys, text) in ROWS {
			let mut caps = Vec::new();
			for key in *keys {
				caps.push(match key {
					Key::Mouse => (s(MOUSE_WIDTH), None),
					Key::Text(label) => {
						let (layout, width, height) = text_layout(write, &key_format, label)?;
						(
							(width + 2.0 * s(KEY_PADDING)).round().max(key_height),
							Some((layout, width, height)),
						)
					}
				});
			}
			rows.push((caps, text_layout(write, &text_format, text)?));
		}

		let key_column = rows
			.iter()
			.map(|(caps, _)| caps.iter().map(|(width, _)| width).sum::<f32>() + s(KEY_GAP) * (caps.len() as f32 - 1.0))
			.fold(0.0, f32::max);
		let text_column = rows.iter().map(|(_, (_, width, _))| *width).fold(0.0, f32::max);
		let row_count = rows.len() as f32;
		let width = 2.0 * s(PADDING) + key_column + s(TEXT_GAP) + text_column;
		let height = 2.0 * s(PADDING) + row_count * key_height + (row_count - 1.0) * s(ROW_GAP);
		let left = s(MARGIN);
		let top = screen_height as f32 - s(MARGIN) - height;

		let mut shapes = Vec::new();
		for (i, (caps, (text, _, text_height))) in rows.into_iter().enumerate() {
			let y = top + s(PADDING) + i as f32 * (key_height + s(ROW_GAP));
			let mut x = left + s(PADDING);
			for (key_width, label) in caps {
				match label {
					// a key cap with its label centered
					Some((layout, label_width, label_height)) => {
						let cap = rounded(x, y, key_width, key_height, s(KEY_RADIUS));
						shapes.push(Shape::Fill(cap, KEY_FILL_OPACITY));
						shapes.push(Shape::Outline(inset(cap, stroke / 2.0), stroke));
						let origin = Vector2 {
							X: (x + (key_width - label_width) / 2.0).round(),
							Y: (y + (key_height - label_height) / 2.0).round(),
						};
						shapes.push(Shape::Text(origin, layout));
					}
					// the mouse: an outlined body with a wheel, centered in the row
					None => {
						let (mouse_width, mouse_height) = (s(MOUSE_WIDTH), s(MOUSE_HEIGHT));
						let mouse_y = (y + (key_height - mouse_height) / 2.0).round();
						let body = rounded(x, mouse_y, mouse_width, mouse_height, mouse_width / 2.0);
						shapes.push(Shape::Outline(inset(body, stroke / 2.0), stroke));
						let wheel_width = s(MOUSE_WHEEL_WIDTH).max(2.0);
						let wheel_x = x + ((mouse_width - wheel_width) / 2.0).round();
						let wheel_y = mouse_y + s(MOUSE_WHEEL_TOP);
						let wheel = rounded(wheel_x, wheel_y, wheel_width, s(MOUSE_WHEEL_HEIGHT), wheel_width / 2.0);
						shapes.push(Shape::Fill(wheel, 1.0));
					}
				}
				x += key_width + s(KEY_GAP);
			}
			let origin = Vector2 {
				X: left + s(PADDING) + key_column + s(TEXT_GAP),
				Y: (y + (key_height - text_height) / 2.0).round(),
			};
			shapes.push(Shape::Text(origin, text));
		}

		let rect = Rect {
			left: left as i32,
			top: top as i32,
			right: (left + width).ceil() as i32,
			bottom: (top + height).ceil() as i32,
		};
		Ok(Self { rect, shapes })
	}

	// Draws the hints at `opacity` (1 is their normal look), for fading them. Leaves per-primitive antialiasing on.
	pub fn draw(&self, render_target: &ID2D1RenderTarget, white: &ID2D1SolidColorBrush, opacity: f32) {
		if opacity <= 0.0 {
			return;
		}
		unsafe {
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
			for shape in &self.shapes {
				match shape {
					Shape::Fill(rect, fill_opacity) => {
						white.SetOpacity(OPACITY * fill_opacity * opacity);
						render_target.FillRoundedRectangle(rect, white);
					}
					Shape::Outline(rect, width) => {
						white.SetOpacity(OPACITY * opacity);
						render_target.DrawRoundedRectangle(rect, white, *width, None);
					}
					Shape::Text(origin, layout) => {
						white.SetOpacity(OPACITY * opacity);
						render_target.DrawTextLayout(*origin, layout, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
					}
				}
			}
		}
	}
}
