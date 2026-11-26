// confirm

// A confirmation drawn in the overlay: a message in the middle of the screen and two buttons under it, the primary one
// black on white and the secondary one white with a white outline. Enter picks the primary one and Esc the secondary
// one. Its backdrop is the overlay's dimming, which covers the whole screen while it's up. Laid out once when it opens.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT, ID2D1Factory, ID2D1Geometry,
	ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextLayout};
use windows::core::Result;
use windows_numerics::Vector2;

use crate::anim::{Tween, cubic_out};
use crate::gpu::{Paint, text_format, text_layout, text_outlines};
use crate::selection::{Point, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
	Primary,
	Secondary,
}

// Sizes in pixels at 100% display scaling.
const MESSAGE_FONT_SIZE: f32 = 16.0;
const BUTTON_FONT_SIZE: f32 = 13.0;
const BUTTON_HEIGHT: f32 = 30.0;
const BUTTON_PADDING: f32 = 16.0; // between a button's label and its sides
const BUTTON_MIN_WIDTH: f32 = 80.0;
const BUTTON_RADIUS: f32 = 4.0;
const BUTTON_GAP: f32 = 8.0;
const MESSAGE_GAP: f32 = 16.0; // between the message and the buttons
const OUTLINE: f32 = 1.5; // how far the black stroke under white text and lines reaches past them

const IDLE_OPACITY: f32 = 0.8; // the buttons, until hovered
const FADE: Duration = Duration::from_millis(150); // in when it opens, out when it's answered

struct Label {
	layout: IDWriteTextLayout,
	origin: Vector2,
	outlines: Vec<ID2D1Geometry>, // for the black stroke
}

struct ChoiceButton {
	rect: D2D_RECT_F,
	label: Label,
}

pub struct Confirm {
	message: Label,
	primary: ChoiceButton,
	secondary: ChoiceButton,
	screen: D2D_RECT_F,
	scale: f32,
	fade: Tween,
	pub pressed: Option<Choice>, // the button the mouse went down on, which picks it if it comes up there too
}

fn label(d2d: &ID2D1Factory, layout: IDWriteTextLayout, origin: Vector2) -> Result<Label> {
	let outlines = text_outlines(d2d, &layout, origin)?;
	Ok(Label {
		layout,
		origin,
		outlines,
	})
}

impl Confirm {
	// Asks `message` in the middle of a `width` × `height` screen, with the primary button (`primary`) on the left as
	// usual on Windows. Fades in unless `animate` is false.
	#[allow(clippy::too_many_arguments)]
	pub fn new(
		d2d: &ID2D1Factory,
		write: &IDWriteFactory,
		scale: f32,
		(width, height): (i32, i32),
		message: &str,
		primary: &str,
		secondary: &str,
		animate: bool,
	) -> Result<Self> {
		let s = |size: f32| (size * scale).round();
		let message_format = text_format(write, s(MESSAGE_FONT_SIZE))?;
		let button_format = text_format(write, s(BUTTON_FONT_SIZE))?;
		let (message, message_width, message_height) = text_layout(write, &message_format, message)?;
		let (primary, primary_width, primary_height) = text_layout(write, &button_format, primary)?;
		let (secondary, secondary_width, secondary_height) = text_layout(write, &button_format, secondary)?;

		// both buttons as wide as the wider label needs, so they match
		let button_width = (primary_width.max(secondary_width) + 2.0 * s(BUTTON_PADDING))
			.round()
			.max(s(BUTTON_MIN_WIDTH));
		let button_height = s(BUTTON_HEIGHT);
		let row_width = 2.0 * button_width + s(BUTTON_GAP);
		let total_height = message_height.ceil() + s(MESSAGE_GAP) + button_height;
		let (center_x, top) = (
			(width as f32 / 2.0).round(),
			((height as f32 - total_height) / 2.0).round(),
		);
		let message_origin = Vector2 {
			X: (center_x - message_width / 2.0).round(),
			Y: top,
		};
		let buttons_top = top + message_height.ceil() + s(MESSAGE_GAP);
		let button = |left: f32, layout: IDWriteTextLayout, label_width: f32, label_height: f32| -> Result<_> {
			let origin = Vector2 {
				X: (left + (button_width - label_width) / 2.0).round(),
				Y: (buttons_top + (button_height - label_height) / 2.0).round(),
			};
			Ok(ChoiceButton {
				rect: D2D_RECT_F {
					left,
					top: buttons_top,
					right: left + button_width,
					bottom: buttons_top + button_height,
				},
				label: label(d2d, layout, origin)?,
			})
		};
		let left = center_x - (row_width / 2.0).round();

		let mut fade = Tween::new(0.0, if animate { FADE } else { Duration::ZERO }, cubic_out);
		fade.retarget(1.0, Instant::now());
		Ok(Self {
			message: label(d2d, message, message_origin)?,
			primary: button(left, primary, primary_width, primary_height)?,
			secondary: button(
				left + button_width + s(BUTTON_GAP),
				secondary,
				secondary_width,
				secondary_height,
			)?,
			screen: D2D_RECT_F {
				left: 0.0,
				top: 0.0,
				right: width as f32,
				bottom: height as f32,
			},
			scale,
			fade,
			pressed: None,
		})
	}

