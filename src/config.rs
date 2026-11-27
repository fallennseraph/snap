// config

// Settings from `config.toml` in `%APPDATA%\snap`, read each time the overlay opens so edits apply without a restart.
// The first launch writes it from `default.toml`: the defaults, with a comment on each setting.

use std::path::PathBuf;
use std::thread;

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, Value};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
	FILE_NOTIFY_CHANGE_LAST_WRITE, FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::core::HSTRING;

use crate::keys::Chord;

const DEFAULT: &str = include_str!("default.toml");

#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
	pub hardware_acceleration: bool, // default: true; off draws with Windows' software renderer
	pub start_with_windows: bool,    // default: false
	pub capture_cursor: bool,        // default: false; draws the mouse cursor into screenshots
	pub ui: Ui,
	pub animations: Animations,
	pub keys: Keys,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			hardware_acceleration: true,
			start_with_windows: false,
			capture_cursor: false,
			ui: Ui::default(),
			animations: Animations::default(),
			keys: Keys::default(),
		}
	}
}

// What the overlay shows.
#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
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
#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
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
#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
#[serde(default)]
pub struct Keys {
	pub capture: Chord, // default: PrintScreen; in the overlay it does whichever action shares its chord
	pub copy: Chord,    // default: Ctrl+C; the box, or the whole screen if there is no box
	pub copy_whole: Chord, // default: PrintScreen
	pub save: Chord,    // default: Ctrl+S
	pub close: Chord,   // default: Escape; clears the box first if there is one
	pub undo: Chord,    // default: Ctrl+Z; the last change to the box
	pub redo: Chord,    // default: Ctrl+Y
	pub confirm: Chord, // default: Enter; the primary button of a confirmation, like Quit
	pub cancel: Chord,  // default: Escape; its secondary button
	pub settings: Chord, // default: Ctrl+Comma
	pub alt_f4: AltF4,  // default: quit
}

// What Alt+F4 does in the overlay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AltF4 {
	Disabled, // nothing
	Quit,     // Windows' own: quits snap without asking
}

impl AltF4 {
	// How `config.toml` writes it.
	pub fn name(self) -> &'static str {
		match self {
			Self::Disabled => "disabled",
			Self::Quit => "quit",
		}
	}
}

impl Keys {
	// Pairs of actions bound to the same chord where they'd fight over it: two actions of the box (`cancel` clears it),
	// or a confirmation's two answers. `cancel` and `close` can share, since clearing the box goes first; `confirm` and
	// the box actions never apply at once; `capture` shares on purpose.
	// Unbound actions share nothing.
	pub fn conflicts(&self) -> Vec<(&'static str, &'static str)> {
		let in_box = [
			("copy", self.copy),
			("copy_whole", self.copy_whole),
			("save", self.save),
			("close", self.close),
			("undo", self.undo),
			("redo", self.redo),
			("cancel", self.cancel),
			("settings", self.settings),
		];
		let mut conflicts = Vec::new();
		for (i, (a, chord)) in in_box.iter().enumerate() {
			for (b, other) in &in_box[i + 1..] {
				// `cancel` clears the box before `close` closes it, so the two can share
				if chord == other && !chord.is_none() && (*a, *b) != ("close", "cancel") {
					conflicts.push((*a, *b));
				}
			}
		}
		if self.confirm == self.cancel && !self.confirm.is_none() {
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
		const COMMA: u8 = 0xBC;
		Self {
			capture: Chord::plain(PRINT_SCREEN),
			copy: Chord::ctrl(b'C'),
			copy_whole: Chord::plain(PRINT_SCREEN),
			save: Chord::ctrl(b'S'),
			close: Chord::plain(ESCAPE),
			undo: Chord::ctrl(b'Z'),
			redo: Chord::ctrl(b'Y'),
			confirm: Chord::plain(ENTER),
			cancel: Chord::plain(ESCAPE),
			settings: Chord::ctrl(COMMA),
			alt_f4: AltF4::Quit,
		}
	}
}

// `%APPDATA%\snap`, which holds `config.toml`.
fn directory() -> PathBuf {
	std::env::var_os("APPDATA")
		.map(PathBuf::from)
		.unwrap_or_default()
		.join("snap")
}

fn path() -> PathBuf {
	directory().join("config.toml")
}

// Writes `config.toml` from `default.toml` if there isn't one yet, making its folder too.
pub fn init() {
	let path = path();
	if path.exists() {
		return;
	}
	if let Err(err) = std::fs::create_dir_all(directory()).and_then(|()| std::fs::write(&path, DEFAULT)) {
		eprintln!("snap: couldn't write {}: {err}", path.display());
	}
}

// A missing file means the defaults. So does a broken one, with a message.
pub fn load() -> Config {
	let Ok(text) = std::fs::read_to_string(path()) else {
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

// Writes `config` to `config.toml`, keeping the file's comments and layout. Each value keeps the comment after it, and
// settings the file doesn't have yet go at the end of their section. A missing or broken file starts over from
// `default.toml`.
pub fn save(config: &Config) -> std::io::Result<()> {
	let path = path();
	let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT.to_string());
	std::fs::create_dir_all(directory())?;
	std::fs::write(path, update(&text, config))
}

fn update(text: &str, config: &Config) -> String {
	let mut document: DocumentMut = text.parse().unwrap_or_else(|err| {
		eprintln!("snap: replacing config.toml, which doesn't parse: {err}");
		DEFAULT.parse().expect("default.toml parses")
	});
	let root = document.as_table_mut();
	set(root, "hardware_acceleration", config.hardware_acceleration.into());
	set(root, "start_with_windows", config.start_with_windows.into());
	set(root, "capture_cursor", config.capture_cursor.into());

	let Ui {
		hints,
		close_button,
		old_box_outline,
		new_box_outline,
	} = config.ui;
	let ui = section(root, "ui");
	set(ui, "hints", hints.into());
	set(ui, "close_button", close_button.into());
	set(ui, "old_box_outline", old_box_outline.into());
	set(ui, "new_box_outline", new_box_outline.into());

	let keys = &config.keys;
	let chords = [
		("capture", keys.capture),
		("copy", keys.copy),
		("copy_whole", keys.copy_whole),
		("save", keys.save),
		("close", keys.close),
		("undo", keys.undo),
		("redo", keys.redo),
		("confirm", keys.confirm),
		("cancel", keys.cancel),
		("settings", keys.settings),
	];
	let table = section(root, "keys");
	for (name, chord) in chords {
		set(table, name, chord.to_string().into());
	}
	set(table, "alt_f4", keys.alt_f4.name().into());

	let Animations {
		enabled,
		overlay_fade,
		modifier_spring,
		drag_spring,
		copy_flash,
		copy_flash_full_screen,
	} = config.animations;
	let animations = section(root, "animations");
	set(animations, "enabled", enabled.into());
	set(animations, "overlay_fade", overlay_fade.into());
	set(animations, "modifier_spring", modifier_spring.into());
	set(animations, "drag_spring", drag_spring.into());
	set(animations, "copy_flash", copy_flash.into());
	set(animations, "copy_flash_full_screen", copy_flash_full_screen.into());

	document.to_string()
}

// The `[name]` table, made (or remade, if it's something else) when the file doesn't have it.
fn section<'a>(root: &'a mut Table, name: &str) -> &'a mut Table {
	if !root.get(name).is_some_and(Item::is_table) {
		root.insert(name, toml_edit::table());
	}
	root[name].as_table_mut().expect("just made it a table")
}

