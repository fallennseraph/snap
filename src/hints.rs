// hints

// The control hints in the bottom-left corner of the overlay: key icons next to what they do, in 50% white with no
// background. Each row shows only when its control does something right now (the arrows only with a box, or Enter and
// Esc while asking whether to quit, for example) and says what it would do (the mouse row follows what's under the
// mouse), fading in and out while the rows above
// slide to make room. The mouse row crossfades when its text changes; the others switch at once. A key lights up fully
// while it's held. Laid out once when the overlay opens as lists of shapes that each frame draws.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT,
	ID2D1Factory, ID2D1Geometry, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextLayout};
use windows::core::Result;
use windows_numerics::{Matrix3x2, Vector2};

use crate::anim::{Tween, cubic_out};
use crate::config::Keys;
use crate::confirm::Choice;
use crate::gpu::{Paint, text_format, text_layout, text_outlines};
use crate::keys::Chord;
use crate::selection::Rect;

enum Key {
	Mouse,
	Text(String, Vec<u16>), // with the virtual-key codes that press it
	Or,                     // a "/" between two shortcuts for the same thing, with no cap
}

// What the mouse would act on: what's under it, or what it's dragging.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pointer {
	Outside, // starts a new box
	Move,
	Resize,
	Copy,
	Save,
	Clear,
	Close,
	Choice(Choice), // a button of the quit confirmation
}

// What the overlay is showing, which decides the controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
	Box,  // the screenshot with the box and its buttons
	Quit, // asking whether to quit snap, with the box hidden
}

// What the hints describe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Context {
	pub mode: Mode,
	pub has_box: bool,
	pub pointer: Pointer,
}

// When a row (or one version of it) shows.
type When = fn(Context) -> bool;

// 1 if `when` holds, 0 if not, as a target for a fade.
fn target(when: When, context: Context) -> f32 {
	if when(context) { 1.0 } else { 0.0 }
}

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
const MOUSE_BUTTON_HEIGHT: f32 = 7.0; // from the top of the mouse to the bottom of its left button
const OUTLINE: f32 = 1.5; // how far the black stroke under everything reaches past it, so it shows on bright areas

const OPACITY: f32 = 0.5; // the hints are drawn at 50%, with their strokes
const HELD_OPACITY: f32 = 1.0; // a key while it's held
const KEY_FILL_OPACITY: f32 = 0.15; // relative to the key's opacity
const VK_LBUTTON: u16 = 0x01;

const ROW_FADE: Duration = Duration::from_millis(200); // rows fading in and out, and the rows above sliding

enum Shape {
	Fill(D2D1_ROUNDED_RECT, f32),                         // with its opacity
	ClippedFill(D2D1_ROUNDED_RECT, D2D_RECT_F),           // fully filled, only inside the rect
	Outline(D2D1_ROUNDED_RECT, f32),                      // with its stroke width
	Text(Vector2, IDWriteTextLayout, Vec<ID2D1Geometry>), // with its glyph outlines, for the black stroke
}

// One key of a row, or its description (which no key presses).
struct Part {
	codes: Vec<u16>, // the virtual-key codes that light it up
	shapes: Vec<Shape>,
	held: bool, // as of the last `update`
}

// One version of a row, like "Save" with a box and "Save the whole screen" without. Its shapes are relative to the
// row's top.
struct Variant {
	parts: Vec<Part>,
	when: When,
	shown: Tween, // 1 while it shows, crossfading with the row's other versions
	opacity: f32, // as of the last `update`, with the row's fade
}

struct Row {
	variants: Vec<Variant>, // the one showing is kept while the row fades out
	presence: Tween,        // 1 while any version shows; fades the row and makes room for it
	top: f32,               // as of the last `update`
}

impl Row {
	fn target(&self, context: Context) -> f32 {
		self.variants
			.iter()
			.map(|variant| target(variant.when, context))
			.fold(0.0, f32::max)
	}
}

