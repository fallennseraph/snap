// overlay

// The full-screen window that shows the frozen screenshot and lets the user draw a box on it.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_LWIN,
	VK_MENU, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
	CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
	HTCLIENT, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
	LoadCursorW, MB_ICONERROR, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW, SW_HIDE, SWP_SHOWWINDOW,
	SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow, WM_APP, WM_CAPTURECHANGED, WM_DESTROY, WM_ERASEBKGND,
	WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR,
	WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::anim::{Spring, Tween, cubic_out};
use crate::buttons::{Button, Buttons};
use crate::capture::{Image, capture_screen};
use crate::clipboard::copy_image;
use crate::config::{self, Animations, Keys};
use crate::confirm::{Choice, Confirm};
use crate::gpu::{Gpu, Scene, text_format, text_layout};
use crate::hints::{Context, Hints, Pointer};
use crate::keys::Chord;
use crate::motion::{self, BoxMotion};
use crate::render::{highlight, is_stacked, position_label, scaled, under_box};
use crate::save::{ask_path, write_png};
use crate::selection::{Grip, Hit, Modifiers, Point, Rect, Selection};
use crate::tray;

// Posted by the keyboard hook when the capture key is pressed.
pub const WM_SNAP_KEY: u32 = WM_APP + 1;

// Sizes in pixels at 100% display scaling; they grow with the scaling.
const HITSLOP_INSIDE: i32 = 10; // how far an edge's grab zone reaches into the box
const HITSLOP_OUTSIDE: i32 = 20; // and out of it
const LABEL_FONT_SIZE: i32 = 14;
const LABEL_STROKE: i32 = 3; // the black stroke around the labels' text, half of it outside the glyphs
const LABEL_GAP: i32 = 4; // between the box and its labels and buttons, and between a label and the buttons
const HIGHLIGHT_THICKNESS: i32 = 2;
const HIGHLIGHT_LENGTH: i32 = 12; // how far a corner highlight reaches along each edge

// While anything animates, a background thread waits for each vsync and posts this, so the overlay redraws once per
// display refresh. (A Win32 timer only fires on the ~15.6 ms system tick, so a 16 ms one ran at about 32 fps.)
const WM_SNAP_FRAME: u32 = WM_APP + 3;
const FALLBACK_FRAME: Duration = Duration::from_millis(16); // if DwmFlush fails, which it shouldn't on Windows 8+
const OVERLAY_FADE: Duration = Duration::from_millis(300); // in when opening, out when closing
const HINT_FADE: Duration = Duration::from_millis(300); // the hints fade out while the mouse is over them
const OLD_BOX_FADE: Duration = Duration::from_millis(300); // the dotted outline of the box before a drag
const COPY_FLASH: Duration = Duration::from_millis(200);
const BUTTONS_FADE: Duration = Duration::from_millis(300); // in when a box appears, out when it's cleared
// Seconds for the buttons to (almost) arrive when they switch between beside the size label and under it.
const STACK_SETTLE: f32 = 0.2;
const MAX_STEP: f32 = 0.05; // longest frame to simulate, so a stall doesn't make the buttons jump
const ARRIVED: f32 = 0.000_5; // how close `stack` has to get to call the switch done: under a pixel on a 2000 px move
// How far behind the drawn box has to be before a dotted outline shows where it's going, in pixels.
const TARGET_OUTLINE_LAG: f32 = 2.0;

// Also how a newly started copy finds this one to close it.
pub const CLASS_NAME: PCWSTR = w!("snap.overlay");

// Everything that exists while the overlay is open.
struct Shot {
	image: Image,
	selection: Selection,
	hover: Point,
	scale: f32,
	hardware: bool,
	old_box_outline: bool,
	new_box_outline: bool,
	animations: Animations,
	keys: Keys,
	label_format: IDWriteTextFormat,
	hints: Option<Hints>,
	hints_fade: Tween,
	buttons: Buttons,
	pressed: Option<Button>, // the button the mouse went down on, which clicks if it comes up there too
	row: Option<Rect>,       // where the buttons were last drawn, kept while they fade out after the box is cleared
	buttons_fade: Tween,
	stack: Spring,  // 0 with the buttons beside the size label, 1 with them under it, springing between
	stacked: bool,  // which one `stack` is heading for
	stack_box: u32, // the box `stack` belongs to; a new box jumps to its layout
	fade: Tween,    // the overlay fading in and out: the dimming and everything drawn over the screenshot
	motion: BoxMotion,
	old_boxes: Vec<(Rect, Tween)>, // dotted outlines of boxes before a drag, fading out
	flash: Option<(Rect, Tween)>,
	last_frame: Instant,
	pacing: bool,             // the vsync thread is posting frames
	saving: bool,             // the Save As dialog is open
	closing: bool,            // fading out; input is ignored
	confirm: Option<Confirm>, // asking whether to quit snap; kept while it fades out after an answer
}

