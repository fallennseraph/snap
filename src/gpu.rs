// gpu

// Drawing the overlay with Direct2D (on the GPU unless hardware acceleration is off) and its text with DirectWrite.
// The screenshot is uploaded once when the overlay opens; each frame is then a handful of draw calls.

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
	D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES,
	D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_CUSTOM, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
	D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINE_JOIN_MITER, D2D1_PRESENT_OPTIONS_NONE,
	D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
	D2D1_RENDER_TARGET_USAGE_NONE, D2D1_STROKE_STYLE_PROPERTIES, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
	ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
	DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
	DWRITE_TEXT_METRICS, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{Result, w};
use windows_numerics::Vector2;

use crate::buttons::{Button, Buttons};
use crate::capture::Image;
use crate::hints::Hints;
use crate::render::{around, outline};
use crate::selection::Rect;

const DIM_OPACITY: f32 = 0.5; // the 50% black over everything outside the box
const DOTTED_OPACITY: f32 = 0.5;
const FLASH_OPACITY: f32 = 0.5;
const DASH: f32 = 4.0; // length of each dash and each gap in the dotted outlines, in pixels

// What to draw in one frame, besides the screenshot.
pub struct Scene<'a> {
	pub dim: f32, // how much of the dimming shows, 0 to 1
	pub ui: f32,  // the opacity of everything drawn over the screenshot
	pub selection: Option<Rect>,
	pub highlight: &'a [Rect],
	pub labels: &'a [(Vector2, IDWriteTextLayout)],
	pub dotted: &'a [(Rect, f32)], // outlines with their opacity
	pub flash: Option<(Rect, f32)>,
	pub hints: Option<(&'a Hints, f32)>,
	// the row, where it goes, the hovered button and its opacity
	pub buttons: Option<(&'a Buttons, Rect, Option<Button>, f32)>,
}

// Brushes and the dash style for one render target.
pub struct Paint {
	pub white: ID2D1SolidColorBrush,
	black: ID2D1SolidColorBrush,
	dashes: ID2D1StrokeStyle,
}

struct Target {
	render_target: ID2D1HwndRenderTarget,
	hardware: bool,
	paint: Paint,
	screenshot: Option<ID2D1Bitmap>,
}

pub struct Gpu {
	pub d2d: ID2D1Factory,
	pub write: IDWriteFactory,
	target: Option<Target>,
}

impl Gpu {
	pub fn new() -> Result<Self> {
		unsafe {
			Ok(Self {
				d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
				write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
				target: None,
			})
		}
	}

	// Makes sure there's a render target for `window` at this size, on the GPU or not as `hardware` says.
	pub fn prepare(&mut self, window: HWND, width: i32, height: i32, hardware: bool) -> Result<()> {
		let size = D2D_SIZE_U {
			width: width as u32,
			height: height as u32,
		};
		if let Some(target) = &self.target
			&& target.hardware == hardware
		{
			if unsafe { target.render_target.GetPixelSize() } != size {
				unsafe { target.render_target.Resize(&size)? };
			}
			return Ok(());
		}

		let properties = D2D1_RENDER_TARGET_PROPERTIES {
			r#type: if hardware {
				D2D1_RENDER_TARGET_TYPE_DEFAULT
			} else {
				D2D1_RENDER_TARGET_TYPE_SOFTWARE
			},
			pixelFormat: D2D1_PIXEL_FORMAT {
				format: DXGI_FORMAT_B8G8R8A8_UNORM,
				alphaMode: D2D1_ALPHA_MODE_IGNORE,
			},
			// 96 DPI makes one Direct2D unit one pixel; the process handles scaling itself
			dpiX: 96.0,
			dpiY: 96.0,
			usage: D2D1_RENDER_TARGET_USAGE_NONE,
			minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
		};
		let window_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
			hwnd: window,
			pixelSize: size,
			presentOptions: D2D1_PRESENT_OPTIONS_NONE,
		};
		let render_target = unsafe { self.d2d.CreateHwndRenderTarget(&properties, &window_properties)? };
		let paint = Paint::new(&self.d2d, &render_target)?;
		self.target = Some(Target {
			render_target,
			hardware,
			paint,
			screenshot: None,
		});
		Ok(())
	}

	// Uploads the screenshot to the render target from `prepare`, unless it's already there.
	pub fn upload(&mut self, image: &Image) -> Result<()> {
		let Some(target) = self.target.as_mut() else {
			return Ok(());
		};
		if target.screenshot.is_none() {
			target.screenshot = Some(create_bitmap(&target.render_target, image)?);
		}
		Ok(())
	}

	// Frees the uploaded screenshot once the overlay closes.
	pub fn release_screenshot(&mut self) {
		if let Some(target) = self.target.as_mut() {
			target.screenshot = None;
		}
	}

	// Draws a frame. If the GPU was reset (a driver update, for example), the render target is dropped, so the next
	// `prepare` and `upload` make a new one.
	pub fn draw(&mut self, scene: &Scene) -> Result<()> {
		let Some(target) = &self.target else { return Ok(()) };
		let Some(screenshot) = &target.screenshot else {
			return Ok(());
		};
		let result = unsafe {
			target.render_target.BeginDraw();
			draw_scene(&target.render_target, &target.paint, screenshot, scene);
			target.render_target.EndDraw(None, None)
		};
		if let Err(err) = &result
			&& err.code() == D2DERR_RECREATE_TARGET
		{
			self.target = None;
		}
		result
	}
}