pub struct Hints {
	pub rect: Rect, // the shown rows plus some padding, in screen pixels, as of the last `update`
	rows: Vec<Row>,
	left: f32,   // of the rows
	bottom: f32, // of the bottom row
	width: f32,  // of the rows
	row_height: f32,
	row_gap: f32,
	padding: f32,
	outline: f32, // `OUTLINE` at the display scaling
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

fn caps(chord: Chord) -> Vec<Key> {
	(chord.caps().into_iter())
		.map(|(label, codes)| Key::Text(label, codes))
		.collect()
}

// One version of a row: its keys, description and when it shows.
type Version = (Vec<Key>, &'static str, When);

// Each row as whether it crossfades between its versions, then the versions.
fn row_specs(keys: &Keys) -> Vec<(bool, Vec<Version>)> {
	use crate::confirm::Choice::{Primary, Secondary};
	use Mode::*;
	use Pointer::*;
	let text = |label: &str, code: u16| Key::Text(label.to_string(), vec![code]);
	// without a box, copying copies the whole screen, so both keys go on one row
	let mut copy_all = caps(keys.copy);
	if keys.copy_whole != keys.copy {
		copy_all.push(Key::Or);
		copy_all.extend(caps(keys.copy_whole));
	}
	let mouse = |text: &'static str, when: When| (vec![Key::Mouse], text, when);
	// a click that does the same as a key shares the key's row, written "mouse / key", and the key's row hides
	let mouse_or = |chord: Chord, text: &'static str, when: When| {
		let mut row_keys = vec![Key::Mouse, Key::Or];
		row_keys.extend(caps(chord));
		(row_keys, text, when)
	};
	// `close` only closes with a box when `cancel` (which clears it) is on another key; otherwise the x's mouse row has no
	// key with a box
	let (close_x, plain_close_x, close_row): (When, When, When) = if keys.close != keys.cancel {
		(
			|c| c.mode == Box && c.pointer == Close,
			|_| false,
			|c| c.mode == Box && c.pointer != Close,
		)
	} else {
		(
			|c| c.mode == Box && c.pointer == Close && !c.has_box,
			|c| c.mode == Box && c.pointer == Close && c.has_box,
			|c| c.mode == Box && !c.has_box && c.pointer != Close,
		)
	};
	vec![
		(
			true,
			vec![
				mouse("Select an area", |c| {
					c.mode == Box && c.pointer == Outside && !c.has_box
				}),
				mouse("Select a new area", |c| {
					c.mode == Box && c.pointer == Outside && c.has_box
				}),
				mouse("Move the box", |c| c.mode == Box && c.pointer == Move),
				mouse("Resize the box", |c| c.mode == Box && c.pointer == Resize),
				mouse_or(keys.copy, "Copy", |c| c.mode == Box && c.pointer == Copy),
				mouse_or(keys.save, "Save", |c| c.mode == Box && c.pointer == Save),
				mouse_or(keys.cancel, "Clear the box", |c| c.mode == Box && c.pointer == Clear),
				mouse_or(keys.close, "Close", close_x),
				mouse("Close", plain_close_x),
				mouse_or(keys.confirm, "Quit snap", |c| {
					c.mode == Quit && c.pointer == Choice(Primary)
				}),
				mouse_or(keys.cancel, "Cancel", |c| {
					c.mode == Quit && c.pointer == Choice(Secondary)
				}),
			],
		),
		(
			false,
			vec![(vec![text("Ctrl", 0x11)], "Scale from center", |c| c.mode == Box)],
		),
		(
			false,
			vec![
				(vec![text("Shift", 0x10)], "Scale uniformly", |c| {
					c.mode == Box && c.pointer != Close
				}),
				(vec![text("Shift", 0x10)], "Click to quit snap", |c| {
					c.mode == Box && c.pointer == Close
				}),
			],
		),
		(
			false,
			vec![(
				vec![text("←", 0x25), text("↑", 0x26), text("↓", 0x28), text("→", 0x27)],
				"Nudge 1 px",
				|c| c.mode == Box && c.has_box,
			)],
		),
		(
			false,
			vec![
				(copy_all, "Copy the whole screen", |c| c.mode == Box && !c.has_box),
				(caps(keys.copy), "Copy", |c| {
					c.mode == Box && c.has_box && c.pointer != Copy
				}),
			],
		),
		(
			false,
			vec![
				(caps(keys.save), "Save the whole screen", |c| {
					c.mode == Box && !c.has_box
				}),
				(caps(keys.save), "Save", |c| {
					c.mode == Box && c.has_box && c.pointer != Save
				}),
			],
		),
		// the copy row covers it without a box
		(
			false,
			vec![(caps(keys.copy_whole), "Copy the whole screen", |c| {
				c.mode == Box && c.has_box
			})],
		),
		(
			false,
			vec![(caps(keys.cancel), "Clear the box", |c| {
				c.mode == Box && c.has_box && c.pointer != Clear
			})],
		),
		(false, vec![(caps(keys.close), "Close", close_row)]),
		(false, vec![(caps(keys.quit), "Quit snap", |c| c.mode == Box)]),
		// the quit confirmation's
		(
			false,
			vec![(caps(keys.confirm), "Quit snap", |c| {
				c.mode == Quit && c.pointer != Choice(Primary)
			})],
		),
		(
			false,
			vec![(caps(keys.cancel), "Cancel", |c| {
				c.mode == Quit && c.pointer != Choice(Secondary)
			})],
		),
	]
}

impl Hints {
	// Starts at `context` without fading in. `animate` false makes rows appear and leave at once.
	pub fn new(
		d2d: &ID2D1Factory,
		write: &IDWriteFactory,
		scale: f32,
		screen_height: i32,
		keys: &Keys,
		context: Context,
		animate: bool,
	) -> Result<Self> {
		let s = |size: f32| (size * scale).round();
		let key_format = text_format(write, s(KEY_FONT_SIZE))?;
		let text_format = text_format(write, s(TEXT_FONT_SIZE))?;
		let key_height = s(KEY_HEIGHT);
		let stroke = s(1.0).max(1.0);
		// one wider when needed, so the line between the buttons sits exactly in the middle and both buttons are as wide
		let mouse_width = match (s(MOUSE_WIDTH) - stroke) % 2.0 {
			0.0 => s(MOUSE_WIDTH),
			_ => s(MOUSE_WIDTH) + 1.0,
		};
		let fade = if animate { ROW_FADE } else { Duration::ZERO };

		// each version's keys as their width, label (none for the mouse), whether they have a cap and the codes that
		// press them, then its description
		let mut specs = Vec::new();
		for (crossfade, row) in row_specs(keys) {
			let mut variants = Vec::new();
			for (row_keys, text, when) in row {
				let mut laid_out = Vec::new();
				for key in row_keys {
					laid_out.push(match key {
						Key::Mouse => (mouse_width, None, false, vec![VK_LBUTTON]),
						Key::Or => {
							let (layout, width, height) = text_layout(write, &key_format, "/")?;
							(width.round(), Some((layout, width, height)), false, Vec::new())
						}
						Key::Text(label, codes) => {
							let (layout, width, height) = text_layout(write, &key_format, &label)?;
							(
								(width + 2.0 * s(KEY_PADDING)).round().max(key_height),
								Some((layout, width, height)),
								true,
								codes,
							)
						}
					});
				}
				variants.push((laid_out, text_layout(write, &text_format, text)?, when));
			}
			specs.push((crossfade, variants));
		}

		// one key column and one text column for every version, so rows line up whichever show
		let key_column = specs
			.iter()
			.flat_map(|(_, variants)| variants)
			.map(|(keys, _, _)| {
				keys.iter().map(|(width, ..)| width).sum::<f32>() + s(KEY_GAP) * (keys.len() as f32 - 1.0)
			})
			.fold(0.0, f32::max);
		let text_column = specs
			.iter()
			.flat_map(|(_, variants)| variants)
			.map(|(_, (_, width, _), _)| *width)
			.fold(0.0, f32::max);
		let left = s(MARGIN) + s(PADDING);
		let text_x = left + key_column + s(TEXT_GAP);

		let mut rows = Vec::new();
		for (crossfade, row) in specs {
			let switch = if crossfade { fade } else { Duration::ZERO };
			let mut variants = Vec::new();
			for (row_keys, (text, _, text_height), when) in row {
				let mut parts = Vec::new();
				let mut x = left;
				for (key_width, label, capped, codes) in row_keys {
					let mut shapes = Vec::new();
					match label {
						// a key cap (or a bare "/") with its label centered
						Some((layout, label_width, label_height)) => {
							if capped {
								let cap = rounded(x, 0.0, key_width, key_height, s(KEY_RADIUS));
								shapes.push(Shape::Fill(cap, KEY_FILL_OPACITY));
								shapes.push(Shape::Outline(inset(cap, stroke / 2.0), stroke));
							}
							let origin = Vector2 {
								X: (x + (key_width - label_width) / 2.0).round(),
								Y: ((key_height - label_height) / 2.0).round(),
							};
							let outlines = text_outlines(d2d, &layout, origin)?;
							shapes.push(Shape::Text(origin, layout, outlines));
						}
						// the mouse: an outlined body with its left button filled, centered in the row
						None => {
							let mouse_height = s(MOUSE_HEIGHT);
							let mouse_y = ((key_height - mouse_height) / 2.0).round();
							let body = rounded(x, mouse_y, mouse_width, mouse_height, mouse_width / 2.0);
							shapes.push(Shape::Outline(inset(body, stroke / 2.0), stroke));
							let split_x = x + (mouse_width - stroke) / 2.0;
							let buttons_bottom = mouse_y + s(MOUSE_BUTTON_HEIGHT);
							// the line between the buttons
							shapes.push(Shape::Fill(
								rounded(split_x, mouse_y, stroke, s(MOUSE_BUTTON_HEIGHT), 0.0),
								1.0,
							));
							// inside the lines, since overlapping 50% white would show brighter
							let left_button = D2D_RECT_F {
								left: x,
								top: mouse_y,
								right: split_x,
								bottom: buttons_bottom,
							};
							shapes.push(Shape::ClippedFill(inset(body, stroke), left_button));
						}
					}
					parts.push(Part {
						codes,
						shapes,
						held: false,
					});
					x += key_width + s(KEY_GAP);
				}
				let origin = Vector2 {
					X: text_x,
					Y: ((key_height - text_height) / 2.0).round(),
				};
				let outlines = text_outlines(d2d, &text, origin)?;
				parts.push(Part {
					codes: Vec::new(),
					shapes: vec![Shape::Text(origin, text, outlines)],
					held: false,
				});
				variants.push(Variant {
					parts,
					when,
					shown: Tween::new(target(when, context), switch, cubic_out),
					opacity: 0.0,
				});
			}
			let presence = variants
				.iter()
				.map(|variant| target(variant.when, context))
				.fold(0.0, f32::max);
			rows.push(Row {
				variants,
				presence: Tween::new(presence, fade, cubic_out),
				top: 0.0,
			});
		}

		let padding = s(PADDING);
		let mut hints = Self {
			rect: Rect {
				left: 0,
				top: 0,
				right: 0,
				bottom: 0,
			},
			rows,
			left,
			bottom: screen_height as f32 - s(MARGIN) - padding,
			width: key_column + s(TEXT_GAP) + text_column,
			row_height: key_height,
			row_gap: s(ROW_GAP),
			padding,
			outline: OUTLINE * scale,
		};
		hints.update(context, |_| false, Instant::now());
		Ok(hints)
	}

