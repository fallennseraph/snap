// save

// Saving an image as a PNG file picked with a Save As dialog.

use std::ffi::OsString;
use std::fs::File;
use std::io::BufWriter;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Controls::Dialogs::{
	GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::core::{PWSTR, w};

use crate::capture::Image;

// Room for the path the dialog returns, in UTF-16 units; well past MAX_PATH so long paths fit.
const PATH_CAPACITY: usize = 4096;

// Shows a Save As dialog owned by `owner`, starting with a timestamped name. `None` if the user cancels.
pub fn ask_path(owner: HWND) -> Option<PathBuf> {
	let now = unsafe { GetLocalTime() };
	let name = format!(
		"snap {:04}-{:02}-{:02} {:02}-{:02}-{:02}.png",
		now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
	);
	let mut buffer = vec![0u16; PATH_CAPACITY];
	for (slot, unit) in buffer.iter_mut().zip(name.encode_utf16()) {
		*slot = unit;
	}

	let mut dialog = OPENFILENAMEW {
		lStructSize: size_of::<OPENFILENAMEW>() as u32,
		hwndOwner: owner,
		// pairs of label and pattern, each ending in a nul; `w!` adds the final one
		lpstrFilter: w!("PNG image (*.png)\0*.png\0"),
		lpstrFile: PWSTR(buffer.as_mut_ptr()),
		nMaxFile: buffer.len() as u32,
		lpstrDefExt: w!("png"),
		Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
		..Default::default()
	};
	if !unsafe { GetSaveFileNameW(&mut dialog) }.as_bool() {
		return None;
	}
	let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
	Some(PathBuf::from(OsString::from_wide(&buffer[..len])))
}

// Writes `image` to `path` as an 8-bit RGB PNG.
pub fn write_png(path: &Path, image: &Image) -> Result<(), Box<dyn std::error::Error>> {
	let file = BufWriter::new(File::create(path)?);
	let mut encoder = png::Encoder::new(file, image.width as u32, image.height as u32);
	encoder.set_color(png::ColorType::Rgb);
	encoder.set_depth(png::BitDepth::Eight);
	let mut writer = encoder.write_header()?;

	let rgb: Vec<u8> = image
		.pixels
		.iter()
		.flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
		.collect();
	writer.write_image_data(&rgb)?;
	writer.finish()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::BufReader;

	#[test]
	fn png_round_trips() {
		let image = Image {
			width: 2,
			height: 1,
			pixels: vec![0xFF11_2233, 0xFFAA_BBCC],
		};
		let path = std::env::temp_dir().join("snap-test-round-trip.png");
		write_png(&path, &image).unwrap();

		let decoder = png::Decoder::new(BufReader::new(File::open(&path).unwrap()));
		let mut reader = decoder.read_info().unwrap();
		let mut data = vec![0; reader.output_buffer_size().unwrap()];
		let info = reader.next_frame(&mut data).unwrap();
		let _ = std::fs::remove_file(&path);

		assert_eq!((info.width, info.height), (2, 1));
		assert_eq!(&data[..info.buffer_size()], &[0x11, 0x22, 0x33, 0xAA, 0xBB, 0xCC]);
	}
}
