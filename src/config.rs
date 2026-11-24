// config

// Settings from `config.toml`, read each time the overlay opens so edits apply without a restart. The file lives in
// the repo for now; it'll move to AppData behind a settings window.

use serde::Deserialize;

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/config.toml");

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
	pub show_hints: bool,            // default: true
	pub hardware_acceleration: bool, // default: true; off draws with Windows' software renderer
	pub old_box_outline: bool,       // default: true; the box from before a move or resize, while dragging
	pub new_box_outline: bool,       // default: true; where the box is going while the drawn one lags behind
	pub animations: Animations,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			show_hints: true,
			hardware_acceleration: true,
			old_box_outline: true,
			new_box_outline: true,
			animations: Animations::default(),
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
	fn enabled_turns_everything_off() {
		let config: Config = toml::from_str("[animations]\nenabled = false\ncopy_flash_full_screen = true\n").unwrap();
		let active = config.animations.active();
		assert!(!active.overlay_fade && !active.copy_flash && !active.copy_flash_full_screen);
	}
}