	// Heads each row for `context`, lights up the keys `held` says are down (by virtual-key code), then lays the rows
	// out for `now`, stacked up from the bottom.
	pub fn update(&mut self, context: Context, held: impl Fn(u16) -> bool, now: Instant) {
		let mut bottom = self.bottom; // of the next row up
		for row in self.rows.iter_mut().rev() {
			let showing = row.target(context);
			row.presence.retarget(showing, now);
			let presence = row.presence.value(now);
			for variant in &mut row.variants {
				// a leaving row fades out as it was
				if showing > 0.0 {
					variant.shown.retarget(target(variant.when, context), now);
				}
				variant.opacity = presence.min(variant.shown.value(now));
				for part in &mut variant.parts {
					part.held = part.codes.iter().any(|&code| held(code));
				}
			}
			row.top = (bottom - self.row_height).round();
			bottom -= presence * (self.row_height + self.row_gap);
		}
		let top = (bottom + self.row_gap).min(self.bottom);
		self.rect = Rect {
			left: (self.left - self.padding) as i32,
			top: (top - self.padding).floor() as i32,
			right: (self.left + self.width + self.padding).ceil() as i32,
			bottom: (self.bottom + self.padding).ceil() as i32,
		};
	}

	// Whether rows are still fading or sliding, or about to start for `context`.
	pub fn is_animating(&self, context: Context, now: Instant) -> bool {
		self.rows.iter().any(|row| {
			let showing = row.target(context);
			!row.presence.is_done(now)
				|| row.presence.target() != showing
				|| (row.variants.iter()).any(|variant| {
					!variant.shown.is_done(now)
						|| (showing > 0.0 && variant.shown.target() != target(variant.when, context))
				})
		})
	}

