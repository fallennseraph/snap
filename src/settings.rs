// settings

// The settings menu: a black panel in the middle of the overlay listing everything in `config.toml` by section, with a
// switch for each on/off setting, a button for each keybind (click it, then press the new chord) and a picker for
// Alt+F4. Each change applies at once; the overlay saves it. Laid out again after every change, and scrolls when it's
// taller than the screen.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
	D2D1_ROUNDED_RECT, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
	DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::Result;
use windows_numerics::{Matrix3x2, Vector2};

use crate::anim::{Tween, cubic_out};
use crate::config::{AltF4, Config, Keys};
use crate::gpu::{Paint, text_format, text_layout};
use crate::keys::Chord;
use crate::selection::Point;

// Something in the panel that takes clicks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
	Switch(usize), // in `SWITCHES`
	Bind(usize),   // in `BINDS`
	AltF4(AltF4),
	Reset,
}

type Field<T, F> = fn(&mut T) -> &mut F;

// The on/off settings in the order they show, under `General`, `UI` and `Animations`.
const SWITCHES: [(&str, Field<Config, bool>); 13] = [
	("Hardware acceleration", |c| &mut c.hardware_acceleration),
	("Start with Windows", |c| &mut c.start_with_windows),
	("Mouse cursor in screenshots", |c| &mut c.capture_cursor),
	("Hints", |c| &mut c.ui.hints),
	("Close and settings buttons", |c| &mut c.ui.close_button),
	("Old box outline while dragging", |c| &mut c.ui.old_box_outline),
	("New box outline while it catches up", |c| &mut c.ui.new_box_outline),
	("Animations", |c| &mut c.animations.enabled),
	("Overlay fade", |c| &mut c.animations.overlay_fade),
	("Modifier spring", |c| &mut c.animations.modifier_spring),
	("Drag spring", |c| &mut c.animations.drag_spring),
	("Copy flash", |c| &mut c.animations.copy_flash),
	("Copy flash on the whole screen", |c| {
		&mut c.animations.copy_flash_full_screen
	}),
];
const UI_START: usize = 3;
const ANIMATIONS_START: usize = 7; // `Animations` itself, which the ones after it need

// The keybinds in the order they show, with their names in `Keys::conflicts`.
const BINDS: [(&str, &str, Field<Keys, Chord>); 10] = [
	("Capture", "capture", |k| &mut k.capture),
	("Copy", "copy", |k| &mut k.copy),
	("Copy the whole screen", "copy_whole", |k| &mut k.copy_whole),
	("Save", "save", |k| &mut k.save),
	("Close", "close", |k| &mut k.close),
	("Undo", "undo", |k| &mut k.undo),
	("Redo", "redo", |k| &mut k.redo),
	("Confirm", "confirm", |k| &mut k.confirm),
	("Cancel or clear the box", "cancel", |k| &mut k.cancel),
	("Settings", "settings", |k| &mut k.settings),
];

const ALT_F4: [(AltF4, &str); 2] = [(AltF4::Disabled, "Nothing"), (AltF4::Quit, "Quit snap")];

const ESCAPE: u16 = 0x1B;

// Sizes in pixels at 100% display scaling.
const PANEL_WIDTH: f32 = 460.0;
const PANEL_RADIUS: f32 = 8.0;
const SCREEN_MARGIN: f32 = 40.0; // the least space above and below a panel that scrolls
const PADDING: f32 = 20.0;
const TITLE_FONT_SIZE: f32 = 16.0;
const HEADING_FONT_SIZE: f32 = 12.0;
const LABEL_FONT_SIZE: f32 = 13.0;
const NOTE_GAP: f32 = 2.0; // between the title and the note under it
const SECTION_GAP: f32 = 18.0; // above each heading
const HEADING_GAP: f32 = 4.0; // under each heading
const ROW_HEIGHT: f32 = 30.0;
const SWITCH_WIDTH: f32 = 32.0;
const SWITCH_HEIGHT: f32 = 18.0;
const KNOB_INSET: f32 = 3.0; // between a switch's knob and its edge
const BUTTON_HEIGHT: f32 = 24.0;
const BUTTON_PADDING: f32 = 10.0; // between a button's label and its sides
const BUTTON_RADIUS: f32 = 4.0;
const BIND_WIDTH: f32 = 140.0; // at least; wider for a long chord
const SEGMENT_GAP: f32 = 4.0; // between the Alt+F4 choices
const WARNING_GAP: f32 = 6.0; // above each warning
const SCROLLBAR_WIDTH: f32 = 3.0;
const SCROLLBAR_INSET: f32 = 4.0; // from the panel's right edge, top and bottom
const SCROLL_STEP: f32 = 48.0; // per notch of the mouse wheel

