// capture

// Taking the screenshot.

use windows::Win32::Graphics::Gdi::{BitBlt, CAPTUREBLT, GetDC, ReleaseDC, SRCCOPY};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use windows::core::{Error, Result};

use crate::gdi::Dib;
use crate::selection::Rect;

// A screenshot or part of one. Pixels are `0xFFRRGGBB`, row by row from the top.
#[derive(Clone)]
pub struct Image {
	pub width: i32,
	pub height: i32,
	pub pixels: Vec<u32>,
}

impl Image {
	// The part of the image inside `rect`, which must lie within the image.
	pub fn crop(&self, rect: Rect) -> Image {
		let mut pixels = Vec::with_capacity((rect.width() * rect.height()) as usize);
		for y in rect.top..rect.bottom {
			let row = (y * self.width) as usize;
			pixels.extend_from_slice(&self.pixels[row + rect.left as usize..row + rect.right as usize]);
		}
		Image {
			width: rect.width(),
			height: rect.height(),
			pixels,
		}
	}
}

// Captures the primary monitor in physical pixels (the process is per-monitor DPI aware).
pub fn capture_screen() -> Result<Image> {
	unsafe {
		let width = GetSystemMetrics(SM_CXSCREEN);
		let height = GetSystemMetrics(SM_CYSCREEN);
		let dib = Dib::new(width, height)?;

		let screen = GetDC(None);
		if screen.is_invalid() {
			return Err(Error::from_thread());
		}
		// CAPTUREBLT includes layered windows like tooltips and menus
		let result = BitBlt(dib.dc, 0, 0, width, height, Some(screen), 0, 0, SRCCOPY | CAPTUREBLT);
		ReleaseDC(None, screen);
		result?;

		// GDI leaves alpha at 0, which some apps read as fully transparent
		let pixels = dib.pixels().iter().map(|p| p | 0xFF00_0000).collect();
		Ok(Image { width, height, pixels })
	}
}