	// Draws the hints as of the last `update`, each shape over a black stroke, faded to `opacity` (1 is their normal
	// look). Leaves per-primitive antialiasing on, the brushes at full opacity and no transform.
	pub fn draw(&self, render_target: &ID2D1RenderTarget, paint: &Paint, opacity: f32) {
		if opacity <= 0.0 {
			return;
		}
		// a row's area, relative to its top
		let bounds = D2D_RECT_F {
			left: self.left - self.padding,
			top: -self.padding,
			right: self.left + self.width + self.padding,
			bottom: self.row_height + self.padding,
		};
		unsafe {
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
			paint.white.SetOpacity(1.0);
			paint.black.SetOpacity(1.0);
		}
		for row in &self.rows {
			for variant in row.variants.iter().filter(|variant| variant.opacity > 0.0) {
				unsafe { render_target.SetTransform(&Matrix3x2::translation(0.0, row.top)) };
				// drawn solid and faded as one, so the white doesn't let its black stroke show through; held keys
				// separately, brighter (their strokes don't reach their neighbors)
				for (held, shown) in [(false, OPACITY), (true, HELD_OPACITY)] {
					let shapes = || {
						(variant.parts.iter())
							.filter(move |part| part.held == held)
							.flat_map(|part| &part.shapes)
					};
					if shapes().next().is_none() {
						continue;
					}
					paint.with_opacity(render_target, bounds, shown * opacity * variant.opacity, || {
						self.draw_shapes(render_target, paint, shapes)
					});
				}
			}
		}
		unsafe { render_target.SetTransform(&Matrix3x2::identity()) };
	}

