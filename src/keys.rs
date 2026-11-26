// keys

// Key chords for the keybinds in `config.toml`, written like `Ctrl+C` or `PrintScreen`: any of the modifiers Ctrl,
// Shift, Alt and Win, then one key. Names are case-insensitive. A chord matches only with exactly its modifiers held.

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

// Each named key as its config names, virtual-key code and hint label. Letters, digits and F1 to F24 are handled
// separately. The arrows aren't here, since they nudge the box.
const NAMED: &[(&[&str], u16, &str)] = &[
	(&["printscreen", "prtsc", "prtscr"], 0x2C, "PrtSc"),
	(&["escape", "esc"], 0x1B, "Esc"),
	(&["enter", "return"], 0x0D, "Enter"),
	(&["space"], 0x20, "Space"),
	(&["tab"], 0x09, "Tab"),
	(&["backspace"], 0x08, "Backspace"),
	(&["delete", "del"], 0x2E, "Del"),
	(&["insert", "ins"], 0x2D, "Ins"),
	(&["home"], 0x24, "Home"),
	(&["end"], 0x23, "End"),
	(&["pageup", "pgup"], 0x21, "PgUp"),
	(&["pagedown", "pgdn"], 0x22, "PgDn"),
	(&["pause"], 0x13, "Pause"),
];

const F1: u16 = 0x70;

impl Chord {
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

	pub fn parse(text: &str) -> Result<Self, String> {
		let mut chord = Self::plain(0);
		let mut parts = text.split('+').map(str::trim).peekable();
		while let Some(part) = parts.next() {
			let name = part.to_ascii_lowercase();
			if parts.peek().is_none() {
				chord.key = key_code(&name).ok_or_else(|| format!("unknown key '{part}' in '{text}'"))?;
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

	// The key caps the hints show for it, the modifiers then the key, each with the virtual-key codes that press it.
	pub fn caps(&self) -> Vec<(String, Vec<u16>)> {
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
	if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
		&& (1..=24).contains(&number)
	{
		return Some(F1 + number - 1);
	}
	NAMED
		.iter()
		.find(|(names, _, _)| names.contains(&name))
		.map(|(_, code, _)| *code)
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
		assert!(Chord::parse("").is_err());
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