thread_local! {
	// The window procedure can be re-entered by Win32 calls that send messages (ShowWindow, ReleaseCapture, the
	// clipboard), so the state is only borrowed around code that makes no such calls.
	static SHOT: RefCell<Option<Shot>> = const { RefCell::new(None) };
	// Made when the overlay first opens and kept, so later opens are quick.
	static GPU: RefCell<Option<Gpu>> = const { RefCell::new(None) };
}

// Runs `f` on the open shot. `None` if the overlay is closed or the state is already borrowed further up the stack.
fn with_shot<R>(f: impl FnOnce(&mut Shot) -> R) -> Option<R> {
	SHOT.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

// Runs `f` on the GPU state, creating it the first time.
fn with_gpu<R>(f: impl FnOnce(&mut Gpu) -> Result<R>) -> Result<R> {
	GPU.with(|cell| {
		let mut gpu = cell.borrow_mut();
		if gpu.is_none() {
			*gpu = Some(Gpu::new()?);
		}
		f(gpu.as_mut().expect("just created"))
	})
}

// Creates the overlay window, hidden.
pub fn create() -> Result<HWND> {
	unsafe {
		let instance = GetModuleHandleW(None)?;
		let class = WNDCLASSEXW {
			cbSize: size_of::<WNDCLASSEXW>() as u32,
			lpfnWndProc: Some(window_proc),
			hInstance: instance.into(),
			hCursor: LoadCursorW(None, IDC_CROSS)?,
			lpszClassName: CLASS_NAME,
			..Default::default()
		};
		if RegisterClassExW(&class) == 0 {
			return Err(Error::from_thread());
		}
		CreateWindowExW(
			WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
			CLASS_NAME,
			w!("snap"),
			WS_POPUP,
			0,
			0,
			0,
			0,
			None,
			None,
			Some(instance.into()),
			None,
		)
	}
}

fn open(window: HWND) -> Result<()> {
	let image = capture_screen()?;
	let (width, height) = (image.width, image.height);
	let config = config::load();
	let animations = config.animations.active();
	let scale = unsafe { GetDpiForWindow(window) } as f32 / 96.0;

	let (d2d, write, label_format, buttons) = with_gpu(|gpu| {
		gpu.prepare(window, width, height, config.hardware_acceleration)?;
		gpu.upload(&image)?;
		Ok((
			gpu.d2d.clone(),
			gpu.write.clone(),
			text_format(&gpu.write, scaled(LABEL_FONT_SIZE, scale) as f32)?,
			Buttons::new(&gpu.d2d, scale)?,
		))
	})?;
	let start = Context {
		has_box: false,
		pointer: Pointer::Outside,
	};
	let hints = match config
		.show_hints
		.then(|| Hints::new(&d2d, &write, scale, height, &config.keys, start, animations.enabled))
	{
		Some(Err(err)) => {
			eprintln!("snap: couldn't lay out the hints: {err}");
			None
		}
		hints => hints.and_then(|hints| hints.ok()),
	};

	let mut cursor = POINT::default();
	let _ = unsafe { GetCursorPos(&mut cursor) };
	let now = Instant::now();
	// the first frame looks exactly like the screen, then the dimming and UI fade in
	let mut fade = Tween::new(if animations.overlay_fade { 0.0 } else { 1.0 }, OVERLAY_FADE, cubic_out);
	fade.retarget(1.0, now);
	let mut shot = Shot {
		image,
		selection: Selection::new(
			width,
			height,
			scaled(HITSLOP_INSIDE, scale),
			scaled(HITSLOP_OUTSIDE, scale),
		),
		hover: Point {
			x: cursor.x,
			y: cursor.y,
		},
		scale,
		hardware: config.hardware_acceleration,
		old_box_outline: config.old_box_outline,
		new_box_outline: config.new_box_outline,
		animations,
		keys: config.keys,
		label_format,
		hints,
		hints_fade: Tween::new(1.0, HINT_FADE, cubic_out),
		buttons,
		pressed: None,
		row: None,
		buttons_fade: Tween::new(
			0.0,
			if animations.enabled {
				BUTTONS_FADE
			} else {
				Duration::ZERO
			},
			cubic_out,
		),
		stack: Spring::new(0.0),
		stacked: false,
		stack_box: 0,
		fade,
		motion: BoxMotion::default(),
		old_boxes: Vec::new(),
		flash: None,
		last_frame: now,
		pacing: false,
		saving: false,
		closing: false,
		confirm: None,
	};
	// no fade when the overlay opens with the mouse already over the hints
	shot.hints_fade = Tween::new(shot.hints_target(), HINT_FADE, cubic_out);
	SHOT.with(|cell| *cell.borrow_mut() = Some(shot));

	unsafe {
		SetWindowPos(window, Some(HWND_TOPMOST), 0, 0, width, height, SWP_SHOWWINDOW)?;
		// paint right away, so nothing stale shows before the first frame
		let _ = UpdateWindow(window);
		bring_to_front(window);
	}
	keep_animating(window);
	Ok(())
}

fn open_or_report(window: HWND) {
	if let Err(err) = open(window) {
		eprintln!("snap: couldn't open the overlay: {err}");
		hide(window);
	}
}

// Windows only lets the app that has focus give it away, and snap doesn't have it when PrtScr is pressed. Joining
// the focused app's input queue for a moment gets around that.
unsafe fn bring_to_front(window: HWND) {
	unsafe {
		let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
		let own_thread = GetCurrentThreadId();
		let attached = foreground_thread != 0
			&& foreground_thread != own_thread
			&& AttachThreadInput(foreground_thread, own_thread, true).as_bool();
		let _ = SetForegroundWindow(window);
		let _ = SetFocus(Some(window));
		if attached {
			let _ = AttachThreadInput(foreground_thread, own_thread, false);
		}
	}
}

// Hides the overlay right away and frees the shot.
fn hide(window: HWND) {
	unsafe {
		let _ = ShowWindow(window, SW_HIDE);
	}
	stop_pacing();
	SHOT.with(|cell| cell.borrow_mut().take());
	let _ = with_gpu(|gpu| {
		gpu.release_screenshot();
		Ok(())
	});
}

// Starts closing: fades out (with a flash over `flash` first, if given), then hides.
fn close(window: HWND, flash: Option<Rect>) {
	let animating = with_shot(|shot| {
		let now = Instant::now();
		shot.closing = true;
		if shot.animations.overlay_fade {
			shot.fade.retarget(0.0, now);
		} else {
			shot.fade = Tween::new(0.0, OVERLAY_FADE, cubic_out);
		}
		if let Some(rect) = flash {
			let mut tween = Tween::new(1.0, COPY_FLASH, cubic_out);
			tween.retarget(0.0, now);
			shot.flash = Some((rect, tween));
		}
		shot.is_animating(now)
	});
	if animating == Some(true) {
		redraw(window);
		keep_animating(window);
	} else {
		hide(window);
	}
}

#[derive(Clone, Copy)]
enum Action {
	Copy,
	CopyWhole,
	Save,
	Close,
}

impl Action {
	// The action bound to `chord`, if any. A chord bound twice does the first of these.
	fn for_chord(keys: &Keys, chord: Chord) -> Option<Self> {
		[
			(keys.close, Self::Close),
			(keys.copy, Self::Copy),
			(keys.save, Self::Save),
			(keys.copy_whole, Self::CopyWhole),
		]
		.into_iter()
		.find(|(bound, _)| *bound == chord)
		.map(|(_, action)| action)
	}

	fn run(self, window: HWND) {
		match self {
			Self::Copy => copy(window, CopyMode::Selection),
			Self::CopyWhole => copy(window, CopyMode::Whole),
			Self::Save => save(window),
			Self::Close => close_or_clear(window),
		}
	}
}

// Esc by default: clears the box if there is one (and it isn't being dragged), otherwise closes the overlay.
fn close_or_clear(window: HWND) {
	let cleared = with_shot(|shot| {
		let clear = shot.selection.rect().is_some() && !shot.selection.is_dragging();
		if clear {
			shot.selection.clear();
		}
		clear
	});
	match cleared {
		Some(true) => {
			redraw(window);
			keep_animating(window);
		}
		_ => close(window, None),
	}
}

// Shows the confirmation for quitting snap.
fn ask_to_quit(window: HWND) {
	let Some((scale, screen, animate)) = with_shot(|shot| {
		(
			shot.scale,
			(shot.image.width, shot.image.height),
			shot.animations.enabled,
		)
	}) else {
		return;
	};
	let confirm = with_gpu(|gpu| {
		Confirm::new(
			&gpu.d2d,
			&gpu.write,
			scale,
			screen,
			"Quit snap?",
			"Quit",
			"Cancel",
			animate,
		)
	});
	match confirm {
		Ok(confirm) => {
			with_shot(|shot| {
				shot.pressed = None;
				shot.confirm = Some(confirm);
			});
		}
		Err(err) => eprintln!("snap: couldn't show the quit confirmation: {err}"),
	}
	redraw(window);
	keep_animating(window);
}

// Answers the quit confirmation: quits snap, or fades the confirmation out.
fn answer(window: HWND, choice: Choice) {
	if choice == Choice::Primary {
		let _ = unsafe { DestroyWindow(window) };
		return;
	}
	with_shot(|shot| {
		if let Some(confirm) = shot.active_confirm() {
			confirm.answer(Instant::now());
		}
	});
	redraw(window);
	keep_animating(window);
}

// What a click picked: a button, or an answer to the confirmation.
enum Click {
	Button(Button),
	Choice(Choice),
}

#[derive(Clone, Copy)]
enum CopyMode {
	Whole,
	// the box, or the whole screen if there is no box
	Selection,
}

// Copies to the clipboard, then closes with a flash if the settings ask for one.
fn copy(window: HWND, mode: CopyMode) {
	let copied = with_shot(|shot| {
		let full = Rect {
			left: 0,
			top: 0,
			right: shot.image.width,
			bottom: shot.image.height,
		};
		let (rect, flash) = match (mode, shot.selection.rect()) {
			(CopyMode::Selection, Some(rect)) => (rect, shot.animations.copy_flash),
			_ => (full, shot.animations.copy_flash_full_screen),
		};
		let image = if rect == full {
			shot.image.clone()
		} else {
			shot.image.crop(rect)
		};
		(image, flash.then_some(rect))
	});
	let Some((image, flash)) = copied else { return };
	if let Err(err) = copy_image(window, &image) {
		eprintln!("snap: couldn't copy to the clipboard: {err}");
	}
	close(window, flash);
}

// Asks where to save the box (or the whole screen if there is no box), then saves it and closes the overlay.
// Cancelling the dialog goes back to the overlay.
fn save(window: HWND) {
	let image = with_shot(|shot| {
		shot.saving = true;
		match shot.selection.rect() {
			Some(rect) => shot.image.crop(rect),
			None => shot.image.clone(),
		}
	});
	let Some(image) = image else { return };
	// the dialog is owned by the overlay, so it shows above it and keeps it disabled while open
	let path = ask_path(window);
	with_shot(|shot| shot.saving = false);
	let Some(path) = path else { return };

	let result = write_png(&path, &image);
	close(window, None);
	if let Err(err) = result {
		let message = HSTRING::from(format!("Couldn't save '{}':\n{err}", path.display()));
		unsafe { MessageBoxW(None, &message, w!("snap"), MB_ICONERROR) };
	}
}

fn paint(window: HWND) {
	let mut ps = PAINTSTRUCT::default();
	unsafe {
		BeginPaint(window, &mut ps);
		let drawn = with_shot(|shot| with_gpu(|gpu| shot.render(window, gpu)));
		if let Some(Err(err)) = drawn {
			eprintln!("snap: couldn't draw the overlay: {err}");
			// a lost GPU gets a fresh render target on the next paint
			redraw(window);
		}
		let _ = EndPaint(window, &ps);
	}
	// the buttons can appear or move under a still mouse (after a drag, a nudge, or while the box springs); not while
	// Save As has the mouse
	if is_interactive() && with_shot(|shot| shot.saving) == Some(false) {
		set_cursor();
	}
	// drawing can start an animation (the buttons fading or switching layout), so keep frames coming for it
	keep_animating(window);
}

impl Shot {
	// Advances the animations to now and draws a frame.
	fn render(&mut self, window: HWND, gpu: &mut Gpu) -> Result<()> {
		let now = Instant::now();
		let dt = now.duration_since(self.last_frame).as_secs_f32();
		self.last_frame = now;
		let target = self.selection.rect();
		let settings = motion::Settings {
			modifier_spring: self.animations.modifier_spring,
			drag_spring: self.animations.drag_spring,
		};
		self.motion.step(target, self.selection.box_id(), dt, settings);
		self.old_boxes.retain(|(_, fade)| !fade.is_done(now));
		if self.flash.is_some_and(|(_, flash)| flash.is_done(now)) {
			self.flash = None;
		}

		let drawn = self.motion.drawn();
		self.buttons_fade
			.retarget(if target.is_some() { 1.0 } else { 0.0 }, now);
		// the labels show the real box, placed by the drawn one, and so do the buttons
		let mut labels = Vec::new();
		if let (Some(at), Some(real)) = (drawn, target) {
			let gap = scaled(LABEL_GAP, self.scale);
			let screen = (self.image.width, self.image.height);
			let measure = |text: String| {
				text_layout(&gpu.write, &self.label_format, &text)
					.map(|(layout, width, height)| (layout, width.ceil() as i32, height.ceil() as i32))
			};
			let (position, position_width, position_height) = measure(format!("{}, {}", real.left, real.top))?;
			let (size, size_width, size_height) = measure(format!("{} × {}", real.width(), real.height()))?;
			let buttons = (self.buttons.width(), self.buttons.height());
			let size_label = (size_width, size_height);

			// the buttons spring between beside the size label and under it as the box's width crosses over
			self.stacked = is_stacked(at, size_width, buttons.0, gap, screen.0);
			let goal = if self.stacked { 1.0 } else { 0.0 };
			if !self.animations.enabled || self.stack_box != self.selection.box_id() {
				self.stack_box = self.selection.box_id();
				self.stack.snap(goal);
			} else {
				self.stack.step(goal, dt.min(MAX_STEP), STACK_SETTLE);
				if (self.stack.value - goal).abs() < ARRIVED {
					self.stack.snap(goal);
				}
			}
			let beside = under_box(at, size_label, buttons, gap, screen, false);
			let stacked = under_box(at, size_label, buttons, gap, screen, true);
			let under = beside.lerp(&stacked, self.stack.value);

			let (x, y) = position_label(at, position_width, position_height, gap, &under, screen.0);
			let origin = |x: i32, y: i32| Vector2 {
				X: x as f32,
				Y: y as f32,
			};
			labels.push((origin(x, y), position));
			labels.push((origin(under.label.left, under.label.top), size));
			self.row = Some(under.buttons);
		}
		let hovered = self.button_at(self.hover);
		let highlight = match (drawn, self.selection.highlight(self.hover)) {
			(Some(rect), Some(grip)) if hovered.is_none() => {
				let length = scaled(HIGHLIGHT_LENGTH, self.scale);
				highlight(rect, grip, scaled(HIGHLIGHT_THICKNESS, self.scale), length)
			}
			_ => Vec::new(),
		};
		let buttons_fade = self.buttons_fade.value(now);
		let buttons = match self.row {
			Some(row) if buttons_fade > 0.0 => Some((&self.buttons, row, hovered, buttons_fade)),
			_ => None,
		};
		let mut dotted: Vec<(Rect, f32)> = self
			.old_boxes
			.iter()
			.map(|(rect, fade)| (*rect, fade.value(now)))
			.collect();
		if self.old_box_outline
			&& let Some(origin) = self.selection.drag_origin()
		{
			dotted.push((origin, 1.0));
		}
		if self.new_box_outline
			&& let Some(target) = target
			&& self.motion.lag(Some(target)) > TARGET_OUTLINE_LAG
		{
			dotted.push((target, 1.0));
		}

		let context = self.hint_context();
		if let Some(hints) = &mut self.hints {
			hints.update(context, |code| key_down(VIRTUAL_KEY(code)), now);
		}
		if self.confirm.as_ref().is_some_and(|confirm| confirm.is_gone(now)) {
			self.confirm = None;
		}
		let ui = self.fade.value(now);
		let scene = Scene {
			dim: ui,
			ui,
			selection: drawn,
			highlight: &highlight,
			labels: &labels,
			label_stroke: scaled(LABEL_STROKE, self.scale) as f32,
			dotted: &dotted,
			flash: self.flash.map(|(rect, flash)| (rect, flash.value(now))),
			hints: self.hints.as_ref().map(|hints| (hints, self.hints_fade.value(now))),
			buttons,
			close: (
				&self.buttons,
				self.buttons.close_rect(self.image.width),
				hovered == Some(Button::Close),
			),
			confirm: (self.confirm.as_ref()).map(|confirm| (confirm, confirm.hit(self.hover), now)),
		};
		gpu.prepare(window, self.image.width, self.image.height, self.hardware)?;
		gpu.upload(&self.image)?;
		gpu.draw(&scene)
	}

	// The button under `p`. The box's buttons are where they were last drawn, so they work where they show while the
	// box springs, and take no clicks while there's no box (fading out). No button takes clicks mid-drag or while the
	// confirmation is up.
	fn button_at(&self, p: Point) -> Option<Button> {
		if self.selection.is_dragging() || self.is_confirming() {
			return None;
		}
		if self.buttons.close_rect(self.image.width).contains(p) {
			return Some(Button::Close);
		}
		self.selection.rect()?;
		self.buttons.hit(self.row?, p)
	}

	// The confirmation, while it's up and unanswered.
	fn active_confirm(&mut self) -> Option<&mut Confirm> {
		self.confirm.as_mut().filter(|confirm| !confirm.is_answered())
	}

	fn is_confirming(&self) -> bool {
		self.confirm.as_ref().is_some_and(|confirm| !confirm.is_answered())
	}

	// What the hints should describe: whether there's a box, and what the mouse would act on.
	fn hint_context(&self) -> Context {
		let pointer = match self.pressed.or(self.button_at(self.hover)) {
			Some(Button::Copy) => Pointer::Copy,
			Some(Button::Save) => Pointer::Save,
			Some(Button::Clear) => Pointer::Clear,
			Some(Button::Close) => Pointer::Close,
			None => match self.selection.active_hit(self.hover) {
				Hit::Outside => Pointer::Outside,
				Hit::Move => Pointer::Move,
				Hit::Resize(_) => Pointer::Resize,
			},
		};
		Context {
			has_box: self.selection.rect().is_some(),
			pointer,
		}
	}

	// Fully shown, or hidden while the mouse is over them.
	fn hints_target(&self) -> f32 {
		match &self.hints {
			Some(hints) if hints.rect.contains(self.hover) => 0.0,
			_ => 1.0,
		}
	}

	// Whether anything is still moving, so frames should keep coming.
	fn is_animating(&self, now: Instant) -> bool {
		!self.fade.is_done(now)
			|| !self.hints_fade.is_done(now)
			|| (self.hints.as_ref()).is_some_and(|hints| hints.is_animating(self.hint_context(), now))
			|| (self.confirm.as_ref()).is_some_and(|confirm| confirm.is_animating(now))
			|| !self.old_boxes.is_empty()
			|| self.flash.is_some()
			|| !self.motion.is_settled(self.selection.rect())
			|| !self.buttons_fade.is_done(now)
			|| self.stack.value != if self.stacked { 1.0 } else { 0.0 }
	}

	// Ends a drag at `p`, leaving the old box's outline to fade out (or vanish, with animations off).
	fn release(&mut self, p: Point) {
		let origin = self.selection.drag_origin();
		self.selection.set_modifiers(held_modifiers());
		self.selection.release(p);
		if let Some(origin) = origin
			&& self.old_box_outline
			&& self.animations.enabled
		{
			let mut fade = Tween::new(1.0, OLD_BOX_FADE, cubic_out);
			fade.retarget(0.0, Instant::now());
			self.old_boxes.push((origin, fade));
		}
	}
}

// Starts frames coming if something is animating and they aren't already.
fn keep_animating(window: HWND) {
	let start = with_shot(|shot| {
		let start = !shot.pacing && shot.is_animating(Instant::now());
		shot.pacing |= start;
		start
	});
	if start == Some(true) {
		start_pacing(window);
	}
}

static PACING: AtomicBool = AtomicBool::new(false);
static FRAME_POSTED: AtomicBool = AtomicBool::new(false); // so frames can't pile up if a redraw runs long
static PACER: OnceLock<Thread> = OnceLock::new();

// Makes the vsync thread post `WM_SNAP_FRAME` to `window` after every display refresh, starting the thread the first
// time.
fn start_pacing(window: HWND) {
	PACING.store(true, Ordering::Release);
	// HWND isn't Send, so the handle crosses to the thread as a number
	let handle = window.0 as usize;
	PACER
		.get_or_init(|| thread::spawn(move || pace(HWND(handle as *mut _))).thread().clone())
		.unpark();
}

fn stop_pacing() {
	PACING.store(false, Ordering::Release);
}

fn pace(window: HWND) {
	loop {
		if !PACING.load(Ordering::Acquire) {
			thread::park();
			continue;
		}
		// blocks until the next vsync
		if unsafe { DwmFlush() }.is_err() {
			thread::sleep(FALLBACK_FRAME);
		}
		if PACING.load(Ordering::Acquire) && !FRAME_POSTED.swap(true, Ordering::AcqRel) {
			let _ = unsafe { PostMessageW(Some(window), WM_SNAP_FRAME, WPARAM(0), LPARAM(0)) };
		}
	}
}

fn on_frame(window: HWND) {
	// this redraw also paints the final frame of anything that just finished
	redraw(window);
	let state = with_shot(|shot| {
		let animating = shot.is_animating(Instant::now());
		shot.pacing = animating;
		(animating, shot.closing)
	});
	match state {
		Some((true, _)) => {}
		Some((false, true)) => hide(window),
		_ => stop_pacing(),
	}
}

fn key_down(key: VIRTUAL_KEY) -> bool {
	unsafe { GetKeyState(key.0 as i32) < 0 }
}

fn held_modifiers() -> Modifiers {
	Modifiers {
		center: key_down(VK_CONTROL),
		keep_shape: key_down(VK_SHIFT),
	}
}

// Picks up Ctrl and Shift, springing the box to its new shape if that changed it.
fn update_modifiers(window: HWND) {
	let changed = with_shot(|shot| {
		let before = shot.selection.rect();
		shot.selection.set_modifiers(held_modifiers());
		let changed = before != shot.selection.rect();
		if changed {
			shot.motion.modifiers_changed();
		}
		changed
	});
	if changed == Some(true) {
		redraw(window);
		keep_animating(window);
	}
}

fn cursor_for(hit: Hit) -> PCWSTR {
	match hit {
		Hit::Outside => IDC_CROSS,
		Hit::Move => IDC_SIZEALL,
		Hit::Resize(Grip {
			left,
			top,
			right,
			bottom,
		}) => {
			if (left && top) || (right && bottom) {
				IDC_SIZENWSE
			} else if (right && top) || (left && bottom) {
				IDC_SIZENESW
			} else if left || right {
				IDC_SIZEWE
			} else {
				IDC_SIZENS
			}
		}
	}
}

// Sets the cursor for what's under the mouse. Windows only asks for it when the mouse moves, so this also runs after
// each paint. `false` if the overlay is closed.
fn set_cursor() -> bool {
	let mut cursor = POINT::default();
	let _ = unsafe { GetCursorPos(&mut cursor) };
	// the window sits at the screen's origin, so screen and window coordinates match
	let hover = Point {
		x: cursor.x,
		y: cursor.y,
	};
	let name = with_shot(|shot| match shot.pressed.or(shot.button_at(hover)) {
		_ if shot.is_confirming() => IDC_ARROW,
		Some(_) => IDC_ARROW,
		None => cursor_for(shot.selection.active_hit(hover)),
	});
	let Some(name) = name else { return false };
	if let Ok(cursor) = unsafe { LoadCursorW(None, name) } {
		unsafe { SetCursor(Some(cursor)) };
	}
	true
}

fn point_from(lparam: LPARAM) -> Point {
	// the low and high words are signed, so this sign-extends them
	Point {
		x: lparam.0 as i16 as i32,
		y: (lparam.0 >> 16) as i16 as i32,
	}
}

fn redraw(window: HWND) {
	unsafe {
		let _ = InvalidateRect(Some(window), None, false);
	}
}

// Whether the overlay is taking input: open, and not fading out.
fn is_interactive() -> bool {
	with_shot(|shot| !shot.closing) == Some(true)
}

unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	match msg {
		WM_SNAP_KEY => {
			match with_shot(|shot| (shot.saving, shot.closing)) {
				// the capture key does nothing while the Save As dialog is open
				Some((true, _)) => {}
				// a fresh capture right away, skipping the rest of the fade-out
				Some((_, true)) => {
					hide(window);
					open_or_report(window);
				}
				// the capture key does whatever it's also bound to in the overlay (copying the whole screen by default),
				// and nothing while the confirmation is up
				Some(_) => {
					let action = with_shot(|shot| {
						(!shot.is_confirming())
							.then(|| Action::for_chord(&shot.keys, shot.keys.capture))
							.flatten()
					})
					.flatten();
					if let Some(action) = action {
						action.run(window);
					}
				}
				None => open_or_report(window),
			}
			LRESULT(0)
		}
		tray::WM_TRAY => {
			match (lparam.0 & 0xFFFF) as u32 {
				WM_LBUTTONUP if with_shot(|_| ()).is_none() => open_or_report(window),
				WM_RBUTTONUP => {
					let command = tray::show_menu(window);
					if command == tray::MENU_QUIT {
						let _ = unsafe { DestroyWindow(window) };
					}
				}
				_ => {}
			}
			LRESULT(0)
		}
		msg if tray::is_taskbar_created(msg) => {
			if let Err(err) = tray::add(window) {
				eprintln!("snap: couldn't add the tray icon again: {err}");
			}
			LRESULT(0)
		}
		WM_SNAP_FRAME => {
			FRAME_POSTED.store(false, Ordering::Release);
			on_frame(window);
			LRESULT(0)
		}
		// the paint covers the whole window, so erasing first would only flicker
		WM_ERASEBKGND => LRESULT(1),
		WM_PAINT => {
			paint(window);
			LRESULT(0)
		}
		WM_LBUTTONDOWN if is_interactive() => {
			unsafe { SetCapture(window) };
			let p = point_from(lparam);
			with_shot(|shot| {
				shot.hover = p;
				if let Some(confirm) = shot.active_confirm() {
					confirm.pressed = confirm.hit(p);
					return;
				}
				shot.pressed = shot.button_at(p);
				if shot.pressed.is_none() {
					shot.selection.set_modifiers(held_modifiers());
					shot.selection.press(p);
				}
			});
			redraw(window);
			LRESULT(0)
		}
		WM_MOUSEMOVE if is_interactive() => {
			let p = point_from(lparam);
			let changed = with_shot(|shot| {
				if let Some(confirm) = &shot.confirm
					&& !confirm.is_answered()
				{
					let changed = confirm.hit(shot.hover) != confirm.hit(p);
					shot.hover = p;
					return changed;
				}
				let state = |shot: &Shot, p| (shot.selection.rect(), shot.selection.highlight(p), shot.button_at(p));
				let before = state(shot, shot.hover);
				shot.hover = p;
				if shot.selection.is_dragging() {
					shot.selection.set_modifiers(held_modifiers());
					shot.selection.drag_to(p);
				}
				let target = shot.hints_target();
				shot.hints_fade.retarget(target, Instant::now());
				before != state(shot, p)
			});
			if changed == Some(true) {
				redraw(window);
			}
			keep_animating(window);
			LRESULT(0)
		}
		WM_LBUTTONUP if is_interactive() => {
			let p = point_from(lparam);
			let clicked = with_shot(|shot| {
				shot.hover = p;
				if let Some(confirm) = shot.active_confirm() {
					let pressed = confirm.pressed.take();
					return pressed
						.filter(|&choice| confirm.hit(p) == Some(choice))
						.map(Click::Choice);
				}
				match shot.pressed.take() {
					Some(pressed) => (shot.button_at(p) == Some(pressed)).then_some(Click::Button(pressed)),
					None => {
						shot.release(p);
						None
					}
				}
			});
			let _ = unsafe { ReleaseCapture() };
			match clicked.flatten() {
				Some(Click::Button(Button::Copy)) => copy(window, CopyMode::Selection),
				Some(Click::Button(Button::Save)) => save(window),
				Some(Click::Button(Button::Clear)) => {
					with_shot(|shot| shot.selection.clear());
				}
				// Shift+click quits snap, after asking
				Some(Click::Button(Button::Close)) if key_down(VK_SHIFT) => ask_to_quit(window),
				Some(Click::Button(Button::Close)) => close(window, None),
				Some(Click::Choice(choice)) => answer(window, choice),
				None => {}
			}
			redraw(window);
			keep_animating(window);
			LRESULT(0)
		}
		// the mouse was taken away mid-drag (for example by a system dialog), so end the drag where it was
		WM_CAPTURECHANGED => {
			with_shot(|shot| {
				shot.pressed = None;
				if let Some(confirm) = shot.active_confirm() {
					confirm.pressed = None;
				}
				if shot.selection.is_dragging() {
					shot.release(shot.hover);
				}
			});
			redraw(window);
			keep_animating(window);
			LRESULT(0)
		}
		WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
			if !set_cursor() {
				return unsafe { DefWindowProcW(window, msg, wparam, lparam) };
			}
			LRESULT(1)
		}
		// Quit from the tray, or a newer copy closed this one (WM_CLOSE destroys the window by default)
		WM_DESTROY => {
			tray::remove(window);
			// released here, while the window still exists: left to the thread-local's destructor at process exit,
			// releasing the Direct2D objects made the process exit with code 2170 (0x87A, the DXGI facility)
			GPU.with_borrow_mut(|gpu| *gpu = None);
			unsafe { PostQuitMessage(0) };
			LRESULT(0)
		}
		// redrawn for the hints, which light up held keys
		WM_KEYUP if is_interactive() => {
			update_modifiers(window);
			redraw(window);
			LRESULT(0)
		}
		WM_SYSKEYUP if is_interactive() => {
			redraw(window);
			unsafe { DefWindowProcW(window, msg, wparam, lparam) }
		}
		// with Alt held (or for F10) keys come as WM_SYSKEYDOWN, which goes on to Windows unless it's a keybind
		WM_KEYDOWN | WM_SYSKEYDOWN if is_interactive() => {
			let key = wparam.0 as u16;
			redraw(window); // for the hints, which light up held keys
			// the confirmation takes only Enter and Esc
			if with_shot(|shot| shot.is_confirming()) == Some(true) {
				match key {
					k if k == VK_RETURN.0 => answer(window, Choice::Primary),
					k if k == VK_ESCAPE.0 => answer(window, Choice::Secondary),
					_ => {}
				}
				return LRESULT(0);
			}
			update_modifiers(window);
			let [ctrl, shift, alt, win] = [
				key_down(VK_CONTROL),
				key_down(VK_SHIFT),
				key_down(VK_MENU),
				key_down(VK_LWIN) || key_down(VK_RWIN),
			];
			let chord = Chord {
				ctrl,
				shift,
				alt,
				win,
				key,
			};
			if let Some(action) = with_shot(|shot| Action::for_chord(&shot.keys, chord)).flatten() {
				action.run(window);
				return LRESULT(0);
			}
			let (dx, dy) = match key {
				k if k == VK_LEFT.0 => (-1, 0),
				k if k == VK_RIGHT.0 => (1, 0),
				k if k == VK_UP.0 => (0, -1),
				k if k == VK_DOWN.0 => (0, 1),
				_ if msg == WM_SYSKEYDOWN => return unsafe { DefWindowProcW(window, msg, wparam, lparam) },
				_ => return LRESULT(0),
			};
			with_shot(|shot| shot.selection.nudge(dx, dy));
			redraw(window);
			keep_animating(window);
			LRESULT(0)
		}
		// input while closing is ignored
		WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP | WM_KEYDOWN | WM_KEYUP => LRESULT(0),
		_ => unsafe { DefWindowProcW(window, msg, wparam, lparam) },
	}
}