impl Paint {
	pub fn new(d2d: &ID2D1Factory, render_target: &ID2D1RenderTarget) -> Result<Self> {
		unsafe {
			let white = D2D1_COLOR_F {
				r: 1.0,
				g: 1.0,
				b: 1.0,
				a: 1.0,
			};
			let black = D2D1_COLOR_F {
				r: 0.0,
				g: 0.0,
				b: 0.0,
				a: 1.0,
			};
			let stroke = D2D1_STROKE_STYLE_PROPERTIES {
				startCap: D2D1_CAP_STYLE_FLAT,
				endCap: D2D1_CAP_STYLE_FLAT,
				dashCap: D2D1_CAP_STYLE_FLAT,
				lineJoin: D2D1_LINE_JOIN_MITER,
				miterLimit: 10.0,
				dashStyle: D2D1_DASH_STYLE_CUSTOM,
				dashOffset: 0.0,
			};
			Ok(Self {
				white: render_target.CreateSolidColorBrush(&white, None)?,
				black: render_target.CreateSolidColorBrush(&black, None)?,
				// dash lengths are in stroke widths, and the outlines are 1 px wide
				dashes: d2d.CreateStrokeStyle(&stroke, Some(&[DASH, DASH]))?,
			})
		}
	}
}

pub fn create_bitmap(render_target: &ID2D1RenderTarget, image: &Image) -> Result<ID2D1Bitmap> {
	let properties = D2D1_BITMAP_PROPERTIES {
		// the screenshot's alpha is always 255, so premultiplied and straight are the same
		pixelFormat: D2D1_PIXEL_FORMAT {
			format: DXGI_FORMAT_B8G8R8A8_UNORM,
			alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
		},
		dpiX: 96.0,
		dpiY: 96.0,
	};
	let size = D2D_SIZE_U {
		width: image.width as u32,
		height: image.height as u32,
	};
	unsafe {
		render_target.CreateBitmap(
			size,
			Some(image.pixels.as_ptr().cast()),
			image.width as u32 * 4,
			&properties,
		)
	}
}

pub fn rect_f(rect: Rect) -> D2D_RECT_F {
	D2D_RECT_F {
		left: rect.left as f32,
		top: rect.top as f32,
		right: rect.right as f32,
		bottom: rect.bottom as f32,
	}
}

// Draws one frame between `BeginDraw` and `EndDraw`.
pub fn draw_scene(render_target: &ID2D1RenderTarget, paint: &Paint, screenshot: &ID2D1Bitmap, scene: &Scene) {
	unsafe {
		let size = screenshot.GetPixelSize();
		let (width, height) = (size.width as i32, size.height as i32);
		render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		render_target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

		render_target.DrawBitmap(
			screenshot,
			None,
			1.0,
			D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
			None,
		);
		paint.black.SetOpacity(DIM_OPACITY * scene.dim);
		for rect in around(scene.selection, width, height) {
			render_target.FillRectangle(&rect_f(rect), &paint.black);
		}

		paint.white.SetOpacity(scene.ui);
		if let Some(rect) = scene.selection {
			for strip in outline(rect).iter().chain(scene.highlight) {
				render_target.FillRectangle(&rect_f(*strip), &paint.white);
			}
		}
		for (rect, opacity) in scene.dotted {
			paint.white.SetOpacity(DOTTED_OPACITY * opacity * scene.ui);
			// centered on the pixels just outside the box, where the solid outline goes
			let line = D2D_RECT_F {
				left: rect.left as f32 - 0.5,
				top: rect.top as f32 - 0.5,
				right: rect.right as f32 + 0.5,
				bottom: rect.bottom as f32 + 0.5,
			};
			render_target.DrawRectangle(&line, &paint.white, 1.0, &paint.dashes);
		}
		paint.white.SetOpacity(scene.ui);
		for (origin, layout) in scene.labels {
			render_target.DrawTextLayout(*origin, layout, &paint.white, D2D1_DRAW_TEXT_OPTIONS_NONE);
		}
		if let Some((buttons, row, hovered, opacity)) = scene.buttons {
			buttons.draw(render_target, &paint.white, row, hovered, opacity * scene.ui);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
		if let Some((hints, opacity)) = scene.hints {
			hints.draw(render_target, &paint.white, opacity * scene.ui);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
		if let Some((rect, opacity)) = scene.flash {
			paint.white.SetOpacity(FLASH_OPACITY * opacity);
			render_target.FillRectangle(&rect_f(rect), &paint.white);
		}
	}
}

// Segoe UI at `size` pixels.
pub fn text_format(write: &IDWriteFactory, size: f32) -> Result<IDWriteTextFormat> {
	unsafe {
		write.CreateTextFormat(
			w!("Segoe UI"),
			None,
			DWRITE_FONT_WEIGHT_NORMAL,
			DWRITE_FONT_STYLE_NORMAL,
			DWRITE_FONT_STRETCH_NORMAL,
			size,
			w!("en-us"),
		)
	}
}

// Lays out one line of text. Returns the layout with its width and height in pixels.
pub fn text_layout(
	write: &IDWriteFactory,
	format: &IDWriteTextFormat,
	text: &str,
) -> Result<(IDWriteTextLayout, f32, f32)> {
	let text: Vec<u16> = text.encode_utf16().collect();
	unsafe {
		let layout = write.CreateTextLayout(&text, format, f32::MAX, f32::MAX)?;
		let mut metrics = DWRITE_TEXT_METRICS::default();
		layout.GetMetrics(&mut metrics)?;
		Ok((layout, metrics.width, metrics.height))
	}
}
