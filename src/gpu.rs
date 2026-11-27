// gpu

// Drawing the overlay with Direct2D (on the GPU unless hardware acceleration is off) and its text with DirectWrite.
// The screenshot is uploaded once when the overlay opens; each frame is then a handful of draw calls. The settings
// menu opened from the tray has no screenshot: it's drawn see-through over the live screen into a pixel buffer, which
// `UpdateLayeredWindow` copies to the window each frame.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
	D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
	D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
	D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_CUSTOM,
	D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
	D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_MITER, D2D1_LINE_JOIN_ROUND,
	D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
	D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_STROKE_STYLE_PROPERTIES,
	D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory,
	ID2D1Geometry, ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
	DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
	DWRITE_GLYPH_RUN, DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_MATRIX, DWRITE_MEASURING_MODE, DWRITE_STRIKETHROUGH,
	DWRITE_TEXT_METRICS, DWRITE_UNDERLINE, DWriteCreateFactory, IDWriteFactory, IDWriteInlineObject,
	IDWritePixelSnapping_Impl, IDWriteTextFormat, IDWriteTextLayout, IDWriteTextRenderer, IDWriteTextRenderer_Impl,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{BOOL, IUnknown, Ref, Result, implement, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::buttons::{Button, Buttons, History};
use crate::capture::Image;
use crate::confirm::{Choice, Confirm};
use crate::gdi::Dib;
use crate::hints::Hints;
use crate::render::{around, outline};
use crate::selection::Rect;
use crate::settings::{self, Settings};

const DIM_OPACITY: f32 = 0.5; // the 50% black over everything outside the box
const DOTTED_OPACITY: f32 = 0.5;
const FLASH_OPACITY: f32 = 0.5;
const DASH: f32 = 4.0; // length of each dash and each gap in the dotted outlines, in pixels

// What to draw in one frame, besides the screenshot, in three layers: the dimming, the box with everything around it
// (hidden while a dialog is up), then the dialog and the hints over it all.
pub struct Scene<'a> {
	pub dim: f32,  // how much of the dimming shows, 0 to 1
	pub ui: f32,   // the opacity of everything drawn over the screenshot
	pub main: f32, // how much the box layer shows, on top of `ui`; as it hides, the box's hole in the dimming closes
	pub selection: Option<Rect>,
	pub highlight: &'a [Rect],
	pub labels: &'a [(Vector2, IDWriteTextLayout)],
	pub label_stroke: f32, // the width of the black stroke around the labels' text, half of it outside the glyphs
	pub dotted: &'a [(Rect, f32)], // outlines with their opacity
	pub flash: Option<(Rect, f32)>,
	pub hints: Option<(&'a Hints, f32)>,
	// the row, where it goes, the hovered button, what can be undone and redone, and its opacity
	pub buttons: Option<(&'a Buttons, Rect, Option<Button>, History, f32)>,
	pub close: Option<(&'a Buttons, Rect, bool, f32)>, // the close button in the top-right corner, hovered, opacity
	pub gear: Option<(&'a Buttons, Rect, bool, f32)>,  // the settings button left of it, the same way
	pub confirm: Option<(&'a Confirm, Option<Choice>, Instant)>, // with its hovered button, and the time to draw it at
	pub settings: Option<(&'a Settings, Option<settings::Target>, Instant)>, // the same way
}

// Brushes and the dash style for one render target.
pub struct Paint {
	pub white: ID2D1SolidColorBrush,
	pub black: ID2D1SolidColorBrush,
	dashes: ID2D1StrokeStyle,
	pub round: ID2D1StrokeStyle, // round joins and caps, for the black strokes under white shapes
}

struct Target {
	render_target: ID2D1HwndRenderTarget,
	hardware: bool,
	paint: Paint,
	screenshot: Option<ID2D1Bitmap>,
}

// The see-through version, drawn into `dib`.
struct LiveTarget {
	render_target: ID2D1DCRenderTarget,
	hardware: bool,
	paint: Paint,
	dib: Dib,
}

pub struct Gpu {
	pub d2d: ID2D1Factory,
	pub write: IDWriteFactory,
	target: Option<Target>,
	live: Option<LiveTarget>,
}

impl Gpu {
	pub fn new() -> Result<Self> {
		unsafe {
			Ok(Self {
				d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
				write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
				target: None,
				live: None,
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

	// Frees the uploaded screenshot, and the see-through version's pixel buffer, once the overlay closes.
	pub fn release_screenshot(&mut self) {
		if let Some(target) = self.target.as_mut() {
			target.screenshot = None;
		}
		self.live = None;
	}

	// Drops the window's render target, so the next `prepare` makes a new one. For after the window has been drawn
	// see-through, which changes how Windows shows it.
	pub fn release_target(&mut self) {
		self.target = None;
	}

	// Draws a frame with no screenshot, see-through where nothing covers it, and shows it in `window` (a layered window
	// at the screen's origin) at `opacity`, 0 to 1.
	pub fn draw_live(
		&mut self,
		window: HWND,
		(width, height): (i32, i32),
		hardware: bool,
		opacity: f32,
		scene: &Scene,
	) -> Result<()> {
		let reusable = self
			.live
			.as_ref()
			.is_some_and(|live| live.hardware == hardware && (live.dib.width, live.dib.height) == (width, height));
		if !reusable {
			self.live = None;
			let properties = D2D1_RENDER_TARGET_PROPERTIES {
				r#type: if hardware {
					D2D1_RENDER_TARGET_TYPE_DEFAULT
				} else {
					D2D1_RENDER_TARGET_TYPE_SOFTWARE
				},
				pixelFormat: D2D1_PIXEL_FORMAT {
					format: DXGI_FORMAT_B8G8R8A8_UNORM,
					alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
				},
				dpiX: 96.0,
				dpiY: 96.0,
				usage: D2D1_RENDER_TARGET_USAGE_NONE,
				minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
			};
			let render_target = unsafe { self.d2d.CreateDCRenderTarget(&properties)? };
			let paint = Paint::new(&self.d2d, &render_target)?;
			let dib = Dib::new(width, height)?;
			self.live = Some(LiveTarget {
				render_target,
				hardware,
				paint,
				dib,
			});
		}
		let Some(live) = &self.live else { return Ok(()) };
		let bounds = RECT {
			left: 0,
			top: 0,
			right: width,
			bottom: height,
		};
		let result = unsafe {
			live.render_target.BindDC(live.dib.dc, &bounds).and_then(|()| {
				live.render_target.BeginDraw();
				draw_scene(&live.render_target, &live.paint, None, (width, height), scene);
				live.render_target.EndDraw(None, None)
			})
		};
		if let Err(err) = result {
			if err.code() == D2DERR_RECREATE_TARGET {
				self.live = None;
			}
			return Err(err);
		}
		let blend = BLENDFUNCTION {
			BlendOp: AC_SRC_OVER as u8,
			BlendFlags: 0,
			SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
			AlphaFormat: AC_SRC_ALPHA as u8,
		};
		let size = SIZE { cx: width, cy: height };
		unsafe {
			UpdateLayeredWindow(
				window,
				None,
				Some(&POINT::default()),
				Some(&size),
				Some(live.dib.dc),
				Some(&POINT::default()),
				COLORREF(0),
				Some(&blend),
				ULW_ALPHA,
			)
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
			let size = screenshot.GetPixelSize();
			target.render_target.BeginDraw();
			let size = (size.width as i32, size.height as i32);
			draw_scene(&target.render_target, &target.paint, Some(screenshot), size, scene);
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
	// Draws what `draw` draws as one image faded to `opacity`, so a white shape at less than full opacity doesn't let
	// its black stroke show through. Only `bounds` (in the current transform) is kept.
	pub fn with_opacity(
		&self,
		render_target: &ID2D1RenderTarget,
		bounds: D2D_RECT_F,
		opacity: f32,
		draw: impl FnOnce(),
	) {
		let parameters = D2D1_LAYER_PARAMETERS {
			contentBounds: bounds,
			maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
			maskTransform: Matrix3x2::identity(),
			opacity,
			..Default::default()
		};
		// no layer object: Direct2D makes one per push (Windows 8+), so these can nest, as in the settings menu. One
		// shared layer pushed twice fails the whole frame in `EndDraw`.
		unsafe { render_target.PushLayer(&parameters, None) };
		draw();
		unsafe { render_target.PopLayer() };
	}

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
				round: d2d.CreateStrokeStyle(
					&D2D1_STROKE_STYLE_PROPERTIES {
						startCap: D2D1_CAP_STYLE_ROUND,
						endCap: D2D1_CAP_STYLE_ROUND,
						lineJoin: D2D1_LINE_JOIN_ROUND,
						..Default::default()
					},
					None,
				)?,
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

// Draws one frame of a `width` × `height` screen between `BeginDraw` and `EndDraw`. With no screenshot, it starts from
// fully see-through.
pub fn draw_scene(
	render_target: &ID2D1RenderTarget,
	paint: &Paint,
	screenshot: Option<&ID2D1Bitmap>,
	(width, height): (i32, i32),
	scene: &Scene,
) {
	unsafe {
		render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		render_target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

		match screenshot {
			Some(screenshot) => render_target.DrawBitmap(
				screenshot,
				None,
				1.0,
				D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
				None,
			),
			None => render_target.Clear(Some(&D2D1_COLOR_F::default())),
		}
		paint.black.SetOpacity(DIM_OPACITY * scene.dim);
		for rect in around(scene.selection, width, height) {
			render_target.FillRectangle(&rect_f(rect), &paint.black);
		}
		if let Some(rect) = scene.selection
			&& scene.main < 1.0
		{
			paint.black.SetOpacity(DIM_OPACITY * scene.dim * (1.0 - scene.main));
			render_target.FillRectangle(&rect_f(rect), &paint.black);
		}

		// the box layer
		let ui = scene.ui * scene.main;
		paint.white.SetOpacity(ui);
		if let Some(rect) = scene.selection {
			// a black line just outside the white one, so the outline shows on white too
			paint.black.SetOpacity(ui);
			let around_outline = Rect {
				left: rect.left - 1,
				top: rect.top - 1,
				right: rect.right + 1,
				bottom: rect.bottom + 1,
			};
			for strip in outline(around_outline) {
				render_target.FillRectangle(&rect_f(strip), &paint.black);
			}
			for strip in outline(rect).iter().chain(scene.highlight) {
				render_target.FillRectangle(&rect_f(*strip), &paint.white);
			}
		}
		paint.white.SetOpacity(1.0);
		paint.black.SetOpacity(1.0);
		for (rect, opacity) in scene.dotted {
			// centered on the pixels just outside the box, where the solid outline goes
			let line = D2D_RECT_F {
				left: rect.left as f32 - 0.5,
				top: rect.top as f32 - 0.5,
				right: rect.right as f32 + 0.5,
				bottom: rect.bottom as f32 + 0.5,
			};
			let bounds = D2D_RECT_F {
				left: line.left - 1.0,
				top: line.top - 1.0,
				right: line.right + 1.0,
				bottom: line.bottom + 1.0,
			};
			// white dashes over a solid black line, so the gaps are black and it shows on white too
			paint.with_opacity(render_target, bounds, DOTTED_OPACITY * opacity * ui, || {
				render_target.DrawRectangle(&line, &paint.black, 1.0, None);
				render_target.DrawRectangle(&line, &paint.white, 1.0, &paint.dashes);
			});
		}
		paint.white.SetOpacity(ui);
		paint.black.SetOpacity(ui);
		let factory = render_target.GetFactory();
		for (origin, layout) in scene.labels {
			// a round black stroke under the text keeps it readable over the bright box (skipped if it can't be made)
			if let Ok(factory) = &factory
				&& let Ok(outlines) = text_outlines(factory, layout, *origin)
			{
				render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
				for outline in outlines {
					render_target.DrawGeometry(&outline, &paint.black, scene.label_stroke, &paint.round);
				}
				render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
			}
			render_target.DrawTextLayout(*origin, layout, &paint.white, D2D1_DRAW_TEXT_OPTIONS_NONE);
		}
		if let Some((buttons, row, hovered, history, opacity)) = scene.buttons {
			buttons.draw(render_target, paint, row, hovered, history, opacity * ui);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
		for (button, corner) in [(Button::Close, scene.close), (Button::Gear, scene.gear)] {
			if let Some((buttons, rect, hovered, opacity)) = corner {
				buttons.draw_one(render_target, paint, button, rect, hovered, true, opacity * ui);
				render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
			}
		}
		if let Some((rect, opacity)) = scene.flash {
			paint.white.SetOpacity(FLASH_OPACITY * opacity * scene.main);
			render_target.FillRectangle(&rect_f(rect), &paint.white);
		}

		// the dialogs, then the hints, which describe them too
		if let Some((confirm, hovered, now)) = scene.confirm {
			confirm.draw(render_target, paint, scene.ui, hovered, now);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
		if let Some((settings, hovered, now)) = scene.settings {
			settings.draw(render_target, paint, scene.ui, hovered, now);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
		if let Some((hints, opacity)) = scene.hints {
			hints.draw(render_target, paint, opacity * scene.ui);
			render_target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
		}
	}
}

// The outlines of the glyphs `layout` draws at `origin`, one geometry per glyph run.
pub fn text_outlines(d2d: &ID2D1Factory, layout: &IDWriteTextLayout, origin: Vector2) -> Result<Vec<ID2D1Geometry>> {
	let outlines = Rc::new(RefCell::new(Vec::new()));
	let collector: IDWriteTextRenderer = OutlineCollector {
		d2d: d2d.clone(),
		outlines: outlines.clone(),
	}
	.into();
	unsafe { layout.Draw(None, &collector, origin.X, origin.Y)? };
	Ok(outlines.take())
}

// A text renderer that draws nothing, only turning each glyph run into a geometry. DirectWrite has no way to stroke
// text, so this is how the glyph shapes are reached.
#[implement(IDWriteTextRenderer)]
struct OutlineCollector {
	d2d: ID2D1Factory,
	outlines: Rc<RefCell<Vec<ID2D1Geometry>>>,
}

// pixel snapping and one pixel per DIP, as `DrawTextLayout` does on the overlay's 96 DPI target, so the outlines line
// up with the text drawn over them
impl IDWritePixelSnapping_Impl for OutlineCollector_Impl {
	fn IsPixelSnappingDisabled(&self, _context: *const c_void) -> Result<BOOL> {
		Ok(false.into())
	}

	fn GetCurrentTransform(&self, _context: *const c_void, transform: *mut DWRITE_MATRIX) -> Result<()> {
		let identity = DWRITE_MATRIX {
			m11: 1.0,
			m22: 1.0,
			..Default::default()
		};
		unsafe { transform.write(identity) };
		Ok(())
	}

	fn GetPixelsPerDip(&self, _context: *const c_void) -> Result<f32> {
		Ok(1.0)
	}
}

impl IDWriteTextRenderer_Impl for OutlineCollector_Impl {
	fn DrawGlyphRun(
		&self,
		_context: *const c_void,
		x: f32,
		y: f32,
		_mode: DWRITE_MEASURING_MODE,
		run: *const DWRITE_GLYPH_RUN,
		_description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
		_effect: Ref<IUnknown>,
	) -> Result<()> {
		unsafe {
			let run = &*run;
			let Some(face) = run.fontFace.as_ref() else {
				return Ok(());
			};
			let path = self.d2d.CreatePathGeometry()?;
			let sink = path.Open()?;
			// in run space, with the baseline origin at 0, 0
			face.GetGlyphRunOutline(
				run.fontEmSize,
				run.glyphIndices,
				Some(run.glyphAdvances),
				Some(run.glyphOffsets),
				run.glyphCount,
				run.isSideways.as_bool(),
				run.bidiLevel % 2 == 1,
				&sink,
			)?;
			sink.Close()?;
			let placed = self
				.d2d
				.CreateTransformedGeometry(&path, &Matrix3x2::translation(x, y))?;
			self.outlines.borrow_mut().push(placed.into());
		}
		Ok(())
	}

	// the labels have no underlines, strikethroughs or inline objects
	fn DrawUnderline(
		&self,
		_: *const c_void,
		_: f32,
		_: f32,
		_: *const DWRITE_UNDERLINE,
		_: Ref<IUnknown>,
	) -> Result<()> {
		Ok(())
	}

	fn DrawStrikethrough(
		&self,
		_: *const c_void,
		_: f32,
		_: f32,
		_: *const DWRITE_STRIKETHROUGH,
		_: Ref<IUnknown>,
	) -> Result<()> {
		Ok(())
	}

	fn DrawInlineObject(
		&self,
		_: *const c_void,
		_: f32,
		_: f32,
		_: Ref<IDWriteInlineObject>,
		_: BOOL,
		_: BOOL,
		_: Ref<IUnknown>,
	) -> Result<()> {
		Ok(())
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
