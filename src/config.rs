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
	pub hardware_acceleration: bool, // default: true; off draws with Windows' software renderer
	pub ui: Ui,
	pub animations: Animations,
	pub keys: Keys,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			hardware_acceleration: true,
			ui: Ui::default(),
			animations: Animations::default(),
			keys: Keys::default(),
		}
	}
}

// What the overlay shows.
#[derive(Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Ui {
	pub hints: bool,           // default: true
	pub close_button: bool,    // default: true; the x in the screen's top-right corner
	pub old_box_outline: bool, // default: true; the box from before a move or resize, while dragging
	pub new_box_outline: bool, // default: true; where the box is going while the drawn one lags behind
}

impl Default for Ui {
	fn default() -> Self {
		Self {
			hints: true,
			close_button: true,
			old_box_outline: true,
			new_box_outline: true,
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
	pub quit: Chord,    // default: Shift+Escape; quits snap, after asking
	pub confirm: Chord, // default: Enter; the primary button of a confirmation, like Quit
	pub cancel: Chord,  // default: Escape; its secondary button
	pub alt_f4: AltF4,  // default: ask
}

// What Alt+F4 does in the overlay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AltF4 {
	Disabled, // nothing
	Ask,      // the same as `quit`
	Quit,     // Windows' own: quits snap without asking
}

impl Keys {
	// Pairs of actions bound to the same chord where they'd fight over it: two actions of the box (`cancel` clears it),
	// or a confirmation's two answers (`quit` counts as confirming there). `cancel` and `close` can share, since
	// clearing the box goes first; `confirm` and the box actions never apply at once; `capture` shares on purpose.
	pub fn conflicts(&self) -> Vec<(&'static str, &'static str)> {
		let in_box = [
			("copy", self.copy),
			("copy_whole", self.copy_whole),
			("save", self.save),
			("close", self.close),
			("quit", self.quit),
			("cancel", self.cancel),
		];
		let mut conflicts = Vec::new();
		for (i, (a, chord)) in in_box.iter().enumerate() {
			for (b, other) in &in_box[i + 1..] {
				// `cancel` clears the box before `close` closes it, so the two can share
				if chord == other && (*a, *b) != ("close", "cancel") {
					conflicts.push((*a, *b));
				}
			}
		}
		if self.confirm == self.cancel {
			conflicts.push(("confirm", "cancel"));
		}
		conflicts
	}
}

impl Default for Keys {
	fn default() -> Self {
		const PRINT_SCREEN: u16 = 0x2C;
		const ESCAPE: u16 = 0x1B;
		const ENTER: u16 = 0x0D;
		Self {
			capture: Chord::plain(PRINT_SCREEN),
			copy: Chord::ctrl(b'C'),
			copy_whole: Chord::plain(PRINT_SCREEN),
			save: Chord::ctrl(b'S'),
			close: Chord::plain(ESCAPE),
			quit: Chord {
				shift: true,
				..Chord::plain(ESCAPE)
			},
			confirm: Chord::plain(ENTER),
			cancel: Chord::plain(ESCAPE),
			alt_f4: AltF4::Ask,
		}
	}
}

// A missing file means the defaults. So does a broken one, with a message.
pub fn load() -> Config {
	let Ok(text) = std::fs::read_to_string(PATH) else {
		return Config::default();
	};
	let config: Config = toml::from_str(&text).unwrap_or_else(|err| {
		eprintln!("snap: ignoring config.toml: {err}");
		Config::default()
	});
	// they still load, with the earlier action in `[keys]` winning
	for (a, b) in config.keys.conflicts() {
		eprintln!("snap: '{a}' and '{b}' share a keybind, so only '{a}' works");
	}
	config
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
		assert!(config.ui.hints && config.ui.close_button && config.hardware_acceleration);
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
		assert_eq!(config.keys.quit, Chord::parse("Shift+Escape").unwrap());
		assert_eq!(config.keys.alt_f4, AltF4::Ask);
		let config: Config = toml::from_str(
			"[keys]
alt_f4 = \"disabled\"
",
		)
		.unwrap();
		assert_eq!(config.keys.alt_f4, AltF4::Disabled);
		assert!(
			toml::from_str::<Config>(
				"[keys]
alt_f4 = \"nope\"
"
			)
			.is_err()
		);
		assert!(toml::from_str::<Config>("[keys]\ncopy = \"Ctrl+Nope\"\n").is_err());
	}

	#[test]
	fn finds_conflicting_keys() {
		assert!(Keys::default().conflicts().is_empty());
		let config: Config = toml::from_str(
			"[keys]
save = \"Ctrl+C\"
confirm = \"Escape\"
",
		)
		.unwrap();
		assert_eq!(config.keys.conflicts(), [("copy", "save"), ("confirm", "cancel")]);
		// `quit` confirms in a confirmation, so sharing with `confirm` is fine, but not with `cancel`
		let config: Config = toml::from_str(
			"[keys]
quit = \"Enter\"
",
		)
		.unwrap();
		assert!(config.keys.conflicts().is_empty());
		let config: Config = toml::from_str(
			"[keys]
quit = \"Escape\"
",
		)
		.unwrap();
		assert_eq!(config.keys.conflicts(), [("close", "quit"), ("quit", "cancel")]);
		// `cancel` clears the box, so it can't share with the box actions either
		let config: Config = toml::from_str(
			"[keys]
cancel = \"Ctrl+S\"
",
		)
		.unwrap();
		assert_eq!(config.keys.conflicts(), [("save", "cancel")]);
	}

	#[test]
	fn enabled_turns_everything_off() {
		let config: Config = toml::from_str("[animations]\nenabled = false\ncopy_flash_full_screen = true\n").unwrap();
		let active = config.animations.active();
		assert!(!active.overlay_fade && !active.copy_flash && !active.copy_flash_full_screen);
	}
}
