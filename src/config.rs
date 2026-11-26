// config

// Settings from `config.toml`, read each time the overlay opens so edits apply without a restart. The file lives in
// the repo for now; it'll move to AppData behind a settings window.

use std::thread;

use serde::Deserialize;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
	FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::core::HSTRING;

use crate::keys::Chord;

const DIRECTORY: &str = env!("CARGO_MANIFEST_DIR");
const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml");

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
	pub show_hints: bool,            // default: true
	pub hardware_acceleration: bool, // default: true; off draws with Windows' software renderer
	pub old_box_outline: bool,       // default: true; the box from before a move or resize, while dragging
	pub new_box_outline: bool,       // default: true; where the box is going while the drawn one lags behind
	pub animations: Animations,
	pub keys: Keys,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			show_hints: true,
			hardware_acceleration: true,
			old_box_outline: true,
			new_box_outline: true,
			animations: Animations::default(),
			keys: Keys::default(),
		}
	}
}

// One switch per effect, plus `enabled` for all of them. Use `active` to read them.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Animations {
	pub enabled: bool,                // default: true
	pub overlay_fade: bool,           // default: true
	pub modifier_spring: bool,        // default: true
	pub drag_spring: bool,            // default: true
	pub copy_flash: bool,             // default: true
	pub copy_flash_full_screen: bool, // default: false
}

impl Default for Animations {
	fn default() -> Self {
		Self {
			enabled: true,
			overlay_fade: true,
			modifier_spring: true,
			drag_spring: true,
			copy_flash: true,
			copy_flash_full_screen: false,
		}
	}
}

impl Animations {
	// The switches with `enabled` applied, so each one says whether its effect is on.
	pub fn active(self) -> Self {
		let on = |switch: bool| self.enabled && switch;
		Self {
			enabled: self.enabled,
			overlay_fade: on(self.overlay_fade),
			modifier_spring: on(self.modifier_spring),
			drag_spring: on(self.drag_spring),
			copy_flash: on(self.copy_flash),
			copy_flash_full_screen: on(self.copy_flash_full_screen),
		}
	}
}

// The keybinds. `capture` works anywhere; the rest only in the overlay.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Keys {
	pub capture: Chord, // default: PrintScreen; in the overlay it does whichever action shares its chord
	pub copy: Chord,    // default: Ctrl+C; the box, or the whole screen if there is no box
	pub copy_whole: Chord, // default: PrintScreen
	pub save: Chord,    // default: Ctrl+S
	pub close: Chord,   // default: Escape; clears the box first if there is one
}

impl Default for Keys {
	fn default() -> Self {
		const PRINT_SCREEN: u16 = 0x2C;
		const ESCAPE: u16 = 0x1B;
		Self {
			capture: Chord::plain(PRINT_SCREEN),
			copy: Chord::ctrl(b'C'),
			copy_whole: Chord::plain(PRINT_SCREEN),
			save: Chord::ctrl(b'S'),
			close: Chord::plain(ESCAPE),
		}
	}
}

// A missing file means the defaults. So does a broken one, with a message.
pub fn load() -> Config {
	let Ok(text) = std::fs::read_to_string(PATH) else {
		return Config::default();
	};
	toml::from_str(&text).unwrap_or_else(|err| {
		eprintln!("snap: ignoring config.toml: {err}");
		Config::default()
	})
}

// Calls `on_change` with the new config on a background thread whenever a file in the config's folder is written, for
// settings needed before the overlay opens (the capture key). Editors often write more than once per save, so it can
// come a few times in a row.
pub fn watch(on_change: impl Fn(Config) + Send + 'static) {
	thread::spawn(move || {
		let change = match unsafe {
			FindFirstChangeNotificationW(&HSTRING::from(DIRECTORY), false, FILE_NOTIFY_CHANGE_LAST_WRITE)
		} {
			Ok(change) => change,
			Err(err) => {
				eprintln!("snap: can't watch config.toml for changes: {err}");
				return;
			}
		};
		while unsafe { WaitForSingleObject(change, INFINITE) } == WAIT_OBJECT_0 {
			on_change(load());
			if unsafe { FindNextChangeNotification(change) }.is_err() {
				return;
			}
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn missing_keys_use_defaults() {
		let config: Config = toml::from_str("[animations]\ndrag_spring = false\n").unwrap();
		assert!(config.show_hints && config.hardware_acceleration);
		assert!(!config.animations.drag_spring && config.animations.overlay_fade);
	}

	#[test]
	fn repo_config_parses() {
		let text = std::fs::read_to_string(PATH).unwrap();
		toml::from_str::<Config>(&text).unwrap();
	}

	#[test]
	fn reads_keys() {
		let config: Config = toml::from_str("[keys]\ncopy = \"Alt+X\"\n").unwrap();
		assert!(config.keys.copy.alt && config.keys.copy.key == u16::from(b'X'));
		assert_eq!(config.keys.save, Chord::ctrl(b'S'));
		assert!(toml::from_str::<Config>("[keys]\ncopy = \"Ctrl+Nope\"\n").is_err());
	}

	#[test]
	fn enabled_turns_everything_off() {
		let config: Config = toml::from_str("[animations]\nenabled = false\ncopy_flash_full_screen = true\n").unwrap();
		let active = config.animations.active();
		assert!(!active.overlay_fade && !active.copy_flash && !active.copy_flash_full_screen);
	}
}
