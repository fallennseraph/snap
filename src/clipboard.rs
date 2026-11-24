// clipboard

// Putting an image on the clipboard.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::core::{Error, Result};

use crate::capture::Image;

// Replaces the clipboard contents with `image` as a `CF_DIB`. Windows converts it to the other bitmap formats
// on request.
pub fn copy_image(owner: HWND, image: &Image) -> Result<()> {
	let header = BITMAPINFOHEADER {
		biSize: size_of::<BITMAPINFOHEADER>() as u32,
		biWidth: image.width,
		// positive height means bottom-up rows, the layout every app reading `CF_DIB` expects
		biHeight: image.height,
		biPlanes: 1,
		biBitCount: 32,
		biCompression: BI_RGB.0,
		..Default::default()
	};
	let header_size = size_of::<BITMAPINFOHEADER>();
	let row_len = image.width as usize;

	unsafe {
		let memory = GlobalAlloc(GMEM_MOVEABLE, header_size + image.pixels.len() * 4)?;
		let base = GlobalLock(memory).cast::<u8>();
		if base.is_null() {
			let err = Error::from_thread();
			let _ = GlobalFree(Some(memory));
			return Err(err);
		}
		base.cast::<BITMAPINFOHEADER>().write_unaligned(header);
		let pixels = base.add(header_size).cast::<u32>();
		for (i, row) in image.pixels.chunks_exact(row_len).rev().enumerate() {
			std::ptr::copy_nonoverlapping(row.as_ptr(), pixels.add(i * row_len), row_len);
		}
		let _ = GlobalUnlock(memory);

		OpenClipboard(Some(owner))?;
		let result = EmptyClipboard().and_then(|()| SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(memory.0))));
		let _ = CloseClipboard();
		// the clipboard owns the memory only if SetClipboardData worked
		if result.is_err() {
			let _ = GlobalFree(Some(memory));
		}
		result.map(|_| ())
	}
}