const PANEL_OPACITY: f32 = 0.9; // the black behind it all
const BORDER_OPACITY: f32 = 0.2;
const NOTE_OPACITY: f32 = 0.5; // the note under the title, and the headings
const LABEL_OPACITY: f32 = 0.9;
const IDLE_OPACITY: f32 = 0.8; // the controls, until hovered
const UNUSED_OPACITY: f32 = 0.4; // animation switches while `Animations` is off
const UNBOUND_OPACITY: f32 = 0.5; // a keybind button's "None", relative to the button
const SCROLLBAR_OPACITY: f32 = 0.3;
const FADE: Duration = Duration::from_millis(150); // in when it opens, out when it closes

struct Text {
	layout: IDWriteTextLayout,
	origin: Vector2,
	opacity: f32,
}

enum Look {
	Switch(bool), // on or off
	Button(bool), // filled (the picked Alt+F4 choice, or a keybind waiting for a key) or outlined
}

struct Control {
	target: Target,
	rect: D2D_RECT_F, // in content coordinates: from the panel's top-left corner, before scrolling
	look: Look,
	label: Option<Text>,
	unused: bool, // dimmed, since nothing uses it right now
}

pub struct Settings {
	pub config: Config,
	write: IDWriteFactory,
	title_format: IDWriteTextFormat,
	heading_format: IDWriteTextFormat,
	label_format: IDWriteTextFormat,
	scale: f32,
	screen: (f32, f32),
	texts: Vec<Text>,
	controls: Vec<Control>,
	panel: D2D_RECT_F,      // on screen
	content_height: f32,    // everything in the panel, which can be taller than the panel
	scroll: f32,            // how far the content is scrolled up, in pixels
	waiting: Option<usize>, // the keybind waiting for a key, in `BINDS`
	fade: Tween,
	pub pressed: Option<Target>, // the control the mouse went down on, which clicks if it comes up there too
}

fn v(x: f32, y: f32) -> Vector2 {
	Vector2 { X: x, Y: y }
}

fn rect(left: f32, top: f32, width: f32, height: f32) -> D2D_RECT_F {
	D2D_RECT_F {
		left,
		top,
		right: left + width,
		bottom: top + height,
	}
}

fn inset(r: D2D_RECT_F, by: f32) -> D2D_RECT_F {
	D2D_RECT_F {
		left: r.left + by,
		top: r.top + by,
		right: r.right - by,
		bottom: r.bottom - by,
	}
}

fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
	D2D1_ROUNDED_RECT {
		rect,
		radiusX: radius,
		radiusY: radius,
	}
}

fn contains(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
	x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

// How a keybind's button reads: its keys joined with "+", or "None".
fn chord_label(chord: Chord) -> String {
	let keys: Vec<String> = chord.caps().into_iter().map(|(label, _)| label).collect();
	keys.join("+")
}

// The panel's warnings about `config`: keybinds that share a chord, and an unbound capture key.
fn warnings(config: &Config) -> Vec<String> {
	let label = |name: &'static str| -> &'static str {
		(BINDS.iter())
			.find(|(_, bind, _)| *bind == name)
			.map_or(name, |(label, ..)| label)
	};
	let mut warnings: Vec<String> = (config.keys.conflicts().into_iter())
		.map(|(a, b)| format!("{} and {} share a keybind.", label(a), label(b)))
		.collect();
	if config.keys.capture.is_none() {
		warnings.push("Capture has no keybind, so only the tray icon opens snap.".to_string());
	}
	warnings
}

// Lays out `text` wrapping at `width`. Returns the layout and its height.
fn wrapped_layout(
	write: &IDWriteFactory,
	format: &IDWriteTextFormat,
	text: &str,
	width: f32,
) -> Result<(IDWriteTextLayout, f32)> {
	let text: Vec<u16> = text.encode_utf16().collect();
	unsafe {
		let layout = write.CreateTextLayout(&text, format, width, f32::MAX)?;
		let mut metrics = DWRITE_TEXT_METRICS::default();
		layout.GetMetrics(&mut metrics)?;
		Ok((layout, metrics.height))
	}
}

// Collects the panel's texts and controls from the top down.
struct Builder<'a> {
	settings: &'a Settings,
	texts: Vec<Text>,
	controls: Vec<Control>,
	y: f32, // the top of what comes next
}