// Sets `key` to `value`, keeping the spacing and comment around the old value.
fn set(table: &mut Table, key: &str, value: Value) {
	match table.get_mut(key).and_then(Item::as_value_mut) {
		Some(old) => {
			let decor = old.decor().clone();
			*old = value;
			*old.decor_mut() = decor;
		}
		None => {
			table.insert(key, toml_edit::value(value));
		}
	}
}

// Calls `on_change` with the new config on a background thread whenever a file in the config's folder is written, for
// settings needed before the overlay opens (the capture key). Editors often write more than once per save, so it can
// come a few times in a row.
pub fn watch(on_change: impl Fn(Config) + Send + 'static) {
	thread::spawn(move || {
		let change = match unsafe {
			FindFirstChangeNotificationW(
				&HSTRING::from(directory().as_path()),
				false,
				FILE_NOTIFY_CHANGE_LAST_WRITE,
			)
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
	fn default_file_is_the_defaults() {
		assert_eq!(toml::from_str::<Config>(DEFAULT).unwrap(), Config::default());
		// and has every setting, since saving would add a missing one at the end of its section
		assert_eq!(update(DEFAULT, &Config::default()), DEFAULT);
	}

	#[test]
	fn reads_keys() {
		let config: Config = toml::from_str("[keys]\ncopy = \"Alt+X\"\n").unwrap();
		assert!(config.keys.copy.alt && config.keys.copy.key == u16::from(b'X'));
		assert_eq!(config.keys.save, Chord::ctrl(b'S'));
		assert_eq!(config.keys.alt_f4, AltF4::Quit);
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
	fn unbound_keys_share_nothing() {
		let config: Config = toml::from_str(
			"[keys]
copy = \"\"
save = \"\"
confirm = \"\"
cancel = \"\"
",
		)
		.unwrap();
		assert!(config.keys.copy.is_none() && config.keys.conflicts().is_empty());
	}

	#[test]
	fn saving_keeps_the_file() {
		let text = DEFAULT.to_string();
		let config: Config = toml::from_str(&text).unwrap();
		assert_eq!(update(&text, &config), text);

		let mut changed = config;
		changed.ui.hints = !config.ui.hints;
		changed.keys.copy = Chord::NONE;
		changed.keys.alt_f4 = AltF4::Disabled;
		let written = update(&text, &changed);
		let line = |text: &str, key: &str| text.lines().find(|line| line.starts_with(key)).unwrap().to_string();
		assert_eq!(
			line(&written, "hints ="),
			line(&text, "hints =").replacen(&config.ui.hints.to_string(), &changed.ui.hints.to_string(), 1)
		);
		assert!(line(&written, "copy =").starts_with("copy = \"\" ") && line(&written, "copy =").contains('#'));
		let read: Config = toml::from_str(&written).unwrap();
		assert!(read.keys.copy.is_none() && read.keys.alt_f4 == AltF4::Disabled);
		assert_eq!(read.ui.hints, changed.ui.hints);
	}

	#[test]
	fn saving_fills_in_a_missing_file() {
		let mut config = Config::default();
		config.keys.save = Chord::parse("Ctrl+Shift+S").unwrap();
		let read: Config = toml::from_str(&update("", &config)).unwrap();
		assert_eq!(read.keys.save, config.keys.save);
		assert_eq!(read.keys.settings, Chord::ctrl(0xBC));
	}

	#[test]
	fn enabled_turns_everything_off() {
		let config: Config = toml::from_str("[animations]\nenabled = false\ncopy_flash_full_screen = true\n").unwrap();
		let active = config.animations.active();
		assert!(!active.overlay_fade && !active.copy_flash && !active.copy_flash_full_screen);
	}
}
