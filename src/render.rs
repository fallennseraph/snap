// render

// Where things go on the overlay: the outline, highlights, labels, buttons and dimmed area, in screen pixels.

use crate::selection::{Grip, Rect};

// `size` at 100% display scaling, scaled up for the current scaling.
pub fn scaled(size: i32, scale: f32) -> i32 {
	(size as f32 * scale).round() as i32
}

// The parts of a `width` by `height` screen outside `rect`: four strips around it, or the whole screen if there's no
// box.
pub fn around(rect: Option<Rect>, width: i32, height: i32) -> Vec<Rect> {
	let screen = Rect {
		left: 0,
		top: 0,
		right: width,
		bottom: height,
	};
	let Some(r) = rect.and_then(|r| r.intersect(&screen)) else {
		return vec![screen];
	};
	[
		Rect {
			left: 0,
			top: 0,
			right: width,
			bottom: r.top,
		},
		Rect {
			left: 0,
			top: r.bottom,
			right: width,
			bottom: height,
		},
		Rect {
			left: 0,
			top: r.top,
			right: r.left,
			bottom: r.bottom,
		},
		Rect {
			left: r.right,
			top: r.top,
			right: width,
			bottom: r.bottom,
		},
	]
	.into_iter()
	.filter(|strip| !strip.is_empty())
	.collect()
}

// The four 1 px strips just outside `r`.
pub fn outline(r: Rect) -> [Rect; 4] {
	[
		Rect {
			left: r.left - 1,
			top: r.top - 1,
			right: r.right + 1,
			bottom: r.top,
		},
		Rect {
			left: r.left - 1,
			top: r.bottom,
			right: r.right + 1,
			bottom: r.bottom + 1,
		},
		Rect {
			left: r.left - 1,
			top: r.top,
			right: r.left,
			bottom: r.bottom,
		},
		Rect {
			left: r.right,
			top: r.top,
			right: r.right + 1,
			bottom: r.bottom,
		},
	]
}

// Strips `thickness` wide just outside the gripped edges of `r`. Exactly the edge's length for an edge grip; for a
// corner, an L
// reaching `length` along each edge from the corner, but at most half the box so small boxes don't get a corner
// highlight covering the whole edge.
pub fn highlight(r: Rect, grip: Grip, thickness: i32, length: i32) -> Vec<Rect> {
	let (length_x, length_y) = (length.min(r.width() / 2), length.min(r.height() / 2));
	let (v0, v1) = if grip.top {
		(r.top - thickness, r.top + length_y)
	} else if grip.bottom {
		(r.bottom - length_y, r.bottom + thickness)
	} else {
		(r.top, r.bottom)
	};
	let (h0, h1) = if grip.left {
		(r.left - thickness, r.left + length_x)
	} else if grip.right {
		(r.right - length_x, r.right + thickness)
	} else {
		(r.left, r.right)
	};

	let mut strips = Vec::new();
	if grip.left {
		strips.push(Rect {
			left: r.left - thickness,
			top: v0,
			right: r.left,
			bottom: v1,
		});
	}
	if grip.right {
		strips.push(Rect {
			left: r.right,
			top: v0,
			right: r.right + thickness,
			bottom: v1,
		});
	}
	if grip.top {
		strips.push(Rect {
			left: h0,
			top: r.top - thickness,
			right: h1,
			bottom: r.top,
		});
	}
	if grip.bottom {
		strips.push(Rect {
			left: h0,
			top: r.bottom,
			right: h1,
			bottom: r.bottom + thickness,
		});
	}
	strips
}

// Where the position label goes: just above the box's top-left corner and its outline (and above the size label and
// buttons when they're above the box too), or just inside the box when that would go off the top of the screen. Kept
// on screen horizontally.
pub fn position_label(
	r: Rect,
	label_width: i32,
	label_height: i32,
	gap: i32,
	under: &Under,
	screen_width: i32,
) -> (i32, i32) {
	let x = r.left.min(screen_width - label_width).max(0);
	let top = if under.bottom() <= r.top {
		under.top()
	} else {
		r.top - 1
	};
	let above = top - gap - label_height;
	let y = if above >= 0 { above } else { r.top + gap };
	(x, y)
}

// Where the size label and the row of buttons go, from `under_box`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Under {
	pub label: Rect,
	pub buttons: Rect,
}

impl Under {
	fn top(&self) -> i32 {
		self.label.top.min(self.buttons.top)
	}

	fn bottom(&self) -> i32 {
		self.label.bottom.max(self.buttons.bottom)
	}