	// The button under `p`. None once answered, while it fades out.
	pub fn hit(&self, p: Point) -> Option<Choice> {
		if self.is_answered() {
			return None;
		}
		let inside = |r: &D2D_RECT_F| {
			let rect = Rect {
				left: r.left as i32,
				top: r.top as i32,
				right: r.right as i32,
				bottom: r.bottom as i32,
			};
			rect.contains(p)
		};
		if inside(&self.primary.rect) {
			Some(Choice::Primary)
		} else if inside(&self.secondary.rect) {
			Some(Choice::Secondary)
		} else {
			None
		}
	}

	// Starts fading out. It takes no more input.
	pub fn answer(&mut self, now: Instant) {
		self.pressed = None;
		self.fade.retarget(0.0, now);
	}

	pub fn is_answered(&self) -> bool {
		self.fade.target() == 0.0
	}

	// Whether it's answered and gone, so it can be dropped.
	pub fn is_gone(&self, now: Instant) -> bool {
		self.is_answered() && self.fade.is_done(now)
	}

	pub fn is_animating(&self, now: Instant) -> bool {
		!self.fade.is_done(now)
	}

	// Draws it at `opacity` (1 is its normal look) with `hovered` lit up. Leaves per-primitive antialiasing on and the
	// brushes at full opacity.
	pub fn draw(
		&self,
		render_target: &ID2D1RenderTarget,
		paint: &Paint,
		opacity: f32,
		hovered: Option<Choice>,
		now: Instant,
	) {
		let opacity = opacity * self.fade.value(now);
		if opacity <= 0.0 {
			return;
		}
		let (white, black, round) = (&paint.white, &paint.black, &paint.round);
		let extra = 2.0 * OUTLINE * self.scale;
		let line = (self.scale).round().max(1.0);
		let rounded = |rect: D2D_RECT_F, inset: f32| D2D1_ROUNDED_RECT {
			rect: D2D_RECT_F {
				left: rect.left + inset,
				top: rect.top + inset,
				right: rect.right - inset,
				bottom: rect.bottom - inset,
			},
			radiusX: (BUTTON_RADIUS * self.scale).round(),
			radiusY: (BUTTON_RADIUS * self.scale).round(),
		};
		// white text over a black stroke
		let stroked_text = |label: &Label| unsafe {
			for outline in &label.outlines {
				render_target.DrawGeometry(outline, black, extra, round);
			}
			render_target.DrawTextLayout(label.origin, &label.layout, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
		};
		let shown = |choice| if hovered == Some(choice) { 1.0 } else { IDLE_OPACITY };
		unsafe {
			black.SetOpacity(1.0);
			white.SetOpacity(1.0);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
		}
		// each part drawn solid and faded as one, so the white doesn't let its black stroke show through
		paint.with_opacity(render_target, self.screen, opacity, || stroked_text(&self.message));
		paint.with_opacity(
			render_target,
			self.primary.rect,
			opacity * shown(Choice::Primary),
			|| unsafe {
				render_target.FillRoundedRectangle(&rounded(self.primary.rect, 0.0), white);
				let label = &self.primary.label;
				render_target.DrawTextLayout(label.origin, &label.layout, black, D2D1_DRAW_TEXT_OPTIONS_NONE);
			},
		);
		let bounds = D2D_RECT_F {
			left: self.secondary.rect.left - extra,
			top: self.secondary.rect.top - extra,
			right: self.secondary.rect.right + extra,
			bottom: self.secondary.rect.bottom + extra,
		};
		paint.with_opacity(render_target, bounds, opacity * shown(Choice::Secondary), || unsafe {
			// the outline inside the button's rect, over a black stroke
			let outline = rounded(self.secondary.rect, line / 2.0);
			render_target.DrawRoundedRectangle(&outline, black, line + extra, round);
			render_target.DrawRoundedRectangle(&outline, white, line, None);
			stroked_text(&self.secondary.label);
		});
	}
}
