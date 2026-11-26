// anim

// Animation for the overlay: easing curves, tweens (a fixed start and end) and springs (a target that can move).

use std::time::{Duration, Instant};

// Fast start, slow finish. `t` and the result run from 0 to 1.
pub fn cubic_out(t: f32) -> f32 {
	1.0 - (1.0 - t).powi(3)
}

// A value that eases from one number to another over `duration`. Retargeting mid-way starts a fresh ease from
// wherever the value is, so it never jumps.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
	from: f32,
	to: f32,
	start: Instant,
	duration: Duration,
	ease: fn(f32) -> f32,
}

impl Tween {
	// Starts at rest at `value`.
	pub fn new(value: f32, duration: Duration, ease: fn(f32) -> f32) -> Self {
		Self {
			from: value,
			to: value,
			start: Instant::now(),
			duration,
			ease,
		}
	}

	pub fn value(&self, now: Instant) -> f32 {
		self.from + (self.to - self.from) * (self.ease)(self.progress(now))
	}

	// Where it's heading (or resting).
	pub fn target(&self) -> f32 {
		self.to
	}

	pub fn is_done(&self, now: Instant) -> bool {
		self.from == self.to || self.progress(now) >= 1.0
	}

	// Eases toward `to`, starting now. Returns whether that started a new ease (`false` if `to` was already the
	// target).
	pub fn retarget(&mut self, to: f32, now: Instant) -> bool {
		if to == self.to {
			return false;
		}
		self.from = self.value(now);
		self.to = to;
		self.start = now;
		true
	}

	fn progress(&self, now: Instant) -> f32 {
		if self.duration.is_zero() {
			return 1.0;
		}
		(now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
	}
}

// For a critically damped spring, `e^-x * (1 + x)` falls to 1% at `x` of about 6.6, so `omega = SETTLE_FACTOR / settle`
// makes it cover 99% of the way in `settle` seconds.
const SETTLE_FACTOR: f32 = 6.6;

// A critically damped spring: follows a target that can keep moving, as fast as possible without overshooting. Like a
// tween to a moving target.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
	pub value: f32,
	pub velocity: f32, // per second
}

impl Spring {
	pub fn new(value: f32) -> Self {
		Self { value, velocity: 0.0 }
	}

	// Advances `dt` seconds toward `target`, getting 99% of the way there in about `settle` seconds. Uses the exact
	// solution for a target that holds still during the step, so it stays stable at any frame time.
	pub fn step(&mut self, target: f32, dt: f32, settle: f32) {
		let omega = SETTLE_FACTOR / settle;
		let offset = self.value - target;
		let slope = self.velocity + omega * offset;
		let decay = (-omega * dt).exp();
		self.value = target + (offset + slope * dt) * decay;
		self.velocity = (slope - omega * (offset + slope * dt)) * decay;
	}

	// Jumps straight to `value` and stops.
	pub fn snap(&mut self, value: f32) {
		self.value = value;
		self.velocity = 0.0;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn linear(t: f32) -> f32 {
		t
	}

	#[test]
	fn cubic_out_curve() {
		assert_eq!(cubic_out(0.0), 0.0);
		assert_eq!(cubic_out(0.5), 0.875);
		assert_eq!(cubic_out(1.0), 1.0);
	}

	#[test]
	fn tween_eases_and_finishes() {
		let mut tween = Tween::new(1.0, Duration::from_millis(100), linear);
		let start = Instant::now();
		assert!(tween.is_done(start));
		assert!(tween.retarget(0.0, start));
		assert!(!tween.retarget(0.0, start));
		assert_eq!(tween.value(start + Duration::from_millis(25)), 0.75);
		assert!(!tween.is_done(start + Duration::from_millis(25)));
		assert_eq!(tween.value(start + Duration::from_millis(500)), 0.0);
		assert!(tween.is_done(start + Duration::from_millis(100)));
	}

	#[test]
	fn retarget_continues_from_current_value() {
		let mut tween = Tween::new(1.0, Duration::from_millis(100), linear);
		let start = Instant::now();
		tween.retarget(0.0, start);
		let halfway = start + Duration::from_millis(50);
		tween.retarget(1.0, halfway);
		assert_eq!(tween.value(halfway), 0.5);
		assert_eq!(tween.value(halfway + Duration::from_millis(50)), 0.75);
	}

	#[test]
	fn spring_settles_on_time_without_overshoot() {
		let mut spring = Spring::new(0.0);
		let mut previous = 0.0;
		// 100 steps of 1 ms over a 0.1 s settle time
		for _ in 0..100 {
			spring.step(100.0, 0.001, 0.1);
			assert!(spring.value >= previous && spring.value <= 100.0);
			previous = spring.value;
		}
		assert!(spring.value > 98.9, "only reached {}", spring.value);
	}

	#[test]
	fn spring_step_size_doesnt_matter() {
		let (mut fine, mut coarse) = (Spring::new(0.0), Spring::new(0.0));
		for _ in 0..50 {
			fine.step(10.0, 0.001, 0.1);
		}
		coarse.step(10.0, 0.05, 0.1);
		assert!((fine.value - coarse.value).abs() < 1e-3);
	}
}
