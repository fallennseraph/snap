// keys

// Key chords for the keybinds in `config.toml`, written like `Ctrl+C` or `PrintScreen`: any of the modifiers Ctrl,
// Shift, Alt and Win, then one key. Names are case-insensitive. A chord matches only with exactly its modifiers held.
// An empty string is `Chord::NONE`, an unbound action, which no key press matches.

use std::fmt;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Chord {
	pub ctrl: bool,
	pub shift: bool,
	pub alt: bool,
	pub win: bool,
	pub key: u16, // the virtual-key code
}

// Each named key as its config names (the first is the one snap writes), virtual-key code and hint label. Letters,
// digits and F1 to F24 are handled separately. The arrows aren't here, since they nudge the box.
const NAMED: &[(&[&str], u16, &str)] = &[
	(&["PrintScreen", "PrtSc", "PrtScr"], 0x2C, "PrtSc"),
	(&["Escape", "Esc"], 0x1B, "Esc"),
	(&["Enter", "Return"], 0x0D, "Enter"),
	(&["Space"], 0x20, "Space"),
	(&["Tab"], 0x09, "Tab"),
	(&["Backspace"], 0x08, "Backspace"),
	(&["Delete", "Del"], 0x2E, "Del"),
	(&["Insert", "Ins"], 0x2D, "Ins"),
	(&["Home"], 0x24, "Home"),
	(&["End"], 0x23, "End"),
	(&["PageUp", "PgUp"], 0x21, "PgUp"),
	(&["PageDown", "PgDn"], 0x22, "PgDn"),
	(&["Pause"], 0x13, "Pause"),
	(&["Comma", ","], 0xBC, ","),
];

const F1: u16 = 0x70;

impl Chord {
	pub const NONE: Self = Self::plain(0);

	pub const fn plain(key: u16) -> Self {
		Self {
			ctrl: false,
			shift: false,
			alt: false,
			win: false,
			key,
		}
	}

	pub const fn ctrl(key: u8) -> Self {
		Self {
			ctrl: true,
			..Self::plain(key as u16)
		}
	}

	pub fn is_none(&self) -> bool {
		self.key == 0
	}

	// Whether its key is one `config.toml` has a name for, so a keybind can use it (not a lone modifier or an arrow).
	pub fn is_bindable(&self) -> bool {
		key_name(self.key).is_some()
	}

	pub fn parse(text: &str) -> Result<Self, String> {
		if text.trim().is_empty() {
			return Ok(Self::NONE);
		}
		let mut chord = Self::NONE;
		let mut parts = text.split('+').map(str::trim).peekable();
		while let Some(part) = parts.next() {
			let name = part.to_ascii_lowercase();
			if parts.peek().is_none() {
				chord.key = key_code(part).ok_or_else(|| format!("unknown key '{part}' in '{text}'"))?;
				break;
			}
			let modifier = match name.as_str() {
				"ctrl" | "control" => &mut chord.ctrl,
				"shift" => &mut chord.shift,
				"alt" => &mut chord.alt,
				"win" | "windows" => &mut chord.win,
				_ => {
					return Err(format!(
						"'{part}' in '{text}' isn't a modifier (Ctrl, Shift, Alt or Win)"
					));
				}
			};
			*modifier = true;
		}
		Ok(chord)
	}

	// Whether exactly this chord's modifiers are held, asking `held` for each of Ctrl, Shift, Alt and Win.
	pub fn modifiers_match(&self, held: [bool; 4]) -> bool {
		[self.ctrl, self.shift, self.alt, self.win] == held
	}

	// The key caps the hints show for it, the modifiers then the key, each with the virtual-key codes that press it. An
	// unbound chord is one cap, "None", that nothing presses.
	pub fn caps(&self) -> Vec<(String, Vec<u16>)> {
		if self.is_none() {
			return vec![("None".to_string(), Vec::new())];
		}
		let modifiers: [(bool, &str, &[u16]); 4] = [
			(self.ctrl, "Ctrl", &[0x11]),
			(self.shift, "Shift", &[0x10]),
			(self.alt, "Alt", &[0x12]),
			(self.win, "Win", &[0x5B, 0x5C]),
		];
		let mut caps: Vec<(String, Vec<u16>)> = modifiers
			.iter()
			.filter(|(held, _, _)| *held)
			.map(|(_, label, codes)| (label.to_string(), codes.to_vec()))
			.collect();
		caps.push((key_label(self.key), vec![self.key]));
		caps
	}

	// Packed into one number, so the keyboard hook can read it from an atomic.
	pub fn to_bits(self) -> u32 {
		self.key as u32
			| (self.ctrl as u32) << 16
			| (self.shift as u32) << 17
			| (self.alt as u32) << 18
			| (self.win as u32) << 19
	}

	pub fn from_bits(bits: u32) -> Self {
		Self {
			ctrl: bits & 1 << 16 != 0,
			shift: bits & 1 << 17 != 0,
			alt: bits & 1 << 18 != 0,
			win: bits & 1 << 19 != 0,
			key: bits as u16,
		}
	}
}

