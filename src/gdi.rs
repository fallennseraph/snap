// gdi

// A 32-bit pixel buffer that GDI can draw into, for taking screenshots.

use windows::Win32::Graphics::Gdi::{
	BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
	GdiFlush, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::core::{Error, Result};

// A top-down 32-bit DIB section selected into its own memory DC. Pixels are `0xAARRGGBB`.
pub struct Dib {
	pub dc: HDC,
	pub width: i32,
	pub height: i32,
	bitmap: HBITMAP,
	old: HGDIOBJ,
	bits: *mut u32,
}

impl Dib {
	pub fn new(width: i32, height: i32) -> Result<Self> {
		let info = BITMAPINFO {
			bmiHeader: BITMAPINFOHEADER {
				biSize: size_of::<BITMAPINFOHEADER>() as u32,
				biWidth: width,
				// negative height makes the rows top-down, matching screen coordinates
				biHeight: -height,
				biPlanes: 1,
				biBitCount: 32,
				biCompression: BI_RGB.0,
				..Default::default()
			},
			..Default::default()
		};
		unsafe {
			let dc = CreateCompatibleDC(None);
			if dc.is_invalid() {
				return Err(Error::from_thread());
			}
			let mut bits = std::ptr::null_mut();
			let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
				Ok(bitmap) => bitmap,
				Err(err) => {
					let _ = DeleteDC(dc);
					return Err(err);
				}
			};
			let old = SelectObject(dc, bitmap.into());
			Ok(Self {
				dc,
				width,
				height,
				bitmap,
				old,
				bits: bits.cast(),
			})
		}
	}

	pub fn pixels(&self) -> &[u32] {
		// GDI batches drawing calls, so finish them before reading
		unsafe {
			let _ = GdiFlush();
		}
		// SAFETY: the DIB section holds exactly `width * height` pixels and lives as long as `self`
		unsafe { std::slice::from_raw_parts(self.bits, self.len()) }
	}

	fn len(&self) -> usize {
		self.width as usize * self.height as usize
	}
}

impl Drop for Dib {
	fn drop(&mut self) {
		unsafe {
			SelectObject(self.dc, self.old);
			let _ = DeleteObject(self.bitmap.into());
			let _ = DeleteDC(self.dc);
		}
	}
}
