// tray

// The notification area icon. Left-click opens the overlay; right-click opens a menu.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
	NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
	AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, IMAGE_ICON, LR_DEFAULTCOLOR,
	LoadImageW, MF_STRING, PostMessageW, RegisterWindowMessageW, SM_CXSMICON, SetForegroundWindow, TPM_BOTTOMALIGN,
	TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::core::{Error, PCWSTR, Result, w};

// Sent to the overlay window for clicks on the icon, with the mouse message in `lparam`.
pub const WM_TRAY: u32 = WM_APP + 2;
pub const MENU_QUIT: usize = 1;

const ICON_ID: usize = 1; // the icon's resource ID in snap.rc
const TRAY_ID: u32 = 1;
const TOOLTIP: &str = "snap";

// The message Explorer broadcasts when the taskbar is recreated, registered on first `add`.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

// Adds the icon. Also needed again whenever Explorer restarts, since a new taskbar has no icons.
pub fn add(window: HWND) -> Result<()> {
	unsafe {
		TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
		let instance = GetModuleHandleW(None)?;
		let size = GetSystemMetrics(SM_CXSMICON);
		// a resource ID passed as a pointer (MAKEINTRESOURCE). The icon is kept for the life of the process.
		let icon = LoadImageW(
			Some(instance.into()),
			PCWSTR(ICON_ID as *const u16),
			IMAGE_ICON,
			size,
			size,
			LR_DEFAULTCOLOR,
		)?;
		let mut data = icon_data(window);
		data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
		data.uCallbackMessage = WM_TRAY;
		data.hIcon = HICON(icon.0);
		for (slot, unit) in data.szTip.iter_mut().zip(TOOLTIP.encode_utf16()) {
			*slot = unit;
		}
		if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
			return Err(Error::new(E_FAIL, "Shell_NotifyIconW couldn't add the tray icon"));
		}
	}
	Ok(())
}

pub fn remove(window: HWND) {
	unsafe {
		let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(window));
	}
}

// Whether `msg` says the taskbar was recreated, so the icon needs adding again.
pub fn is_taskbar_created(msg: u32) -> bool {
	let id = TASKBAR_CREATED.load(Ordering::Relaxed);
	id != 0 && msg == id
}

// Shows the menu at the mouse and returns the chosen item, or 0 if none.
pub fn show_menu(window: HWND) -> usize {
	unsafe {
		let Ok(menu) = CreatePopupMenu() else { return 0 };
		let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
		let mut cursor = POINT::default();
		let _ = GetCursorPos(&mut cursor);
		// the menu only closes on a click elsewhere if its window is in the foreground, and the message posted after
		// it lets it open properly next time (both documented quirks of menus for tray icons)
		let _ = SetForegroundWindow(window);
		let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
		let command = TrackPopupMenu(menu, flags, cursor.x, cursor.y, None, window, None).0 as usize;
		let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
		let _ = DestroyMenu(menu);
		command
	}
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
	NOTIFYICONDATAW {
		cbSize: size_of::<NOTIFYICONDATAW>() as u32,
		hWnd: window,
		uID: TRAY_ID,
		..Default::default()
	}
}
