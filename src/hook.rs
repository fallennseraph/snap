// hook

// Takes over PrtScr with a low-level keyboard hook, so it works even when Windows would open the Snipping Tool.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
	GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::WindowsAndMessaging::{
	CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
	WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::Result;

// The window that gets `message` when PrtScr is pressed. The hook runs on the same thread as the window's message
// loop, but statics are the only way to hand it state.
static TARGET: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicIsize = AtomicIsize::new(0);
// Whether the hook swallowed the current PrtScr press, so it swallows the matching release and ignores key repeat.
static HELD: AtomicBool = AtomicBool::new(false);

// Posts `message` to `target` on every PrtScr press. The hook lasts until the process exits.
pub fn install(target: HWND, message: u32) -> Result<HHOOK> {
	TARGET.store(target.0 as isize, Ordering::Relaxed);
	MESSAGE.store(message as isize, Ordering::Relaxed);
	unsafe {
		let instance = GetModuleHandleW(None)?;
		SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance.into()), 0)
	}
}

// Only plain PrtScr is taken, so Alt+PrtScr and Win+PrtScr keep their Windows behavior.
fn modifier_held() -> bool {
	const MODIFIERS: [VIRTUAL_KEY; 5] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
	MODIFIERS
		.iter()
		.any(|key| unsafe { GetAsyncKeyState(key.0 as i32) } < 0)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	if code == HC_ACTION as i32 {
		// SAFETY: for WH_KEYBOARD_LL with HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT
		let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
		if info.vkCode == VK_SNAPSHOT.0 as u32 {
			match wparam.0 as u32 {
				WM_KEYDOWN | WM_SYSKEYDOWN => {
					let repeat = HELD.load(Ordering::Relaxed);
					if repeat || !modifier_held() {
						if !repeat {
							HELD.store(true, Ordering::Relaxed);
							let target = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
							let message = MESSAGE.load(Ordering::Relaxed) as u32;
							let _ = unsafe { PostMessageW(Some(target), message, WPARAM(0), LPARAM(0)) };
						}
						return LRESULT(1);
					}
				}
				WM_KEYUP | WM_SYSKEYUP if HELD.swap(false, Ordering::Relaxed) => {
					return LRESULT(1);
				}
				_ => {}
			}
		}
	}
	unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