	fn draw_shapes<'a, I: Iterator<Item = &'a Shape>>(
		&self,
		render_target: &ID2D1RenderTarget,
		paint: &Paint,
		shapes: impl Fn() -> I,
	) {
		let (white, black, round) = (&paint.white, &paint.black, &paint.round);
		unsafe {
			// the black stroke under everything first, so it never covers the white of a neighbor
			let extra = 2.0 * self.outline;
			for shape in shapes() {
				match shape {
					Shape::Fill(rect, _) => render_target.DrawRoundedRectangle(rect, black, extra, round),
					// inside the mouse, whose outline's stroke already surrounds it
					Shape::ClippedFill(..) => {}
					Shape::Outline(rect, width) => {
						render_target.DrawRoundedRectangle(rect, black, width + extra, round)
					}
					Shape::Text(_, _, outlines) => {
						for outline in outlines {
							render_target.DrawGeometry(outline, black, extra, round);
						}
					}
				}
			}
			for shape in shapes() {
				match shape {
					Shape::Fill(rect, fill_opacity) => {
						white.SetOpacity(*fill_opacity);
						render_target.FillRoundedRectangle(rect, white);
						white.SetOpacity(1.0);
					}
					Shape::ClippedFill(rect, clip) => {
						render_target.PushAxisAlignedClip(clip, D2D1_ANTIALIAS_MODE_ALIASED);
						render_target.FillRoundedRectangle(rect, white);
						render_target.PopAxisAlignedClip();
					}
					Shape::Outline(rect, width) => render_target.DrawRoundedRectangle(rect, white, *width, None),
					Shape::Text(origin, layout, _) => {
						render_target.DrawTextLayout(*origin, layout, white, D2D1_DRAW_TEXT_OPTIONS_NONE)
					}
				}
			}
		}
	}
}
