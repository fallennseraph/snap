// capture

// Taking the screenshot.

use windows::Win32::Graphics::Gdi::{BitBlt, CAPTUREBLT, DeleteObject, GetDC, HDC, ReleaseDC, SRCCOPY};
use windows::Win32::UI::WindowsAndMessaging::{
	CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, GetSystemMetrics, HICON, ICONINFO,
	SM_CXSCREEN, SM_CYSCREEN,
};
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

// Captures the primary monitor in physical pixels (the process is per-monitor DPI aware), with the mouse cursor drawn
// in if `cursor` is on.
pub fn capture_screen(cursor: bool) -> Result<Image> {
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
		// BitBlt never includes the cursor, so it's drawn in by hand. A screenshot without it is better than none.
		if cursor && let Err(err) = draw_cursor(dib.dc) {
			eprintln!("snap: couldn't draw the cursor into the screenshot: {err}");
		}

		// GDI leaves alpha at 0, which some apps read as fully transparent
		let pixels = dib.pixels().iter().map(|p| p | 0xFF00_0000).collect();
		Ok(Image { width, height, pixels })
	}
}

// Draws the mouse cursor onto `dc` where it is on screen, unless it's hidden.
unsafe fn draw_cursor(dc: HDC) -> Result<()> {
	unsafe {
		let mut info = CURSORINFO {
			cbSize: size_of::<CURSORINFO>() as u32,
			..Default::default()
		};
		GetCursorInfo(&mut info)?;
		if info.flags.0 & CURSOR_SHOWING.0 == 0 {
			return Ok(());
		}
		let icon = HICON(info.hCursor.0);
		// the hotspot is the point the cursor's position refers to, like the arrow's tip
		let mut icon_info = ICONINFO::default();
		GetIconInfo(icon, &mut icon_info)?;
		// GetIconInfo makes copies of the cursor's bitmaps, which are freed here
		let _ = DeleteObject(icon_info.hbmMask.into());
		if !icon_info.hbmColor.is_invalid() {
			let _ = DeleteObject(icon_info.hbmColor.into());
		}
		let (x, y) = (
			info.ptScreenPos.x - icon_info.xHotspot as i32,
			info.ptScreenPos.y - icon_info.yHotspot as i32,
		);
		DrawIconEx(dc, x, y, icon, 0, 0, 0, None, DI_NORMAL)
	}
}
