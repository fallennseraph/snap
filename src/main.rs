// main

// snap: a lightweight screenshot tool. Runs in the background; the capture key (PrtScr by default) opens the overlay.

// release builds have no console window; debug builds keep one for error output
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod buttons;
mod capture;
mod clipboard;
mod config;
mod confirm;
mod gdi;
mod gpu;
mod hints;
mod hook;
mod keys;
mod motion;
mod overlay;
mod render;
mod save;
mod selection;
mod settings;
mod tray;

use windows::Win32::Foundation::{
	ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
	DispatchMessageW, FindWindowW, GetMessageW, MSG, PostMessageW, WM_CLOSE,
};
use windows::core::{PCWSTR, Result, w};

// How long a new copy waits for the running one to close: this many tries, `TAKE_OVER_WAIT_MS` apart.
const TAKE_OVER_TRIES: u32 = 50;
const TAKE_OVER_WAIT_MS: u32 = 100;

fn main() -> Result<()> {
	// One copy runs at a time, so only one hook takes the capture key. The newest copy wins, so starting an updated build
	// replaces the old one. Each copy owns the mutex until it exits.
	let instance = unsafe { CreateMutexW(None, true, w!("Local\\snap.instance"))? };
	if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS && !take_over(instance) {
		eprintln!("snap: the running copy didn't close, so this one is quitting");
		return Ok(());
	}

	unsafe {
		// physical pixels everywhere, so captures and box sizes aren't scaled
		SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
	}
	let window = overlay::create()?;
	config::init();
	let config = config::load();
	let _hook = hook::install(window, overlay::WM_SNAP_KEY, config.keys.capture)?;
	// the rest of the config is read when the overlay opens
	config::watch(|config| hook::set_capture(config.keys.capture));
	// PrtScr still works without the icon (for example before Explorer has started); it's added when the
	// taskbar appears
	if let Err(err) = tray::add(window) {
		eprintln!("snap: couldn't add the tray icon: {err}");
	}

	let mut msg = MSG::default();
	unsafe {
		// GetMessageW returns -1 on error and 0 on WM_QUIT
		while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
			DispatchMessageW(&msg);
		}
	}
	Ok(())
}

// Asks the running copy to close and waits until it has, taking over `mutex`. `false` if it doesn't close in time.
fn take_over(mutex: HANDLE) -> bool {
	for _ in 0..TAKE_OVER_TRIES {
		// asked on every try, since a copy that only just started may not have made its window yet
		if let Ok(window) = unsafe { FindWindowW(overlay::CLASS_NAME, PCWSTR::null()) } {
			let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
		}
		// the old copy exits without releasing the mutex, so it usually comes back abandoned
		let result = unsafe { WaitForSingleObject(mutex, TAKE_OVER_WAIT_MS) };
		if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
			return true;
		}
	}
	false
}