impl Builder<'_> {
	fn s(&self, size: f32) -> f32 {
		(size * self.settings.scale).round()
	}

	fn width(&self) -> f32 {
		self.s(PANEL_WIDTH)
	}

	fn heading(&mut self, name: &str) -> Result<()> {
		self.y += self.s(SECTION_GAP);
		let format = &self.settings.heading_format;
		let (layout, _, height) = text_layout(&self.settings.write, format, name)?;
		self.texts.push(Text {
			layout,
			origin: v(self.s(PADDING), self.y),
			opacity: NOTE_OPACITY,
		});
		self.y += height.ceil() + self.s(HEADING_GAP);
		Ok(())
	}

	// A row with `label` on the left. Returns the row's top.
	fn row(&mut self, label: &str, unused: bool) -> Result<f32> {
		let top = self.y;
		let (layout, _, height) = text_layout(&self.settings.write, &self.settings.label_format, label)?;
		let row_height = self.s(ROW_HEIGHT);
		self.texts.push(Text {
			layout,
			origin: v(self.s(PADDING), (top + (row_height - height) / 2.0).round()),
			opacity: if unused { UNUSED_OPACITY } else { LABEL_OPACITY },
		});
		self.y += row_height;
		Ok(top)
	}

	// The vertical position of a control `height` tall in the row at `top`.
	fn centered(&self, top: f32, height: f32) -> f32 {
		(top + (self.s(ROW_HEIGHT) - height) / 2.0).round()
	}

	fn switch(&mut self, i: usize) -> Result<()> {
		let mut config = self.settings.config;
		let on = *SWITCHES[i].1(&mut config);
		let unused = i > ANIMATIONS_START && !config.animations.enabled;
		let top = self.row(SWITCHES[i].0, unused)?;
		let (width, height) = (self.s(SWITCH_WIDTH), self.s(SWITCH_HEIGHT));
		self.controls.push(Control {
			target: Target::Switch(i),
			rect: rect(
				self.width() - self.s(PADDING) - width,
				self.centered(top, height),
				width,
				height,
			),
			look: Look::Switch(on),
			label: None,
			unused,
		});
		Ok(())
	}

	// A button with `text` centered in it, `min_width` wide or as wide as the text needs, its right edge at `right`.
	// Returns its left edge.
	fn button(
		&mut self,
		target: Target,
		text: &str,
		filled: bool,
		right: f32,
		top: f32,
		min_width: f32,
	) -> Result<f32> {
		let (layout, text_width, text_height) = text_layout(&self.settings.write, &self.settings.label_format, text)?;
		let width = (text_width + 2.0 * self.s(BUTTON_PADDING)).ceil().max(min_width);
		let height = self.s(BUTTON_HEIGHT);
		let left = right - width;
		self.controls.push(Control {
			target,
			rect: rect(left, top, width, height),
			look: Look::Button(filled),
			label: Some(Text {
				layout,
				origin: v(
					(left + (width - text_width) / 2.0).round(),
					(top + (height - text_height) / 2.0).round(),
				),
				opacity: 1.0,
			}),
			unused: false,
		});
		Ok(left)
	}

	fn bind(&mut self, i: usize) -> Result<()> {
		let mut keys = self.settings.config.keys;
		let chord = *BINDS[i].2(&mut keys);
		let waiting = self.settings.waiting == Some(i);
		let top = self.row(BINDS[i].0, false)?;
		let text = if waiting {
			"Press a key…".to_string()
		} else {
			chord_label(chord)
		};
		let top = self.centered(top, self.s(BUTTON_HEIGHT));
		let (right, min_width) = (self.width() - self.s(PADDING), self.s(BIND_WIDTH));
		self.button(Target::Bind(i), &text, waiting, right, top, min_width)?;
		// an unbound action's "None" is dimmed, so it doesn't read like a key
		if chord.is_none()
			&& !waiting
			&& let Some(label) = self.controls.last_mut().and_then(|control| control.label.as_mut())
		{
			label.opacity = UNBOUND_OPACITY;
		}
		Ok(())
	}

	fn alt_f4(&mut self) -> Result<()> {
		let top = self.row("Alt+F4", false)?;
		let top = self.centered(top, self.s(BUTTON_HEIGHT));
		let mut right = self.width() - self.s(PADDING);
		for &(choice, text) in ALT_F4.iter().rev() {
			let picked = self.settings.config.keys.alt_f4 == choice;
			let left = self.button(Target::AltF4(choice), text, picked, right, top, 0.0)?;
			right = left - self.s(SEGMENT_GAP);
		}
		Ok(())
	}

	fn warnings(&mut self) -> Result<()> {
		let width = self.width() - 2.0 * self.s(PADDING);
		for warning in warnings(&self.settings.config) {
			self.y += self.s(WARNING_GAP);
			let (layout, height) = wrapped_layout(&self.settings.write, &self.settings.label_format, &warning, width)?;
			self.texts.push(Text {
				layout,
				origin: v(self.s(PADDING), self.y),
				opacity: 1.0,
			});
			self.y += height.ceil();
		}
		Ok(())
	}
}