	// `t` of the way from `self` to `other`, on whole pixels.
	pub fn lerp(&self, other: &Under, t: f32) -> Under {
		let mix = |a: i32, b: i32| a + ((b - a) as f32 * t).round() as i32;
		let rect = |a: Rect, b: Rect| Rect {
			left: mix(a.left, b.left),
			top: mix(a.top, b.top),
			right: mix(a.right, b.right),
			bottom: mix(a.bottom, b.bottom),
		};
		Under {
			label: rect(self.label, other.label),
			buttons: rect(self.buttons, other.buttons),
		}
	}
}

fn sized(left: i32, top: i32, width: i32, height: i32) -> Rect {
	Rect {
		left,
		top,
		right: left + width,
		bottom: top + height,
	}
}

// Whether the row of buttons goes under the size label instead of beside it: when the two would touch (on a narrow
// box), even with the row pulled inside the box's corner.
pub fn is_stacked(r: Rect, label_width: i32, row_width: i32, gap: i32, screen_width: i32) -> bool {
	let label_x = r.left.min(screen_width - label_width).max(0);
	let row_left = (r.right - gap - row_width).min(screen_width - row_width).max(0);
	label_x + label_width + gap > row_left
}

// Where the size label and the row of buttons go, given each one's width and height: the label just under the box's
// bottom-left corner, and the row under its bottom-right corner, lined up with the outside of its outline, with the
// label centered on the row. When `stacked` (see `is_stacked`), the row goes under the label instead, lined up with
// it.
//
// When that would go off the bottom of the screen, they go just inside the box's bottom corners, or above its top
// corners if the box is too short for that. Kept on screen horizontally.
pub fn under_box(
	r: Rect,
	label: (i32, i32),
	buttons: (i32, i32),
	gap: i32,
	screen: (i32, i32),
	stacked: bool,
) -> Under {
	let ((label_width, label_height), (row_width, row_height)) = (label, buttons);
	let (screen_width, screen_height) = screen;
	let label_x = r.left.min(screen_width - label_width).max(0);
	let row = |left: i32, top: i32| sized(left.min(screen_width - row_width).max(0), top, row_width, row_height);
	// with the row's right edge at `right` (unless it's stacked) and the top of both at `top`
	let place = |right: i32, top: i32| {
		if !stacked {
			Under {
				label: sized(
					label_x,
					top + (row_height - label_height) / 2,
					label_width,
					label_height,
				),
				buttons: row(right - row_width, top),
			}
		} else {
			Under {
				label: sized(label_x, top, label_width, label_height),
				buttons: row(label_x, top + label_height + gap),
			}
		}
	};
	// `place` at a top of 0 gives the height of the pair
	let height = |right: i32| place(right, 0).bottom() - place(right, 0).top();

	let below = place(r.right + 1, r.bottom + 1 + gap);
	if below.bottom() <= screen_height {
		return below;
	}
	let inside = place(r.right - gap, r.bottom - gap - height(r.right - gap));
	if inside.top() >= r.top + gap {
		return inside;
	}
	place(r.right + 1, r.top - 1 - gap - height(r.right + 1))
}

#[cfg(test)]
mod tests {
	use super::*;

	// an 80 by 24 row of buttons and a 60 by 16 size label on a 1920 by 1080 screen
	fn under(r: Rect) -> Under {
		let stacked = is_stacked(r, 60, 80, 4, 1920);
		under_box(r, (60, 16), (80, 24), 4, (1920, 1080), stacked)
	}

	fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
		Rect {
			left,
			top,
			right,
			bottom,
		}
	}

	#[test]
	fn label_sits_above_box() {
		let r = rect(100, 100, 300, 200);
		assert_eq!(position_label(r, 80, 16, 4, &under(r), 1920), (100, 79));
	}

	#[test]
	fn label_moves_inside_at_top_of_screen() {
		let r = rect(100, 10, 300, 200);
		assert_eq!(position_label(r, 80, 16, 4, &under(r), 1920), (100, 14));
	}

	#[test]
	fn label_stays_on_screen_at_right_edge() {
		let r = rect(1900, 100, 1920, 200);
		assert_eq!(position_label(r, 80, 16, 4, &under(r), 1920).0, 1840);
	}

	#[test]
	fn size_label_and_buttons_sit_under_the_bottom_corners() {
		let r = rect(100, 100, 300, 200);
		assert_eq!(
			under(r),
			Under {
				label: rect(100, 209, 160, 225),
				buttons: rect(221, 205, 301, 229),
			}
		);
	}

	#[test]
	fn buttons_go_under_the_size_label_on_a_narrow_box() {
		let r = rect(500, 100, 600, 200);
		assert_eq!(
			under(r),
			Under {
				label: rect(500, 205, 560, 221),
				buttons: rect(500, 225, 580, 249),
			}
		);
	}

	#[test]
	fn they_move_inside_at_bottom_of_screen() {
		let r = rect(100, 100, 300, 1070);
		assert_eq!(
			under(r),
			Under {
				label: rect(100, 1046, 160, 1062),
				buttons: rect(216, 1042, 296, 1066),
			}
		);
		// stacked on a narrow box, the label above the buttons
		let r = rect(100, 100, 150, 1070);
		assert_eq!(
			under(r),
			Under {
				label: rect(100, 1022, 160, 1038),
				buttons: rect(100, 1042, 180, 1066),
			}
		);
	}

	#[test]
	fn they_go_above_a_short_box_at_bottom_of_screen() {
		let r = rect(100, 1060, 300, 1075);
		assert_eq!(under(r).buttons, rect(221, 1031, 301, 1055));
		// and the position label goes above them
		assert_eq!(position_label(r, 80, 16, 4, &under(r), 1920).1, 1011);
	}

	#[test]
	fn stacking_leaves_room_for_the_row_pulled_inside() {
		// label 100..160, gap, row 80 wide ending a gap inside the right edge: 100 + 60 + 4 + 80 + 4 = 248
		assert!(!is_stacked(rect(100, 100, 248, 200), 60, 80, 4, 1920));
		assert!(is_stacked(rect(100, 100, 247, 200), 60, 80, 4, 1920));
	}

	#[test]
	fn lerp_goes_between_layouts() {
		let r = rect(100, 100, 300, 200);
		let beside = under_box(r, (60, 16), (80, 24), 4, (1920, 1080), false);
		let stacked = under_box(r, (60, 16), (80, 24), 4, (1920, 1080), true);
		assert_eq!(beside.lerp(&stacked, 0.0), beside);
		assert_eq!(beside.lerp(&stacked, 1.0), stacked);
		// buttons go from 221 to 100 (half of -121 rounds to -61), label from 209 to 205
		assert_eq!(beside.lerp(&stacked, 0.5).buttons.left, 160);
		assert_eq!(beside.lerp(&stacked, 0.5).label.top, 207);
	}

	#[test]
	fn they_stay_on_screen() {
		let r = rect(1500, 100, 1920, 200);
		assert_eq!(under(r).buttons.right, 1920);
		let r = rect(1900, 100, 1920, 200);
		assert_eq!(under(r).label.right, 1920);
		assert_eq!(under(r).buttons.right, 1920);
	}

	#[test]
	fn corner_highlight_is_an_l() {
		let r = Rect {
			left: 100,
			top: 100,
			right: 300,
			bottom: 200,
		};
		let grip = Grip {
			left: true,
			top: true,
			..Default::default()
		};
		assert_eq!(
			highlight(r, grip, 2, 12),
			vec![
				Rect {
					left: 98,
					top: 98,
					right: 100,
					bottom: 112
				},
				Rect {
					left: 98,
					top: 98,
					right: 112,
					bottom: 100
				},
			]
		);
	}

	#[test]
	fn corner_highlight_shrinks_on_small_box() {
		let r = Rect {
			left: 100,
			top: 100,
			right: 106,
			bottom: 104,
		};
		let grip = Grip {
			right: true,
			bottom: true,
			..Default::default()
		};
		assert_eq!(
			highlight(r, grip, 2, 12),
			vec![
				Rect {
					left: 106,
					top: 102,
					right: 108,
					bottom: 106
				},
				Rect {
					left: 103,
					top: 104,
					right: 108,
					bottom: 106
				},
			]
		);
	}

	#[test]
	fn edge_highlight_only_grows_outward() {
		let r = Rect {
			left: 100,
			top: 100,
			right: 300,
			bottom: 300,
		};
		let grip = Grip {
			top: true,
			..Default::default()
		};
		assert_eq!(
			highlight(r, grip, 2, 12),
			vec![Rect {
				left: 100,
				top: 98,
				right: 300,
				bottom: 100
			}]
		);
	}

	#[test]
	fn around_covers_everything_but_the_box() {
		let strips = around(
			Some(Rect {
				left: 10,
				top: 20,
				right: 30,
				bottom: 40,
			}),
			100,
			50,
		);
		let area: i32 = strips.iter().map(|r| r.width() * r.height()).sum();
		assert_eq!(area, 100 * 50 - 20 * 20);
		assert_eq!(
			around(None, 100, 50),
			vec![Rect {
				left: 0,
				top: 0,
				right: 100,
				bottom: 50
			}]
		);
		// a box touching the screen edges leaves no empty strips
		assert_eq!(
			around(
				Some(Rect {
					left: 0,
					top: 0,
					right: 100,
					bottom: 40
				}),
				100,
				50
			)
			.len(),
			1
		);
	}
}
