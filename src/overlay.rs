// overlay

// The full-screen window that shows the frozen screenshot and lets the user draw a box on it. Opened from the tray's
// Settings, it shows the settings menu over the live screen instead, with no screenshot.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Factory;
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F4, VK_LEFT,
	VK_LWIN, VK_MENU, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
	CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
	GetWindowLongPtrW, GetWindowThreadProcessId, HTCLIENT, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_SIZEALL,
	IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, LWA_ALPHA, LoadCursorW, MB_ICONERROR, MessageBoxW,
	PostMessageW, PostQuitMessage, RegisterClassExW, SC_CLOSE, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SWP_SHOWWINDOW,
	SetCursor, SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_APP,
	WM_CAPTURECHANGED, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
	WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW,
	WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::anim::{Spring, Tween, cubic_out};
use crate::buttons::{Button, Buttons, History};
use crate::capture::{Image, capture_screen};
use crate::clipboard::copy_image;
use crate::config::{self, AltF4, Animations, Config, Keys};
use crate::confirm::{Choice, Confirm};
use crate::gpu::{Gpu, Scene, text_format, text_layout};
use crate::hints::{Context, Hints, Mode, Pointer};
use crate::hook;
use crate::keys::Chord;
use crate::motion::{self, BoxMotion};
use crate::render::{highlight, is_stacked, position_label, scaled, under_box};
use crate::save::{ask_path, write_png};
use crate::selection::{Grip, Hit, Modifiers, Point, Rect, Selection};
use crate::settings::{self, Settings};
use crate::tray;

// Posted by the keyboard hook when the capture key is pressed, or with `hook::RECORDED` for a key the settings menu is
// waiting for.
pub const WM_SNAP_KEY: u32 = WM_APP + 1;

// Posted by `redraw` while the window is see-through, which gets no `WM_PAINT`.
const WM_SNAP_DRAW: u32 = WM_APP + 4;

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
const MAIN_FADE: Duration = Duration::from_millis(150); // the box and its UI hiding behind a dialog, like it fades
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
	image: Image,   // with no pixels when `live`
	live: bool,     // opened from the tray: the settings menu over the live screen, with no screenshot or box
	config: Config, // as read when it opened or last saved from the settings menu
	selection: Selection,
	hover: Point,
	scale: f32,
	hardware: bool,
	old_box_outline: bool,
	new_box_outline: bool,
	close_button: bool, // whether the config wants it at all
	animations: Animations,
	keys: Keys,
	label_format: IDWriteTextFormat,
	hints: Option<Hints>,
	hints_fade: Tween,
	buttons: Buttons,
	pressed: Option<Button>, // the button the mouse went down on, which clicks if it comes up there too
	row: Option<Rect>,       // where the buttons were last drawn, kept while they fade out after the box is cleared
	buttons_fade: Tween,
	close_fade: Tween, // the close button, hidden while the box reaches it
	stack: Spring,     // 0 with the buttons beside the size label, 1 with them under it, springing between
	stacked: bool,     // which one `stack` is heading for
	stack_box: u32,    // the box `stack` belongs to; a new box jumps to its layout
	fade: Tween,       // the overlay fading in and out: the dimming and everything drawn over the screenshot
	main_fade: Tween,  // the box and everything around it, hidden while a dialog is up
	motion: BoxMotion,
	old_boxes: Vec<(Rect, Tween)>, // dotted outlines of boxes before a drag, fading out
	flash: Option<(Rect, Tween)>,
	last_frame: Instant,
	pacing: bool,               // the vsync thread is posting frames
	saving: bool,               // the Save As dialog is open
	closing: bool,              // fading out; input is ignored
	close_from: f32,            // `fade`'s value when closing started
	quitting: bool,             // fading out, then quitting snap
	confirm: Option<Confirm>,   // asking whether to quit snap; kept while it fades out after an answer
	settings: Option<Settings>, // the settings menu; kept while it fades out after closing
}