impl Settings {
	// Shows `config` in the middle of a `width` × `height` screen. Fades in unless `animate` is false.
	pub fn new(
		write: &IDWriteFactory,
		scale: f32,
		(width, height): (i32, i32),
		config: Config,
		animate: bool,
	) -> Result<Self> {
		let s = |size: f32| (size * scale).round();
		let mut fade = Tween::new(0.0, if animate { FADE } else { Duration::ZERO }, cubic_out);
		fade.retarget(1.0, Instant::now());
		let mut settings = Self {
			config,
			write: write.clone(),
			title_format: text_format(write, s(TITLE_FONT_SIZE))?,
			heading_format: text_format(write, s(HEADING_FONT_SIZE))?,
			label_format: text_format(write, s(LABEL_FONT_SIZE))?,
			scale,
			screen: (width as f32, height as f32),
			texts: Vec::new(),
			controls: Vec::new(),
			panel: D2D_RECT_F::default(),
			content_height: 0.0,
			scroll: 0.0,
			waiting: None,
			fade,
			pressed: None,
		};
		settings.layout()?;
		Ok(settings)
	}

	// Lays everything out again for the current config, keeping the scroll position where it can.
	fn layout(&mut self) -> Result<()> {
		let mut b = Builder {
			settings: self,
			texts: Vec::new(),
			controls: Vec::new(),
			y: 0.0,
		};
		b.y = b.s(PADDING);
		let (layout, _, height) = text_layout(&self.write, &self.title_format, "Settings")?;
		b.texts.push(Text {
			layout,
			origin: v(b.s(PADDING), b.y),
			opacity: 1.0,
		});
		b.y += height.ceil() + b.s(NOTE_GAP);
		let (layout, _, height) = text_layout(&self.write, &self.heading_format, "Changes are saved right away.")?;
		b.texts.push(Text {
			layout,
			origin: v(b.s(PADDING), b.y),
			opacity: NOTE_OPACITY,
		});
		b.y += height.ceil();

		b.heading("General")?;
		for i in 0..UI_START {
			b.switch(i)?;
		}
		b.heading("UI")?;
		for i in UI_START..ANIMATIONS_START {
			b.switch(i)?;
		}
		b.heading("Animations")?;
		for i in ANIMATIONS_START..SWITCHES.len() {
			b.switch(i)?;
		}
		b.heading("Keys")?;
		for i in 0..BINDS.len() {
			b.bind(i)?;
		}
		b.alt_f4()?;
		b.warnings()?;
		b.y += b.s(SECTION_GAP);
		let (left, top) = (b.s(PADDING), b.y);
		let reset = b.button(Target::Reset, "Reset to defaults", false, 0.0, top, 0.0)?;
		// placed from its right edge, so move it to start at the padding
		if let Some(control) = b.controls.last_mut() {
			let shift = left - reset;
			control.rect.left += shift;
			control.rect.right += shift;
			if let Some(label) = &mut control.label {
				label.origin.X += shift;
			}
		}
		b.y += b.s(BUTTON_HEIGHT) + b.s(PADDING);

		let (texts, controls, content_height) = (b.texts, b.controls, b.y);
		let s = |size: f32| (size * self.scale).round();
		let (width, screen_height) = (s(PANEL_WIDTH), self.screen.1);
		let height = content_height.min(screen_height - 2.0 * s(SCREEN_MARGIN)).max(0.0);
		let (left, top) = (
			((self.screen.0 - width) / 2.0).round(),
			((screen_height - height) / 2.0).round(),
		);
		self.texts = texts;
		self.controls = controls;
		self.content_height = content_height;
		self.panel = rect(left, top, width, height);
		self.scroll_by(0.0);
		Ok(())
	}

	fn max_scroll(&self) -> f32 {
		(self.content_height - (self.panel.bottom - self.panel.top)).max(0.0)
	}