// How `config.toml` writes it, like `Ctrl+Shift+C`, or an empty string when unbound.
impl fmt::Display for Chord {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let Some(key) = key_name(self.key) else {
			return Ok(());
		};
		for (held, name) in [
			(self.ctrl, "Ctrl"),
			(self.shift, "Shift"),
			(self.alt, "Alt"),
			(self.win, "Win"),
		] {
			if held {
				write!(f, "{name}+")?;
			}
		}
		f.write_str(&key)
	}
}

impl TryFrom<String> for Chord {
	type Error = String;

	fn try_from(text: String) -> Result<Self, String> {
		Self::parse(&text)
	}
}

fn key_code(name: &str) -> Option<u16> {
	let bytes = name.as_bytes();
	if let [byte] = bytes
		&& byte.is_ascii_alphanumeric()
	{
		return Some(byte.to_ascii_uppercase() as u16);
	}
	if let Some(number) = name.strip_prefix(['f', 'F']).and_then(|n| n.parse::<u16>().ok())
		&& (1..=24).contains(&number)
	{
		return Some(F1 + number - 1);
	}
	NAMED
		.iter()
		.find(|(names, _, _)| names.iter().any(|named| named.eq_ignore_ascii_case(name)))
		.map(|(_, code, _)| *code)
}

// The name `config.toml` uses for a key, if it has one.
fn key_name(code: u16) -> Option<String> {
	match code {
		0x30..=0x39 | 0x41..=0x5A => Some((code as u8 as char).to_string()),
		_ if (F1..F1 + 24).contains(&code) => Some(format!("F{}", code - F1 + 1)),
		_ => NAMED
			.iter()
			.find(|(_, named, _)| *named == code)
			.map(|(names, _, _)| names[0].to_string()),
	}
}

fn key_label(code: u16) -> String {
	match code {
		0x30..=0x39 | 0x41..=0x5A => (code as u8 as char).to_string(),
		_ if (F1..F1 + 24).contains(&code) => format!("F{}", code - F1 + 1),
		_ => NAMED
			.iter()
			.find(|(_, named, _)| *named == code)
			.map_or_else(|| format!("{code:#04X}"), |(_, _, label)| label.to_string()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_chords() {
		assert_eq!(Chord::parse("Ctrl+C").unwrap(), Chord::ctrl(b'C'));
		assert_eq!(Chord::parse("printscreen").unwrap(), Chord::plain(0x2C));
		let chord = Chord::parse("ctrl + Shift+F12").unwrap();
		assert!(chord.ctrl && chord.shift && !chord.alt && chord.key == 0x7B);
	}

	#[test]
	fn rejects_bad_chords() {
		assert!(Chord::parse("Ctrl+Nope").is_err());
		assert!(Chord::parse("C+Ctrl").is_err());
		assert!(Chord::parse("Ctrl").is_err());
		assert!(Chord::parse("F25").is_err());
		assert!(Chord::parse("Ctrl+").is_err());
	}

	#[test]
	fn empty_is_unbound() {
		assert_eq!(Chord::parse("").unwrap(), Chord::NONE);
		assert!(Chord::NONE.is_none() && !Chord::ctrl(b'C').is_none());
		assert_eq!(Chord::NONE.to_string(), "");
		assert_eq!(Chord::NONE.caps(), [("None".to_string(), vec![])]);
	}

	#[test]
	fn writes_config_names() {
		for text in [
			"Ctrl+Shift+C",
			"PrintScreen",
			"Shift+Escape",
			"Alt+F4",
			"Ctrl+Comma",
			"Win+PageDown",
			"7",
		] {
			assert_eq!(Chord::parse(text).unwrap().to_string(), text);
		}
		assert_eq!(Chord::parse("ctrl+prtsc").unwrap().to_string(), "Ctrl+PrintScreen");
		assert_eq!(Chord::parse("Ctrl+,").unwrap(), Chord::ctrl(0xBC));
		assert!(Chord::ctrl(0xBC).is_bindable());
		// modifiers and arrows on their own aren't keys a keybind can use
		assert!(!Chord::ctrl(0x11).is_bindable() && !Chord::plain(0x25).is_bindable() && !Chord::NONE.is_bindable());
	}

	#[test]
	fn labels_and_bits_round_trip() {
		let chord = Chord::parse("Win+Alt+PgDn").unwrap();
		let labels: Vec<String> = chord.caps().into_iter().map(|(label, _)| label).collect();
		assert_eq!(labels, ["Alt", "Win", "PgDn"]);
		assert_eq!(chord.caps()[1].1, [0x5B, 0x5C]);
		assert_eq!(Chord::from_bits(chord.to_bits()), chord);
		assert_eq!(Chord::parse("F3").unwrap().caps(), [("F3".to_string(), vec![0x72])]);
	}
}