thread_local! {
	// Whether the window is set up for see-through drawing with `UpdateLayeredWindow` (see `set_live_style`).
	static LIVE_STYLE: Cell<bool> = const { Cell::new(false) };
	static DRAW_POSTED: Cell<bool> = const { Cell::new(false) }; // a `WM_SNAP_DRAW` is on its way
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
		// layered, so closing can fade the whole window out, crossfading the screenshot to the live screen under it
		CreateWindowExW(
			WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
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

// Opens the overlay on a fresh screenshot, or with `live`, the settings menu over the live screen.
fn open(window: HWND, live: bool) -> Result<()> {
	let config = config::load();
	let image = if live {
		let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
		Image {
			width,
			height,
			pixels: Vec::new(),
		}
	} else {
		capture_screen()?
	};
	let (width, height) = (image.width, image.height);
	let animations = config.animations.active();
	let scale = unsafe { GetDpiForWindow(window) } as f32 / 96.0;
	set_live_style(window, live);

	let (d2d, write, label_format, buttons, settings) = with_gpu(|gpu| {
		if !live {
			gpu.prepare(window, width, height, config.hardware_acceleration)?;
			gpu.upload(&image)?;
		}
		let settings = live
			.then(|| Settings::new(&gpu.write, scale, (width, height), config, animations.enabled))
			.transpose()?;
		Ok((
			gpu.d2d.clone(),
			gpu.write.clone(),
			text_format(&gpu.write, scaled(LABEL_FONT_SIZE, scale) as f32)?,
			Buttons::new(&gpu.d2d, scale)?,
			settings,
		))
	})?;
	let start = Context {
		mode: if live { Mode::Settings } else { Mode::Box },
		has_box: false,
		can_undo: false,
		can_redo: false,
		recording: false,
		pointer: Pointer::Outside,
	};
	let hints = make_hints(&d2d, &write, scale, height, &config, start);

	let mut cursor = POINT::default();
	let _ = unsafe { GetCursorPos(&mut cursor) };
	let now = Instant::now();
	// the first frame looks exactly like the screen, then the dimming and UI fade in
	let mut fade = Tween::new(if animations.overlay_fade { 0.0 } else { 1.0 }, OVERLAY_FADE, cubic_out);
	fade.retarget(1.0, now);
	let mut shot = Shot {
		image,
		live,
		config,
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
		old_box_outline: config.ui.old_box_outline,
		new_box_outline: config.ui.new_box_outline,
		close_button: config.ui.close_button,
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
		close_fade: Tween::new(
			1.0,
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
		main_fade: Tween::new(
			if live { 0.0 } else { 1.0 },
			if animations.enabled { MAIN_FADE } else { Duration::ZERO },
			cubic_out,
		),
		motion: BoxMotion::default(),
		old_boxes: Vec::new(),
		flash: None,
		last_frame: now,
		pacing: false,
		saving: false,
		closing: false,
		close_from: 1.0,
		quitting: false,
		confirm: None,
		settings,
	};
	// no fade when the overlay opens with the mouse already over the hints
	shot.hints_fade = Tween::new(shot.hints_target(), HINT_FADE, cubic_out);
	SHOT.with(|cell| *cell.borrow_mut() = Some(shot));

	unsafe {
		if live {
			// a see-through window shows nothing until its first frame
			render_or_report(window);
		} else {
			// fully opaque again after the last close faded it out
			set_window_opacity(window, 1.0)?;
		}
		SetWindowPos(window, Some(HWND_TOPMOST), 0, 0, width, height, SWP_SHOWWINDOW)?;
		if !live {
			// paint right away, so nothing stale shows before the first frame
			let _ = UpdateWindow(window);
		}
		bring_to_front(window);
	}
	keep_animating(window);
	Ok(())
}

fn open_or_report(window: HWND, live: bool) {
	if let Err(err) = open(window, live) {
		eprintln!("snap: couldn't open the overlay: {err}");
		hide(window);
	}
}

// The hints for `config`'s keys, if it wants them, starting out showing `context`.
fn make_hints(
	d2d: &ID2D1Factory,
	write: &IDWriteFactory,
	scale: f32,
	height: i32,
	config: &Config,
	context: Context,
) -> Option<Hints> {
	if !config.ui.hints {
		return None;
	}
	let animate = config.animations.active().enabled;
	match Hints::new(d2d, write, scale, height, &config.keys, context, animate) {
		Ok(hints) => Some(hints),
		Err(err) => {
			eprintln!("snap: couldn't lay out the hints: {err}");
			None
		}
	}
}

// Switches the window between showing a screenshot, faded as a whole with `SetLayeredWindowAttributes`, and see-through
// drawing with `UpdateLayeredWindow`. Windows only allows one after the other once the layered style is cleared and set
// again.
fn set_live_style(window: HWND, live: bool) {
	if LIVE_STYLE.replace(live) == live {
		return;
	}
	unsafe {
		let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
		SetWindowLongPtrW(window, GWL_EXSTYLE, style & !(WS_EX_LAYERED.0 as isize));
		SetWindowLongPtrW(window, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
	}
	if !live {
		// made for the window as it was before
		let _ = with_gpu(|gpu| {
			gpu.release_target();
			Ok(())
		});
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

// Hides the overlay right away and frees the shot, then quits snap if it was quitting.
fn hide(window: HWND) {
	unsafe {
		let _ = ShowWindow(window, SW_HIDE);
	}
	stop_pacing();
	hook::record(false);
	let quitting = SHOT
		.with(|cell| cell.borrow_mut().take())
		.is_some_and(|shot| shot.quitting);
	let _ = with_gpu(|gpu| {
		gpu.release_screenshot();
		Ok(())
	});
	if quitting {
		let _ = unsafe { DestroyWindow(window) };
	}
}

// Quits snap, fading the overlay out first if it's open.
fn quit(window: HWND) {
	if with_shot(|shot| shot.quitting = true).is_some() {
		close(window, None);
	} else {
		let _ = unsafe { DestroyWindow(window) };
	}
}

// Sets the opacity of the whole window, 0 to 1.
fn set_window_opacity(window: HWND, opacity: f32) -> Result<()> {
	let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
	unsafe { SetLayeredWindowAttributes(window, COLORREF(0), alpha, LWA_ALPHA) }
}

// Starts closing: fades out (with a flash over `flash` first, if given), then hides.
fn close(window: HWND, flash: Option<Rect>) {
	let animating = with_shot(|shot| {
		let now = Instant::now();
		shot.closing = true;
		shot.close_from = shot.fade.value(now);
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
	// the normal cursor right away, not once the fade-out is done (Windows only asks again when the mouse moves)
	set_cursor();
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
	Undo,
	Redo,
	Clear,
	Settings,
}

impl Action {
	// The action bound to `chord`, if any. `cancel` clears the box when there's one to clear and wins over the rest (it
	// shares Esc with `close` by default); after that, a chord bound twice does the first of these, matching the order
	// `Keys::conflicts` warns in.
	fn for_chord(keys: &Keys, chord: Chord, can_clear: bool) -> Option<Self> {
		if can_clear && chord == keys.cancel {
			return Some(Self::Clear);
		}
		[
			(keys.copy, Self::Copy),
			(keys.copy_whole, Self::CopyWhole),
			(keys.save, Self::Save),
			(keys.close, Self::Close),
			(keys.undo, Self::Undo),
			(keys.redo, Self::Redo),
			(keys.settings, Self::Settings),
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
			Self::Close => close(window, None),
			Self::Undo => change(window, Selection::undo),
			Self::Redo => change(window, Selection::redo),
			Self::Clear => change(window, Selection::clear),
			Self::Settings => open_settings(window),
		}
	}
}

// Whether there's a box for `cancel` to clear: one that isn't being dragged.
fn can_clear(shot: &Shot) -> bool {
	shot.selection.rect().is_some() && !shot.selection.is_dragging()
}

// Changes the box with `f` (clearing it, or undo and redo), animating whatever that starts.
fn change(window: HWND, f: fn(&mut Selection)) {
	with_shot(|shot| f(&mut shot.selection));
	redraw(window);
	keep_animating(window);
}

// Shows the confirmation for quitting snap, ending any drag.
fn ask_to_quit(window: HWND) {
	// WM_CAPTURECHANGED ends the drag
	let _ = unsafe { ReleaseCapture() };
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
	// quitting leaves the confirmation up, fading out with the rest
	if choice == Choice::Primary {
		quit(window);
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

// Opens the settings menu over the box, ending any drag.
fn open_settings(window: HWND) {
	// WM_CAPTURECHANGED ends the drag
	let _ = unsafe { ReleaseCapture() };
	let Some((scale, screen, config, animate)) = with_shot(|shot| {
		(
			shot.scale,
			(shot.image.width, shot.image.height),
			shot.config,
			shot.animations.enabled,
		)
	}) else {
		return;
	};
	match with_gpu(|gpu| Settings::new(&gpu.write, scale, screen, config, animate)) {
		Ok(settings) => {
			with_shot(|shot| {
				shot.pressed = None;
				shot.settings = Some(settings);
			});
		}
		Err(err) => eprintln!("snap: couldn't show the settings: {err}"),
	}
	redraw(window);
	keep_animating(window);
}

// Closes the settings menu, going back to the box, or closes the overlay if it was opened from the tray.
fn close_settings(window: HWND) {
	hook::record(false);
	let live = with_shot(|shot| {
		// from the tray, the menu fades out with the rest
		if !shot.live
			&& let Some(settings) = &mut shot.settings
		{
			settings.close(Instant::now());
		}
		shot.live
	});
	if live == Some(true) {
		close(window, None);
	} else {
		redraw(window);
		keep_animating(window);
	}
}

// The tray's Settings: the menu over the live screen if the overlay is closed, or over the box if it's open.
fn settings_from_tray(window: HWND) {
	match with_shot(|shot| (shot.saving || shot.quitting, shot.closing, shot.mode())) {
		Some((true, _, _)) => {}
		Some((_, true, _)) => {
			hide(window);
			open_or_report(window, true);
		}
		Some((_, _, Mode::Box)) => open_settings(window),
		Some(_) => {}
		None => open_or_report(window, true),
	}
}

// Changes the settings menu with `f` (a click, or a key for a keybind), which says whether the config changed, then
// saves and applies the config if it did.
fn change_settings(window: HWND, f: impl FnOnce(&mut Settings) -> Result<bool>) {
	let result = with_shot(|shot| {
		let settings = shot.active_settings()?;
		let result = f(settings).map(|changed| changed.then_some(settings.config));
		Some((result, settings.is_waiting()))
	})
	.flatten();
	// the hook sends keys here while a keybind waits for one
	hook::record(result.as_ref().is_some_and(|(_, waiting)| *waiting));
	match result {
		Some((Ok(Some(config)), _)) => apply(window, config),
		Some((Err(err), _)) => eprintln!("snap: couldn't lay out the settings: {err}"),
		_ => {}
	}
	redraw(window);
	keep_animating(window);
}

// Saves `config` to `config.toml` and starts using it.
fn apply(window: HWND, config: Config) {
	if let Err(err) = config::save(&config) {
		eprintln!("snap: couldn't save config.toml: {err}");
	}
	// the config watcher does this too, a moment later
	hook::set_capture(config.keys.capture);
	if let Ok((d2d, write)) = with_gpu(|gpu| Ok((gpu.d2d.clone(), gpu.write.clone()))) {
		with_shot(|shot| shot.apply(config, &d2d, &write));
	}
	redraw(window);
}

// What a click picked: a button, an answer to the confirmation, or a control in the settings menu.
enum Click {
	Button(Button),
	Choice(Choice),
	Setting(settings::Target),
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
		render_or_report(window);
		let _ = EndPaint(window, &ps);
	}
	after_frame(window);
}

fn render_or_report(window: HWND) {
	let drawn = with_shot(|shot| with_gpu(|gpu| shot.render(window, gpu)));
	if let Some(Err(err)) = drawn {
		eprintln!("snap: couldn't draw the overlay: {err}");
		// a lost GPU gets a fresh render target on the next paint
		redraw(window);
	}
}

fn after_frame(window: HWND) {
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
		self.close_fade
			.retarget(if self.is_close_shown() { 1.0 } else { 0.0 }, now);
		self.main_fade
			.retarget(if self.mode() == Mode::Box { 1.0 } else { 0.0 }, now);
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
		} else {
			// a box cleared mid-switch would otherwise leave `stack` short of its goal forever, keeping
			// `is_animating` true, so the overlay never finished closing
			self.stack.snap(if self.stacked { 1.0 } else { 0.0 });
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
			Some(row) if buttons_fade > 0.0 => {
				let history = History {
					undo: self.selection.can_undo(),
					redo: self.selection.can_redo(),
				};
				Some((&self.buttons, row, hovered, history, buttons_fade))
			}
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
		if self.settings.as_ref().is_some_and(|settings| settings.is_gone(now)) {
			self.settings = None;
		}
		// while closing, the scene stays as it was and the whole window fades out instead, so the screenshot crossfades
		// to what's on screen now
		let closing = self.closing && self.close_from > 0.0;
		let window_opacity = if closing {
			self.fade.value(now) / self.close_from
		} else {
			1.0
		};
		let ui = if self.closing {
			if closing && !self.live {
				set_window_opacity(window, window_opacity)?;
			}
			self.close_from
		} else {
			self.fade.value(now)
		};
		let scene = Scene {
			dim: ui,
			ui,
			main: self.main_fade.value(now),
			selection: drawn,
			highlight: &highlight,
			labels: &labels,
			label_stroke: scaled(LABEL_STROKE, self.scale) as f32,
			dotted: &dotted,
			flash: self.flash.map(|(rect, flash)| (rect, flash.value(now))),
			hints: self.hints.as_ref().map(|hints| (hints, self.hints_fade.value(now))),
			buttons,
			close: self.close_button.then(|| {
				(
					&self.buttons,
					self.buttons.close_rect(self.image.width),
					hovered == Some(Button::Close),
					self.close_fade.value(now),
				)
			}),
			gear: self.close_button.then(|| {
				(
					&self.buttons,
					self.buttons.gear_rect(self.image.width),
					hovered == Some(Button::Gear),
					self.close_fade.value(now),
				)
			}),
			confirm: (self.confirm.as_ref()).map(|confirm| (confirm, confirm.hit(self.hover), now)),
			settings: (self.settings.as_ref()).map(|settings| (settings, settings.hit(self.hover), now)),
		};
		if self.live {
			let screen = (self.image.width, self.image.height);
			return gpu.draw_live(window, screen, self.hardware, window_opacity, &scene);
		}
		gpu.prepare(window, self.image.width, self.image.height, self.hardware)?;
		gpu.upload(&self.image)?;
		gpu.draw(&scene)
	}

	// The button under `p`. The box's buttons are where they were last drawn, so they work where they show while the
	// box springs, and take no clicks while there's no box (fading out). No button takes clicks mid-drag or while a
	// dialog is up.
	fn button_at(&self, p: Point) -> Option<Button> {
		if self.selection.is_dragging() || self.mode() != Mode::Box {
			return None;
		}
		if self.is_close_shown() {
			if self.buttons.close_hit_rect(self.image.width).contains(p) {
				return Some(Button::Close);
			}
			if self.buttons.gear_hit_rect(self.image.width).contains(p) {
				return Some(Button::Gear);
			}
		}
		self.selection.rect()?;
		self.buttons.hit(self.row?, p)
	}

	// Whether the close and settings buttons show and take clicks: they're on in the config, and the box (with the
	// margin where it can be grabbed) doesn't reach them, so grabbing the box's corner up there always works.
	fn is_close_shown(&self) -> bool {
		let slop = scaled(HITSLOP_OUTSIDE, self.scale);
		let (close, gear) = (
			self.buttons.close_hit_rect(self.image.width),
			self.buttons.gear_hit_rect(self.image.width),
		);
		let close = Rect {
			left: gear.left,
			top: gear.top.min(close.top),
			right: close.right,
			bottom: gear.bottom.max(close.bottom),
		};
		self.close_button
			&& !(self.selection.rect()).is_some_and(|r| {
				let grabbable = Rect {
					left: r.left - slop,
					top: r.top - slop,
					right: r.right + slop,
					bottom: r.bottom + slop,
				};
				grabbable.intersect(&close).is_some()
			})
	}

	// The confirmation, while it's up and unanswered.
	fn active_confirm(&mut self) -> Option<&mut Confirm> {
		self.confirm.as_mut().filter(|confirm| !confirm.is_answered())
	}

	fn is_confirming(&self) -> bool {
		self.confirm.as_ref().is_some_and(|confirm| !confirm.is_answered())
	}

	// The settings menu, while it's up and not closing.
	fn active_settings(&mut self) -> Option<&mut Settings> {
		self.settings.as_mut().filter(|settings| !settings.is_closed())
	}

	fn is_in_settings(&self) -> bool {
		self.settings.as_ref().is_some_and(|settings| !settings.is_closed())
	}

	// Anything but `Box` hides the box and its UI. Quitting keeps it hidden while everything fades out.
	fn mode(&self) -> Mode {
		if self.is_confirming() || self.quitting {
			Mode::Quit
		} else if self.live || self.is_in_settings() {
			Mode::Settings
		} else {
			Mode::Box
		}
	}

	// What the hints should describe: the mode, whether there's a box, and what the mouse would act on.
	fn hint_context(&self) -> Context {
		let mode = self.mode();
		let confirm_hit = (self.confirm.as_ref()).and_then(|confirm| confirm.hit(self.hover));
		let settings_hit = (self.settings.as_ref()).and_then(|settings| settings.pressed.or(settings.hit(self.hover)));
		let pointer = match self.pressed.or(self.button_at(self.hover)) {
			_ if mode == Mode::Quit => confirm_hit.map_or(Pointer::Outside, Pointer::Choice),
			_ if mode == Mode::Settings => match settings_hit {
				Some(settings::Target::Switch(_)) => Pointer::Switch,
				Some(settings::Target::Bind(_)) => Pointer::Bind,
				Some(settings::Target::AltF4(_)) => Pointer::Pick,
				Some(settings::Target::Reset) => Pointer::Reset,
				None => Pointer::Outside,
			},
			Some(Button::Gear) => Pointer::Gear,
			Some(Button::Undo) => Pointer::Undo,
			Some(Button::Redo) => Pointer::Redo,
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
			mode,
			has_box: self.selection.rect().is_some(),
			can_undo: self.selection.can_undo(),
			can_redo: self.selection.can_redo(),
			recording: self.settings.as_ref().is_some_and(Settings::is_waiting),
			pointer,
		}
	}

	// Starts using `config`, just saved from the settings menu. Animations already set up keep their length.
	fn apply(&mut self, config: Config, d2d: &ID2D1Factory, write: &IDWriteFactory) {
		self.config = config;
		self.hardware = config.hardware_acceleration;
		self.old_box_outline = config.ui.old_box_outline;
		self.new_box_outline = config.ui.new_box_outline;
		self.close_button = config.ui.close_button;
		self.animations = config.animations.active();
		self.keys = config.keys;
		// laid out again for the new keys
		self.hints = make_hints(d2d, write, self.scale, self.image.height, &config, self.hint_context());
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
			|| (self.settings.as_ref()).is_some_and(|settings| settings.is_animating(now))
			|| !self.old_boxes.is_empty()
			|| self.flash.is_some()
			|| !self.motion.is_settled(self.selection.rect())
			|| !self.buttons_fade.is_done(now)
			|| !self.close_fade.is_done(now)
			|| !self.main_fade.is_done(now)
			|| self.stack.value != if self.stacked { 1.0 } else { 0.0 }
	}

	// Whether it's closing and has finished fading out (and flashing). Anything else still moving doesn't matter then.
	fn is_closed(&self, now: Instant) -> bool {
		self.closing && self.fade.is_done(now) && self.flash.is_none_or(|(_, flash)| flash.is_done(now))
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
		let now = Instant::now();
		let animating = shot.is_animating(now);
		shot.pacing = animating;
		(animating, shot.is_closed(now))
	});
	match state {
		Some((_, true)) => hide(window),
		Some((true, false)) => {}
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
		_ if shot.closing || shot.mode() != Mode::Box => IDC_ARROW,
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
	if !LIVE_STYLE.get() {
		unsafe {
			let _ = InvalidateRect(Some(window), None, false);
		}
	} else if !DRAW_POSTED.replace(true) {
		let _ = unsafe { PostMessageW(Some(window), WM_SNAP_DRAW, WPARAM(0), LPARAM(0)) };
	}
}

// Whether the overlay is taking input: open, and not fading out.
fn is_interactive() -> bool {
	with_shot(|shot| !shot.closing) == Some(true)
}

unsafe extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	match msg {
		WM_SNAP_KEY if wparam.0 == hook::RECORDED => {
			let chord = Chord::from_bits(lparam.0 as u32);
			change_settings(window, |settings| settings.record(chord));
			LRESULT(0)
		}
		WM_SNAP_KEY => {
			match with_shot(|shot| (shot.saving || shot.quitting, shot.closing)) {
				// the capture key does nothing while the Save As dialog is open or snap is quitting
				Some((true, _)) => {}
				// a fresh capture right away, skipping the rest of the fade-out
				Some((_, true)) => {
					hide(window);
					open_or_report(window, false);
				}
				// the capture key does whatever it's also bound to in the overlay (copying the whole screen by default),
				// and nothing while a dialog is up
				Some(_) => {
					let action = with_shot(|shot| {
						(shot.mode() == Mode::Box)
							.then(|| Action::for_chord(&shot.keys, shot.keys.capture, can_clear(shot)))
							.flatten()
					})
					.flatten();
					if let Some(action) = action {
						action.run(window);
					}
				}
				None => open_or_report(window, false),
			}
			LRESULT(0)
		}
		tray::WM_TRAY => {
			match (lparam.0 & 0xFFFF) as u32 {
				WM_LBUTTONUP if with_shot(|_| ()).is_none() => open_or_report(window, false),
				WM_RBUTTONUP => {
					let command = tray::show_menu(window);
					if command == tray::MENU_QUIT {
						quit(window);
					} else if command == tray::MENU_SETTINGS {
						settings_from_tray(window);
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
		WM_SNAP_DRAW => {
			DRAW_POSTED.set(false);
			render_or_report(window);
			after_frame(window);
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
				if let Some(settings) = shot.active_settings() {
					settings.pressed = settings.hit(p);
					return;
				}
				// no box over the live screen
				if shot.live {
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
				if let Some(settings) = &shot.settings
					&& !settings.is_closed()
				{
					let changed = settings.hit(shot.hover) != settings.hit(p);
					shot.hover = p;
					let target = shot.hints_target();
					shot.hints_fade.retarget(target, Instant::now());
					return changed;
				}
				if shot.live {
					shot.hover = p;
					return false;
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
				if let Some(settings) = shot.active_settings() {
					let pressed = settings.pressed.take();
					return pressed
						.filter(|&target| settings.hit(p) == Some(target))
						.map(Click::Setting);
				}
				if shot.live {
					return None;
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
				Some(Click::Button(Button::Undo)) => change(window, Selection::undo),
				Some(Click::Button(Button::Redo)) => change(window, Selection::redo),
				Some(Click::Button(Button::Clear)) => change(window, Selection::clear),
				// Shift+click quits snap, after asking
				Some(Click::Button(Button::Close)) if key_down(VK_SHIFT) => ask_to_quit(window),
				Some(Click::Button(Button::Close)) => close(window, None),
				Some(Click::Button(Button::Gear)) => open_settings(window),
				Some(Click::Choice(choice)) => answer(window, choice),
				Some(Click::Setting(target)) => change_settings(window, |settings| settings.click(target)),
				None => {}
			}
			redraw(window);
			keep_animating(window);
			LRESULT(0)
		}
		WM_MOUSEWHEEL if is_interactive() => {
			// the high word is how far it turned, signed
			let delta = (wparam.0 >> 16) as i16;
			let scrolled = with_shot(|shot| shot.active_settings().is_some_and(|settings| settings.scroll(delta)));
			if scrolled == Some(true) {
				redraw(window);
			}
			LRESULT(0)
		}
		// the mouse was taken away mid-drag (for example by a system dialog), so end the drag where it was
		WM_CAPTURECHANGED => {
			with_shot(|shot| {
				shot.pressed = None;
				if let Some(confirm) = shot.active_confirm() {
					confirm.pressed = None;
				}
				if let Some(settings) = &mut shot.settings {
					settings.pressed = None;
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
		// Alt+F4. "quit" leaves it to Windows, which closes the window and so quits snap without asking (a newer copy
		// closing this one sends WM_CLOSE straight away, so it's unaffected).
		WM_SYSCOMMAND if (wparam.0 & 0xFFF0) as u32 == SC_CLOSE => {
			match with_shot(|shot| shot.keys.alt_f4) {
				Some(AltF4::Disabled) => {}
				Some(AltF4::Quit) | None => return unsafe { DefWindowProcW(window, msg, wparam, lparam) },
			}
			LRESULT(0)
		}
		// Quit from the tray, or a newer copy closed this one (WM_CLOSE destroys the window by default)
		WM_DESTROY => {
			tray::remove(window);
			// released here, while the window still exists: left to the thread-locals' destructors at process exit,
			// releasing the Direct2D objects made the process exit with code 2170 (0x87A, the DXGI facility). Quitting
			// from the confirmation happens with the overlay open, so the shot's objects (the hints, buttons and
			// confirmation) go first, then the factories they came from.
			stop_pacing();
			SHOT.with(|cell| cell.try_borrow_mut().map(|mut shot| shot.take()).ok());
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
			// the confirmation takes only its answers (and Alt+F4, which goes on to Windows as `WM_SYSCOMMAND`)
			if let Some(keys) = with_shot(|shot| shot.is_confirming().then_some(shot.keys)).flatten() {
				if chord == keys.confirm {
					answer(window, Choice::Primary);
				} else if chord == keys.cancel {
					answer(window, Choice::Secondary);
				} else if msg == WM_SYSKEYDOWN && key == VK_F4.0 {
					return unsafe { DefWindowProcW(window, msg, wparam, lparam) };
				}
				return LRESULT(0);
			}
			// the settings menu closes on Esc or its own key; a keybind waiting for a key gets it from the hook instead
			if let Some((waiting, keys)) = with_shot(|shot| {
				let waiting = shot.settings.as_ref().is_some_and(Settings::is_waiting);
				(shot.mode() == Mode::Settings).then_some((waiting, shot.keys))
			})
			.flatten()
			{
				if !waiting && (chord == Chord::plain(VK_ESCAPE.0) || chord == keys.settings) {
					close_settings(window);
				} else if msg == WM_SYSKEYDOWN && key == VK_F4.0 {
					return unsafe { DefWindowProcW(window, msg, wparam, lparam) };
				}
				return LRESULT(0);
			}
			update_modifiers(window);
			if let Some(action) = with_shot(|shot| Action::for_chord(&shot.keys, chord, can_clear(shot))).flatten() {
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