	fn scroll_by(&mut self, pixels: f32) {
		self.scroll = (self.scroll + pixels).clamp(0.0, self.max_scroll()).round();
	}

	// Scrolls for a mouse wheel turn of `delta` (120 per notch, positive away from the user). `false` if it was
	// already as far as it goes.
	pub fn scroll(&mut self, delta: i16) -> bool {
		let before = self.scroll;
		self.scroll_by(-(delta as f32) / 120.0 * SCROLL_STEP * self.scale);
		self.scroll != before
	}

	// The control under `p`. None once closing, while it fades out.
	pub fn hit(&self, p: Point) -> Option<Target> {
		let (x, y) = (p.x as f32, p.y as f32);
		if self.is_closed() || !contains(&self.panel, x, y) {
			return None;
		}
		let (x, y) = (x - self.panel.left, y - self.panel.top + self.scroll);
		(self.controls.iter())
			.find(|control| contains(&control.rect, x, y))
			.map(|control| control.target)
	}

	// Clicks `target`: flips a switch, picks an Alt+F4 choice, resets everything, or starts (or stops) waiting for a
	// keybind. Any other click stops the wait first. `true` if the config changed.
	pub fn click(&mut self, target: Target) -> Result<bool> {
		let waiting = self.waiting.take();
		let changed = match target {
			Target::Switch(i) => {
				let switch = SWITCHES[i].1(&mut self.config);
				*switch = !*switch;
				true
			}
			Target::Bind(i) => {
				self.waiting = (waiting != Some(i)).then_some(i);
				false
			}
			Target::AltF4(choice) => {
				let changed = self.config.keys.alt_f4 != choice;
				self.config.keys.alt_f4 = choice;
				changed
			}
			Target::Reset => {
				self.config = Config::default();
				true
			}
		};
		self.layout()?;
		Ok(changed)
	}

	// Takes `chord` for the keybind waiting for one: plain Esc unbinds it, and a lone modifier or a key `config.toml`
	// has no name for keeps it waiting. `true` if the config changed.
	pub fn record(&mut self, chord: Chord) -> Result<bool> {
		let Some(i) = self.waiting else { return Ok(false) };
		let chord = if chord == Chord::plain(ESCAPE) {
			Chord::NONE
		} else if chord.is_bindable() {
			chord
		} else {
			return Ok(false);
		};
		self.waiting = None;
		*BINDS[i].2(&mut self.config.keys) = chord;
		self.layout()?;
		Ok(true)
	}

	// Whether a keybind is waiting for a key, which should then come here instead of doing anything else.
	pub fn is_waiting(&self) -> bool {
		self.waiting.is_some()
	}

	// Starts fading out, stopping any wait for a key. It takes no more input.
	pub fn close(&mut self, now: Instant) {
		self.pressed = None;
		self.waiting = None;
		self.fade.retarget(0.0, now);
	}

	pub fn is_closed(&self) -> bool {
		self.fade.target() == 0.0
	}

	// Whether it's closed and gone, so it can be dropped.
	pub fn is_gone(&self, now: Instant) -> bool {
		self.is_closed() && self.fade.is_done(now)
	}

	pub fn is_animating(&self, now: Instant) -> bool {
		!self.fade.is_done(now)
	}

