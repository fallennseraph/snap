// hook

// Takes over the capture key with a low-level keyboard hook, so PrtScr works even when Windows would open the Snipping
// Tool.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL,
	VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
	CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
	WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::Result;

use crate::keys::Chord;

// The window that gets `message` when the capture key is pressed. The hook runs on the same thread as the window's
// message loop, but statics are the only way to hand it state.
static TARGET: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicIsize = AtomicIsize::new(0);
static CAPTURE: AtomicU32 = AtomicU32::new(0); // the capture chord, from `Chord::to_bits`
// The key of the capture press the hook swallowed (0 for none), so it swallows the matching release and ignores key
// repeat.
static HELD: AtomicU32 = AtomicU32::new(0);
// Whether the settings menu is waiting for a keybind, so every key press goes to it instead.
static RECORDING: AtomicBool = AtomicBool::new(false);

// `message`'s `wparam` for a key press taken while recording, with the chord's `Chord::to_bits` in `lparam`. It's 0 for
// the capture key.
pub const RECORDED: usize = 1;

// An unassigned virtual-key code, tapped to keep Windows from treating a lone Alt or Win release as a press of it.
const MASK_KEY: u16 = 0xE8;

// Posts `message` to `target` on every press of `capture`. The hook lasts until the process exits.
pub fn install(target: HWND, message: u32, capture: Chord) -> Result<HHOOK> {
	TARGET.store(target.0 as isize, Ordering::Relaxed);
	MESSAGE.store(message as isize, Ordering::Relaxed);
	set_capture(capture);
	unsafe {
		let instance = GetModuleHandleW(None)?;
		SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0)
	}
}

// Changes the capture chord. Safe from any thread.
pub fn set_capture(capture: Chord) {
	CAPTURE.store(capture.to_bits(), Ordering::Relaxed);
}

// Starts or stops sending every key press (with its modifiers, which pass through as usual) to the target as
// `RECORDED`, swallowing it, so the settings menu can take any chord as a keybind: PrtScr and Alt+F4 included.
pub fn record(on: bool) {
	RECORDING.store(on, Ordering::Relaxed);
}

fn is_modifier(key: u32) -> bool {
	// Shift, Ctrl and Alt, their left and right versions, and both Win keys
	matches!(key, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C) || key == MASK_KEY as u32
}

fn post(wparam: usize, lparam: isize) {
	let target = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
	let message = MESSAGE.load(Ordering::Relaxed) as u32;
	let _ = unsafe { PostMessageW(Some(target), message, WPARAM(wparam), LPARAM(lparam)) };
}

fn held_modifiers() -> [bool; 4] {
	let down = |key: VIRTUAL_KEY| unsafe { GetAsyncKeyState(key.0 as i32) } < 0;
	[
		down(VK_CONTROL),
		down(VK_SHIFT),
		down(VK_MENU),
		down(VK_LWIN) || down(VK_RWIN),
	]
}

// Taps `MASK_KEY`. Without it, swallowing the key of a Win chord lets the Win release open the Start menu, and an Alt
// chord's Alt release focus the menu bar of the app underneath.
fn mask_modifiers() {
	let input = |flags| INPUT {
		r#type: INPUT_KEYBOARD,
		Anonymous: INPUT_0 {
			ki: KEYBDINPUT {
				wVk: VIRTUAL_KEY(MASK_KEY),
				dwFlags: flags,
				..Default::default()
			},
		},
	};
	let inputs = [input(Default::default()), input(KEYEVENTF_KEYUP)];
	unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	if code == HC_ACTION as i32 {
		// SAFETY: for WH_KEYBOARD_LL with HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT
		let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
		let capture = Chord::from_bits(CAPTURE.load(Ordering::Relaxed));
		match wparam.0 as u32 {
			WM_KEYDOWN | WM_SYSKEYDOWN if RECORDING.load(Ordering::Relaxed) && !is_modifier(info.vkCode) => {
				if HELD.load(Ordering::Relaxed) == info.vkCode {
					return LRESULT(1);
				}
				HELD.store(info.vkCode, Ordering::Relaxed);
				let [ctrl, shift, alt, win] = held_modifiers();
				if alt || win {
					mask_modifiers();
				}
				let chord = Chord {
					ctrl,
					shift,
					alt,
					win,
					key: info.vkCode as u16,
				};
				post(RECORDED, chord.to_bits() as isize);
				return LRESULT(1);
			}
			// an unbound capture key has key code 0, which no press has
			WM_KEYDOWN | WM_SYSKEYDOWN if info.vkCode == capture.key as u32 => {
				// key repeat of a press already taken
				if HELD.load(Ordering::Relaxed) == info.vkCode {
					return LRESULT(1);
				}
				// other modifier combinations keep their usual behavior (with PrtScr, Alt+PrtScr and Win+PrtScr)
				if capture.modifiers_match(held_modifiers()) {
					HELD.store(info.vkCode, Ordering::Relaxed);
					if capture.alt || capture.win {
						mask_modifiers();
					}
					post(0, 0);
					return LRESULT(1);
				}
			}
			WM_KEYUP | WM_SYSKEYUP if HELD.load(Ordering::Relaxed) == info.vkCode => {
				HELD.store(0, Ordering::Relaxed);
				return LRESULT(1);
			}
			_ => {}
		}
	}
	unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
