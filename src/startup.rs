// startup

// Starting snap with Windows, through the current user's `Run` key.

use std::io::{Error, Result};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
use windows::core::w;

// Adds snap to the programs Windows starts at sign-in, or removes it. Turning it on registers the running exe's path,
// so the last exe started with it on is the one Windows starts.
pub fn set(on: bool) -> Result<()> {
	let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
	let result = if on {
		// quoted, since the path can have spaces
		let command: Vec<u16> = format!("\"{}\"", std::env::current_exe()?.display())
			.encode_utf16()
			.chain([0])
			.collect();
		unsafe {
			RegSetKeyValueW(
				HKEY_CURRENT_USER,
				key,
				w!("snap"),
				REG_SZ.0,
				Some(command.as_ptr().cast()),
				(command.len() * size_of::<u16>()) as u32,
			)
		}
	} else {
		match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key, w!("snap")) } {
			// already off
			ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
			result => result,
		}
	};
	if result == ERROR_SUCCESS {
		Ok(())
	} else {
		Err(Error::from_raw_os_error(result.0 as i32))
	}
}