	// Draws it at `opacity` (1 is its normal look) with `hovered` lit up. Leaves the transform reset, the brushes at
	// full opacity and per-primitive antialiasing on.
	pub fn draw(
		&self,
		render_target: &ID2D1RenderTarget,
		paint: &Paint,
		opacity: f32,
		hovered: Option<Target>,
		now: Instant,
	) {
		let opacity = opacity * self.fade.value(now);
		if opacity <= 0.0 {
			return;
		}
		let (white, black) = (&paint.white, &paint.black);
		let s = |size: f32| (size * self.scale).round();
		let line = self.scale.round().max(1.0);
		let panel = self.panel;
		unsafe {
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
		}
		paint.with_opacity(render_target, panel, opacity, || unsafe {
			black.SetOpacity(PANEL_OPACITY);
			render_target.FillRoundedRectangle(&rounded(panel, s(PANEL_RADIUS)), black);
			white.SetOpacity(BORDER_OPACITY);
			render_target.DrawRoundedRectangle(&rounded(inset(panel, line / 2.0), s(PANEL_RADIUS)), white, line, None);

			// the content, scrolled and cut off at the panel's border
			render_target.PushAxisAlignedClip(&inset(panel, line), D2D1_ANTIALIAS_MODE_ALIASED);
			render_target.SetTransform(&Matrix3x2::translation(panel.left, panel.top - self.scroll));
			for text in &self.texts {
				white.SetOpacity(text.opacity);
				render_target.DrawTextLayout(text.origin, &text.layout, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
			}
			for control in &self.controls {
				let shown = if control.unused {
					UNUSED_OPACITY
				} else if hovered == Some(control.target) {
					1.0
				} else {
					IDLE_OPACITY
				};
				// drawn solid and faded as one, so a black knob or label over white stays black
				white.SetOpacity(1.0);
				black.SetOpacity(1.0);
				paint.with_opacity(render_target, inset(control.rect, -line), shown, || {
					self.draw_control(render_target, paint, control, line)
				});
			}
			render_target.SetTransform(&Matrix3x2::identity());
			render_target.PopAxisAlignedClip();

			// a thin bar along the right edge when it scrolls
			let max_scroll = self.max_scroll();
			if max_scroll > 0.0 {
				let track = panel.bottom - panel.top - 2.0 * s(SCROLLBAR_INSET);
				let length = (track * (panel.bottom - panel.top) / self.content_height).round();
				let top = panel.top + s(SCROLLBAR_INSET) + ((track - length) * self.scroll / max_scroll).round();
				let width = s(SCROLLBAR_WIDTH).max(1.0);
				let bar = rect(panel.right - s(SCROLLBAR_INSET) - width, top, width, length);
				white.SetOpacity(SCROLLBAR_OPACITY);
				render_target.FillRoundedRectangle(&rounded(bar, width / 2.0), white);
			}
			white.SetOpacity(1.0);
			black.SetOpacity(1.0);
		});
	}

	fn draw_control(&self, render_target: &ID2D1RenderTarget, paint: &Paint, control: &Control, line: f32) {
		let (white, black) = (&paint.white, &paint.black);
		let r = control.rect;
		unsafe {
			match control.look {
				Look::Switch(on) => {
					let height = r.bottom - r.top;
					let pill = rounded(r, height / 2.0);
					let radius = height / 2.0 - (KNOB_INSET * self.scale).round();
					let center_y = r.top + height / 2.0;
					let knob = |x: f32| D2D1_ELLIPSE {
						point: v(x, center_y),
						radiusX: radius,
						radiusY: radius,
					};
					if on {
						render_target.FillRoundedRectangle(&pill, white);
						render_target.FillEllipse(&knob(r.right - height / 2.0), black);
					} else {
						let outline = rounded(inset(r, line / 2.0), (height - line) / 2.0);
						render_target.DrawRoundedRectangle(&outline, white, line, None);
						render_target.FillEllipse(&knob(r.left + height / 2.0), white);
					}
				}
				Look::Button(filled) => {
					let radius = (BUTTON_RADIUS * self.scale).round();
					let text_brush = if filled {
						render_target.FillRoundedRectangle(&rounded(r, radius), white);
						black
					} else {
						render_target.DrawRoundedRectangle(&rounded(inset(r, line / 2.0), radius), white, line, None);
						white
					};
					if let Some(label) = &control.label {
						text_brush.SetOpacity(label.opacity);
						render_target.DrawTextLayout(
							label.origin,
							&label.layout,
							text_brush,
							D2D1_DRAW_TEXT_OPTIONS_NONE,
						);
						text_brush.SetOpacity(1.0);
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_keybind_shows_with_its_conflict_name() {
		// each of `Keys::conflicts`'s names has a label, so no warning shows a raw field name
		let mut keys = Keys::default();
		for (_, _, field) in BINDS {
			*field(&mut keys) = Chord::ctrl(b'Q');
		}
		for (a, b) in keys.conflicts() {
			for name in [a, b] {
				assert!(BINDS.iter().any(|(_, bind, _)| *bind == name), "no label for '{name}'");
			}
		}
	}

	#[test]
	fn warns_about_shared_and_missing_keys() {
		let mut config = Config::default();
		assert!(warnings(&config).is_empty());
		config.keys.save = config.keys.copy;
		config.keys.capture = Chord::NONE;
		assert_eq!(
			warnings(&config),
			[
				"Copy and Save share a keybind.",
				"Capture has no keybind, so only the tray icon opens snap.",
			]
		);
	}

	#[test]
	fn button_labels() {
		assert_eq!(chord_label(Chord::ctrl(b'C')), "Ctrl+C");
		assert_eq!(chord_label(Chord::NONE), "None");
	}
}
