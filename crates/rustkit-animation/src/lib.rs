//! # RustKit Animation
//!
//! CSS Animations and Transitions for the RustKit browser engine.
//!
//! ## Features
//!
//! - **CSS Transitions**: Smooth property changes
//! - **CSS Animations**: Keyframe-based sequences
//! - **Timing Functions**: Easing curves (cubic-bezier, steps)
//! - **Property Interpolation**: Animate lengths, colors, transforms
//! - **Web Animations API**: JavaScript animation control
//!
//! ## Architecture
//!
//! ```text
//! AnimationTimeline
//!    └── Animation
//!           ├── Keyframes
//!           ├── Timing (duration, delay, iterations)
//!           └── Target Element
//!
//! TransitionManager
//!    └── Transition
//!           ├── Property
//!           ├── From/To Values
//!           └── Timing
//! ```

use rustkit_css::Color;
use rustkit_dom::NodeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, trace};

// ==================== Errors ====================

/// Errors that can occur in animations.
#[derive(Error, Debug)]
pub enum AnimationError {
    #[error("Invalid timing function: {0}")]
    InvalidTimingFunction(String),

    #[error("Invalid keyframe offset: {0}")]
    InvalidKeyframeOffset(f32),

    #[error("Animation not found: {0:?}")]
    AnimationNotFound(AnimationId),

    #[error("Property not animatable: {0}")]
    PropertyNotAnimatable(String),
}

// ==================== Identifiers ====================

/// Unique animation identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnimationId(u64);

impl AnimationId {
    /// Generate a new unique ID.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for AnimationId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique transition identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransitionId(u64);

impl TransitionId {
    /// Generate a new unique ID.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for TransitionId {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Timing Functions ====================

/// CSS timing function (easing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimingFunction {
    /// Linear interpolation.
    Linear,
    /// Default ease (0.25, 0.1, 0.25, 1.0).
    Ease,
    /// Ease in (0.42, 0, 1, 1).
    EaseIn,
    /// Ease out (0, 0, 0.58, 1).
    EaseOut,
    /// Ease in-out (0.42, 0, 0.58, 1).
    EaseInOut,
    /// Custom cubic bezier.
    CubicBezier(f64, f64, f64, f64),
    /// Step function.
    Steps(u32, StepPosition),
}

impl Default for TimingFunction {
    fn default() -> Self {
        TimingFunction::Ease
    }
}

impl TimingFunction {
    /// Evaluate the timing function at time t (0.0 to 1.0).
    pub fn evaluate(&self, t: f64) -> f64 {
        match self {
            TimingFunction::Linear => t,
            TimingFunction::Ease => cubic_bezier(0.25, 0.1, 0.25, 1.0, t),
            TimingFunction::EaseIn => cubic_bezier(0.42, 0.0, 1.0, 1.0, t),
            TimingFunction::EaseOut => cubic_bezier(0.0, 0.0, 0.58, 1.0, t),
            TimingFunction::EaseInOut => cubic_bezier(0.42, 0.0, 0.58, 1.0, t),
            TimingFunction::CubicBezier(x1, y1, x2, y2) => cubic_bezier(*x1, *y1, *x2, *y2, t),
            TimingFunction::Steps(steps, position) => {
                step_function(*steps, *position, t)
            }
        }
    }

    /// Parse from CSS string.
    pub fn parse(s: &str) -> Result<Self, AnimationError> {
        let s = s.trim().to_lowercase();

        match s.as_str() {
            "linear" => Ok(TimingFunction::Linear),
            "ease" => Ok(TimingFunction::Ease),
            "ease-in" => Ok(TimingFunction::EaseIn),
            "ease-out" => Ok(TimingFunction::EaseOut),
            "ease-in-out" => Ok(TimingFunction::EaseInOut),
            _ if s.starts_with("cubic-bezier(") => {
                let inner = s.trim_start_matches("cubic-bezier(").trim_end_matches(')');
                let parts: Vec<f64> = inner
                    .split(',')
                    .filter_map(|p| p.trim().parse().ok())
                    .collect();

                if parts.len() == 4 {
                    Ok(TimingFunction::CubicBezier(parts[0], parts[1], parts[2], parts[3]))
                } else {
                    Err(AnimationError::InvalidTimingFunction(s))
                }
            }
            _ if s.starts_with("steps(") => {
                let inner = s.trim_start_matches("steps(").trim_end_matches(')');
                let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();

                let steps: u32 = parts.first()
                    .and_then(|p| p.parse().ok())
                    .ok_or_else(|| AnimationError::InvalidTimingFunction(s.clone()))?;

                let position = parts.get(1)
                    .map(|p| StepPosition::parse(p))
                    .unwrap_or(StepPosition::End);

                Ok(TimingFunction::Steps(steps, position))
            }
            _ => Err(AnimationError::InvalidTimingFunction(s)),
        }
    }
}

/// Position for step timing function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepPosition {
    /// Jump at the start.
    Start,
    /// Jump at the end (default).
    #[default]
    End,
    /// Jump at both start and end.
    Both,
    /// Jump at neither (CSS jump-none).
    None,
}

impl StepPosition {
    fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "start" | "jump-start" => StepPosition::Start,
            "end" | "jump-end" => StepPosition::End,
            "both" | "jump-both" => StepPosition::Both,
            "none" | "jump-none" => StepPosition::None,
            _ => StepPosition::End,
        }
    }
}

/// Evaluate cubic bezier curve.
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    // Newton-Raphson iteration to find t for given x
    let epsilon = 1e-6;
    let mut guess = t;

    for _ in 0..8 {
        let x = bezier_value(x1, x2, guess) - t;
        if x.abs() < epsilon {
            break;
        }
        let dx = bezier_derivative(x1, x2, guess);
        if dx.abs() < epsilon {
            break;
        }
        guess -= x / dx;
    }

    bezier_value(y1, y2, guess)
}

/// Calculate bezier value at t.
fn bezier_value(p1: f64, p2: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    let mt = 1.0 - t;
    let mt2 = mt * mt;
    let _mt3 = mt2 * mt;

    // B(t) = 3*mt^2*t*P1 + 3*mt*t^2*P2 + t^3
    3.0 * mt2 * t * p1 + 3.0 * mt * t2 * p2 + t3
}

/// Calculate bezier derivative at t.
fn bezier_derivative(p1: f64, p2: f64, t: f64) -> f64 {
    let t2 = t * t;
    let mt = 1.0 - t;

    // B'(t) = 3*mt^2*P1 + 6*mt*t*(P2-P1) + 3*t^2*(1-P2)
    3.0 * mt * mt * p1 + 6.0 * mt * t * (p2 - p1) + 3.0 * t2 * (1.0 - p2)
}

/// Evaluate step function.
fn step_function(steps: u32, position: StepPosition, t: f64) -> f64 {
    let steps = steps as f64;

    match position {
        StepPosition::Start => (t * steps).ceil() / steps,
        StepPosition::End => (t * steps).floor() / steps,
        StepPosition::Both => {
            let adjusted = (t * (steps + 1.0)).floor() / steps;
            adjusted.min(1.0)
        }
        StepPosition::None => {
            if steps <= 1.0 {
                t
            } else {
                let adjusted = (t * (steps - 1.0)).floor() / (steps - 1.0);
                adjusted.max(0.0).min(1.0)
            }
        }
    }
}

// ==================== Animatable Values ====================

/// An animatable property value.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimatableValue {
    /// Length in pixels.
    Length(f32),
    /// Percentage.
    Percent(f32),
    /// Number.
    Number(f32),
    /// Color (RGBA).
    Color(Color),
    /// Opacity (0.0 - 1.0).
    Opacity(f32),
    /// Transform (as a string for now, could be matrix).
    Transform(String),
    /// Visibility (discrete).
    Visibility(bool),
    /// No value (for discrete animations).
    None,
}

impl AnimatableValue {
    /// Interpolate between two values.
    pub fn interpolate(&self, other: &AnimatableValue, progress: f64) -> AnimatableValue {
        match (self, other) {
            (AnimatableValue::Length(a), AnimatableValue::Length(b)) => {
                AnimatableValue::Length(lerp(*a, *b, progress as f32))
            }
            (AnimatableValue::Percent(a), AnimatableValue::Percent(b)) => {
                AnimatableValue::Percent(lerp(*a, *b, progress as f32))
            }
            (AnimatableValue::Number(a), AnimatableValue::Number(b)) => {
                AnimatableValue::Number(lerp(*a, *b, progress as f32))
            }
            (AnimatableValue::Opacity(a), AnimatableValue::Opacity(b)) => {
                AnimatableValue::Opacity(lerp(*a, *b, progress as f32).clamp(0.0, 1.0))
            }
            (AnimatableValue::Color(a), AnimatableValue::Color(b)) => {
                AnimatableValue::Color(interpolate_color(a, b, progress as f32))
            }
            (AnimatableValue::Visibility(a), AnimatableValue::Visibility(b)) => {
                // Discrete: switch at 50%
                if progress < 0.5 {
                    AnimatableValue::Visibility(*a)
                } else {
                    AnimatableValue::Visibility(*b)
                }
            }
            // Fallback: discrete switch
            _ => {
                if progress < 0.5 {
                    self.clone()
                } else {
                    other.clone()
                }
            }
        }
    }
}

/// Linear interpolation.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolate between two colors in sRGB space.
/// CSS color transitions use sRGB interpolation for browser compatibility.
fn interpolate_color(a: &Color, b: &Color, t: f32) -> Color {
    Color {
        r: (lerp(a.r as f32, b.r as f32, t).round() as u8).clamp(0, 255),
        g: (lerp(a.g as f32, b.g as f32, t).round() as u8).clamp(0, 255),
        b: (lerp(a.b as f32, b.b as f32, t).round() as u8).clamp(0, 255),
        a: lerp(a.a, b.a, t).clamp(0.0, 1.0),
    }
}

// ==================== CSS Properties ====================

/// Animatable CSS properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimatableProperty {
    // Box model
    Width,
    Height,
    MinWidth,
    MinHeight,
    MaxWidth,
    MaxHeight,
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
    BorderTopWidth,
    BorderRightWidth,
    BorderBottomWidth,
    BorderLeftWidth,

    // Positioning
    Top,
    Right,
    Bottom,
    Left,

    // Colors
    Color,
    BackgroundColor,
    BorderTopColor,
    BorderRightColor,
    BorderBottomColor,
    BorderLeftColor,

    // Typography
    FontSize,
    LineHeight,
    LetterSpacing,
    WordSpacing,

    // Visual
    Opacity,
    Visibility,

    // Transform
    Transform,

    // Flex/Grid
    FlexGrow,
    FlexShrink,
    Gap,
    RowGap,
    ColumnGap,
}

impl AnimatableProperty {
    /// Check if this property is animatable.
    pub fn is_animatable(&self) -> bool {
        true // All listed properties are animatable
    }

    /// Check if this property triggers layout when changed.
    pub fn triggers_layout(&self) -> bool {
        matches!(
            self,
            AnimatableProperty::Width
                | AnimatableProperty::Height
                | AnimatableProperty::MinWidth
                | AnimatableProperty::MinHeight
                | AnimatableProperty::MaxWidth
                | AnimatableProperty::MaxHeight
                | AnimatableProperty::MarginTop
                | AnimatableProperty::MarginRight
                | AnimatableProperty::MarginBottom
                | AnimatableProperty::MarginLeft
                | AnimatableProperty::PaddingTop
                | AnimatableProperty::PaddingRight
                | AnimatableProperty::PaddingBottom
                | AnimatableProperty::PaddingLeft
                | AnimatableProperty::BorderTopWidth
                | AnimatableProperty::BorderRightWidth
                | AnimatableProperty::BorderBottomWidth
                | AnimatableProperty::BorderLeftWidth
                | AnimatableProperty::FontSize
                | AnimatableProperty::LineHeight
                | AnimatableProperty::FlexGrow
                | AnimatableProperty::FlexShrink
                | AnimatableProperty::Gap
        )
    }

    /// Check if this property can be compositor-accelerated.
    pub fn is_compositor_only(&self) -> bool {
        matches!(
            self,
            AnimatableProperty::Opacity | AnimatableProperty::Transform
        )
    }

    /// Parse from CSS property name.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "width" => Some(AnimatableProperty::Width),
            "height" => Some(AnimatableProperty::Height),
            "min-width" => Some(AnimatableProperty::MinWidth),
            "min-height" => Some(AnimatableProperty::MinHeight),
            "max-width" => Some(AnimatableProperty::MaxWidth),
            "max-height" => Some(AnimatableProperty::MaxHeight),
            "margin-top" => Some(AnimatableProperty::MarginTop),
            "margin-right" => Some(AnimatableProperty::MarginRight),
            "margin-bottom" => Some(AnimatableProperty::MarginBottom),
            "margin-left" => Some(AnimatableProperty::MarginLeft),
            "padding-top" => Some(AnimatableProperty::PaddingTop),
            "padding-right" => Some(AnimatableProperty::PaddingRight),
            "padding-bottom" => Some(AnimatableProperty::PaddingBottom),
            "padding-left" => Some(AnimatableProperty::PaddingLeft),
            "border-top-width" => Some(AnimatableProperty::BorderTopWidth),
            "border-right-width" => Some(AnimatableProperty::BorderRightWidth),
            "border-bottom-width" => Some(AnimatableProperty::BorderBottomWidth),
            "border-left-width" => Some(AnimatableProperty::BorderLeftWidth),
            "top" => Some(AnimatableProperty::Top),
            "right" => Some(AnimatableProperty::Right),
            "bottom" => Some(AnimatableProperty::Bottom),
            "left" => Some(AnimatableProperty::Left),
            "color" => Some(AnimatableProperty::Color),
            "background-color" => Some(AnimatableProperty::BackgroundColor),
            "border-top-color" => Some(AnimatableProperty::BorderTopColor),
            "border-right-color" => Some(AnimatableProperty::BorderRightColor),
            "border-bottom-color" => Some(AnimatableProperty::BorderBottomColor),
            "border-left-color" => Some(AnimatableProperty::BorderLeftColor),
            "font-size" => Some(AnimatableProperty::FontSize),
            "line-height" => Some(AnimatableProperty::LineHeight),
            "letter-spacing" => Some(AnimatableProperty::LetterSpacing),
            "word-spacing" => Some(AnimatableProperty::WordSpacing),
            "opacity" => Some(AnimatableProperty::Opacity),
            "visibility" => Some(AnimatableProperty::Visibility),
            "transform" => Some(AnimatableProperty::Transform),
            "flex-grow" => Some(AnimatableProperty::FlexGrow),
            "flex-shrink" => Some(AnimatableProperty::FlexShrink),
            "gap" => Some(AnimatableProperty::Gap),
            "row-gap" => Some(AnimatableProperty::RowGap),
            "column-gap" => Some(AnimatableProperty::ColumnGap),
            _ => None,
        }
    }
}

// ==================== Keyframes ====================

/// A single keyframe in an animation.
#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Offset in the animation (0.0 to 1.0).
    pub offset: f32,
    /// Properties to animate at this keyframe.
    pub properties: HashMap<AnimatableProperty, AnimatableValue>,
    /// Timing function to next keyframe.
    pub easing: TimingFunction,
}

impl Keyframe {
    /// Create a new keyframe.
    pub fn new(offset: f32) -> Self {
        Self {
            offset: offset.clamp(0.0, 1.0),
            properties: HashMap::new(),
            easing: TimingFunction::default(),
        }
    }

    /// Add a property to this keyframe.
    pub fn with_property(mut self, property: AnimatableProperty, value: AnimatableValue) -> Self {
        self.properties.insert(property, value);
        self
    }

    /// Set the easing function.
    pub fn with_easing(mut self, easing: TimingFunction) -> Self {
        self.easing = easing;
        self
    }
}

/// A @keyframes rule.
#[derive(Debug, Clone)]
pub struct KeyframesRule {
    /// Name of the keyframes.
    pub name: String,
    /// Keyframes in order.
    pub keyframes: Vec<Keyframe>,
}

impl KeyframesRule {
    /// Create a new keyframes rule.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            keyframes: Vec::new(),
        }
    }

    /// Add a keyframe.
    pub fn add_keyframe(&mut self, keyframe: Keyframe) {
        self.keyframes.push(keyframe);
        self.keyframes.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap());
    }

    /// Get keyframes bracketing a given offset.
    pub fn get_bracketing_keyframes(&self, offset: f32) -> Option<(&Keyframe, &Keyframe, f32)> {
        if self.keyframes.is_empty() {
            return None;
        }

        // Find the keyframes that bracket the offset
        let mut prev = &self.keyframes[0];
        for keyframe in &self.keyframes {
            if keyframe.offset > offset {
                // Calculate local progress between prev and this keyframe
                let range = keyframe.offset - prev.offset;
                let local_progress = if range > 0.0 {
                    (offset - prev.offset) / range
                } else {
                    0.0
                };
                return Some((prev, keyframe, local_progress));
            }
            prev = keyframe;
        }

        // Past the last keyframe
        Some((prev, prev, 1.0))
    }
}

// ==================== Animation State ====================

/// Animation play state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationPlayState {
    /// Animation is not started.
    Idle,
    /// Animation is running.
    #[default]
    Running,
    /// Animation is paused.
    Paused,
    /// Animation has finished.
    Finished,
}

/// Animation direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationDirection {
    /// Play forward.
    #[default]
    Normal,
    /// Play backward.
    Reverse,
    /// Alternate forward and backward.
    Alternate,
    /// Alternate backward and forward.
    AlternateReverse,
}

/// Animation fill mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationFillMode {
    /// No fill - element reverts to original style.
    #[default]
    None,
    /// Apply final keyframe after animation ends.
    Forwards,
    /// Apply first keyframe before animation starts (during delay).
    Backwards,
    /// Apply both forwards and backwards.
    Both,
}

/// Animation timing configuration.
#[derive(Debug, Clone)]
pub struct AnimationTiming {
    /// Duration of one iteration.
    pub duration: Duration,
    /// Delay before starting.
    pub delay: Duration,
    /// Number of iterations (f64::INFINITY for infinite).
    pub iterations: f64,
    /// Direction of playback.
    pub direction: AnimationDirection,
    /// Fill mode.
    pub fill_mode: AnimationFillMode,
    /// Timing function.
    pub easing: TimingFunction,
}

impl Default for AnimationTiming {
    fn default() -> Self {
        Self {
            duration: Duration::from_millis(250),
            delay: Duration::ZERO,
            iterations: 1.0,
            direction: AnimationDirection::Normal,
            fill_mode: AnimationFillMode::None,
            easing: TimingFunction::Ease,
        }
    }
}

// ==================== Animation ====================

/// A CSS animation.
#[derive(Debug)]
pub struct Animation {
    /// Unique identifier.
    pub id: AnimationId,
    /// Target element.
    pub target: NodeId,
    /// Animation name (keyframes reference).
    pub name: String,
    /// Keyframes.
    pub keyframes: KeyframesRule,
    /// Timing configuration.
    pub timing: AnimationTiming,
    /// Current play state.
    pub play_state: AnimationPlayState,
    /// Start time.
    pub start_time: Option<Instant>,
    /// Pause time (for resuming).
    pub pause_time: Option<Instant>,
    /// Current iteration.
    pub current_iteration: u32,
    /// Current computed values.
    pub computed_values: HashMap<AnimatableProperty, AnimatableValue>,
}

impl Animation {
    /// Create a new animation.
    pub fn new(target: NodeId, name: &str, keyframes: KeyframesRule, timing: AnimationTiming) -> Self {
        Self {
            id: AnimationId::new(),
            target,
            name: name.to_string(),
            keyframes,
            timing,
            play_state: AnimationPlayState::Idle,
            start_time: None,
            pause_time: None,
            current_iteration: 0,
            computed_values: HashMap::new(),
        }
    }

    /// Start the animation.
    pub fn play(&mut self) {
        match self.play_state {
            AnimationPlayState::Idle => {
                self.start_time = Some(Instant::now());
                self.play_state = AnimationPlayState::Running;
            }
            AnimationPlayState::Paused => {
                // Resume from pause
                if let (Some(start), Some(pause)) = (self.start_time, self.pause_time) {
                    let paused_duration = pause.elapsed();
                    self.start_time = Some(start + paused_duration);
                    self.pause_time = None;
                }
                self.play_state = AnimationPlayState::Running;
            }
            _ => {}
        }
    }

    /// Pause the animation.
    pub fn pause(&mut self) {
        if self.play_state == AnimationPlayState::Running {
            self.pause_time = Some(Instant::now());
            self.play_state = AnimationPlayState::Paused;
        }
    }

    /// Cancel the animation.
    pub fn cancel(&mut self) {
        self.play_state = AnimationPlayState::Idle;
        self.start_time = None;
        self.pause_time = None;
        self.current_iteration = 0;
        self.computed_values.clear();
    }

    /// Finish the animation immediately.
    pub fn finish(&mut self) {
        self.play_state = AnimationPlayState::Finished;
    }

    /// Update the animation state.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.play_state != AnimationPlayState::Running {
            return false;
        }

        let Some(start) = self.start_time else {
            return false;
        };

        let elapsed = now.duration_since(start);

        // Check delay
        if elapsed < self.timing.delay {
            // Still in delay - apply backwards fill if needed
            if matches!(self.timing.fill_mode, AnimationFillMode::Backwards | AnimationFillMode::Both) {
                self.apply_keyframe(0.0);
            }
            return true;
        }

        let active_time = elapsed - self.timing.delay;
        let iteration_duration = self.timing.duration;

        if iteration_duration.is_zero() {
            self.play_state = AnimationPlayState::Finished;
            return false;
        }

        // Calculate current iteration and progress
        let iterations_elapsed = active_time.as_secs_f64() / iteration_duration.as_secs_f64();

        // Check if finished
        if iterations_elapsed >= self.timing.iterations {
            self.play_state = AnimationPlayState::Finished;

            // Apply forwards fill if needed
            if matches!(self.timing.fill_mode, AnimationFillMode::Forwards | AnimationFillMode::Both) {
                let final_offset = match self.timing.direction {
                    AnimationDirection::Normal => 1.0,
                    AnimationDirection::Reverse => 0.0,
                    AnimationDirection::Alternate => {
                        if self.timing.iterations.fract() == 0.0 && self.timing.iterations as u32 % 2 == 0 {
                            0.0
                        } else {
                            1.0
                        }
                    }
                    AnimationDirection::AlternateReverse => {
                        if self.timing.iterations.fract() == 0.0 && self.timing.iterations as u32 % 2 == 0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                self.apply_keyframe(final_offset);
            }
            return false;
        }

        let current_iteration = iterations_elapsed.floor() as u32;
        let iteration_progress = iterations_elapsed.fract() as f32;

        // Apply direction
        let offset = match self.timing.direction {
            AnimationDirection::Normal => iteration_progress,
            AnimationDirection::Reverse => 1.0 - iteration_progress,
            AnimationDirection::Alternate => {
                if current_iteration % 2 == 0 {
                    iteration_progress
                } else {
                    1.0 - iteration_progress
                }
            }
            AnimationDirection::AlternateReverse => {
                if current_iteration % 2 == 0 {
                    1.0 - iteration_progress
                } else {
                    iteration_progress
                }
            }
        };

        // Apply overall easing
        let eased_offset = self.timing.easing.evaluate(offset as f64) as f32;

        self.current_iteration = current_iteration;
        self.apply_keyframe(eased_offset);

        true
    }

    /// Apply keyframe values at the given offset.
    fn apply_keyframe(&mut self, offset: f32) {
        let Some((prev, next, local_progress)) = self.keyframes.get_bracketing_keyframes(offset) else {
            return;
        };

        // Apply easing from the previous keyframe
        let eased_progress = prev.easing.evaluate(local_progress as f64) as f32;

        // Interpolate all properties
        for (property, prev_value) in &prev.properties {
            if let Some(next_value) = next.properties.get(property) {
                let interpolated = prev_value.interpolate(next_value, eased_progress as f64);
                self.computed_values.insert(*property, interpolated);
            } else {
                // No next value, use prev
                self.computed_values.insert(*property, prev_value.clone());
            }
        }
    }

    /// Get the current computed value for a property.
    pub fn get_value(&self, property: AnimatableProperty) -> Option<&AnimatableValue> {
        self.computed_values.get(&property)
    }
}

// ==================== Transition ====================

/// A CSS transition.
#[derive(Debug)]
pub struct Transition {
    /// Unique identifier.
    pub id: TransitionId,
    /// Target element.
    pub target: NodeId,
    /// Property being transitioned.
    pub property: AnimatableProperty,
    /// Starting value.
    pub from: AnimatableValue,
    /// Ending value.
    pub to: AnimatableValue,
    /// Duration.
    pub duration: Duration,
    /// Delay.
    pub delay: Duration,
    /// Timing function.
    pub easing: TimingFunction,
    /// Start time.
    pub start_time: Option<Instant>,
    /// Current state.
    pub state: TransitionState,
    /// Current computed value.
    pub current_value: AnimatableValue,
}

/// Transition state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionState {
    /// Waiting for delay.
    Pending,
    /// Actively transitioning.
    Running,
    /// Completed.
    Completed,
    /// Cancelled.
    Cancelled,
}

impl Transition {
    /// Create a new transition.
    pub fn new(
        target: NodeId,
        property: AnimatableProperty,
        from: AnimatableValue,
        to: AnimatableValue,
        duration: Duration,
        delay: Duration,
        easing: TimingFunction,
    ) -> Self {
        Self {
            id: TransitionId::new(),
            target,
            property,
            from: from.clone(),
            to,
            duration,
            delay,
            easing,
            start_time: Some(Instant::now()),
            state: TransitionState::Pending,
            current_value: from,
        }
    }

    /// Update the transition.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(start) = self.start_time else {
            return false;
        };

        let elapsed = now.duration_since(start);

        // Check delay
        if elapsed < self.delay {
            self.current_value = self.from.clone();
            return true;
        }

        if self.state == TransitionState::Pending {
            self.state = TransitionState::Running;
        }

        let active_time = elapsed - self.delay;

        // Check if complete
        if active_time >= self.duration {
            self.state = TransitionState::Completed;
            self.current_value = self.to.clone();
            return false;
        }

        // Calculate progress
        let progress = if self.duration.is_zero() {
            1.0
        } else {
            active_time.as_secs_f64() / self.duration.as_secs_f64()
        };

        let eased_progress = self.easing.evaluate(progress);
        self.current_value = self.from.interpolate(&self.to, eased_progress);

        true
    }

    /// Cancel the transition.
    pub fn cancel(&mut self) {
        self.state = TransitionState::Cancelled;
    }
}

// ==================== Animation Timeline ====================

/// Animation timeline manager.
#[derive(Debug, Default)]
pub struct AnimationTimeline {
    /// All active animations.
    animations: HashMap<AnimationId, Animation>,
    /// All active transitions.
    transitions: HashMap<TransitionId, Transition>,
    /// Registered @keyframes rules.
    keyframes_registry: HashMap<String, KeyframesRule>,
    /// Animation events to dispatch.
    pending_events: Vec<AnimationEvent>,
}

/// Animation event.
#[derive(Debug, Clone)]
pub struct AnimationEvent {
    /// Event type.
    pub event_type: AnimationEventType,
    /// Target element.
    pub target: NodeId,
    /// Animation name (for animation events).
    pub animation_name: Option<String>,
    /// Property name (for transition events).
    pub property_name: Option<String>,
    /// Elapsed time in seconds.
    pub elapsed_time: f64,
    /// Pseudo-element.
    pub pseudo_element: String,
}

/// Animation event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationEventType {
    AnimationStart,
    AnimationEnd,
    AnimationIteration,
    AnimationCancel,
    TransitionStart,
    TransitionEnd,
    TransitionCancel,
    TransitionRun,
}

impl AnimationTimeline {
    /// Create a new animation timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a @keyframes rule.
    pub fn register_keyframes(&mut self, rule: KeyframesRule) {
        self.keyframes_registry.insert(rule.name.clone(), rule);
    }

    /// Get a registered keyframes rule.
    pub fn get_keyframes(&self, name: &str) -> Option<&KeyframesRule> {
        self.keyframes_registry.get(name)
    }

    /// Create and start an animation.
    pub fn animate(
        &mut self,
        target: NodeId,
        name: &str,
        timing: AnimationTiming,
    ) -> Option<AnimationId> {
        let keyframes = self.keyframes_registry.get(name)?.clone();
        let mut animation = Animation::new(target, name, keyframes, timing);
        animation.play();

        let id = animation.id;
        self.animations.insert(id, animation);

        self.pending_events.push(AnimationEvent {
            event_type: AnimationEventType::AnimationStart,
            target,
            animation_name: Some(name.to_string()),
            property_name: None,
            elapsed_time: 0.0,
            pseudo_element: String::new(),
        });

        debug!("Started animation '{}' for {:?}", name, target);
        Some(id)
    }

    /// Create and start a transition.
    pub fn transition(
        &mut self,
        target: NodeId,
        property: AnimatableProperty,
        from: AnimatableValue,
        to: AnimatableValue,
        duration: Duration,
        delay: Duration,
        easing: TimingFunction,
    ) -> TransitionId {
        let transition = Transition::new(target, property, from, to, duration, delay, easing);
        let id = transition.id;

        self.transitions.insert(id, transition);

        trace!("Started transition {:?} for {:?}", property, target);
        id
    }

    /// Update all animations and transitions.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut any_running = false;

        // Update animations
        let mut finished_animations = Vec::new();
        for (id, animation) in &mut self.animations {
            let was_running = animation.play_state == AnimationPlayState::Running;
            let is_running = animation.tick(now);

            if was_running && animation.play_state == AnimationPlayState::Finished {
                let elapsed = if let Some(start) = animation.start_time {
                    (now - start).as_secs_f64()
                } else {
                    animation.timing.duration.as_secs_f64()
                };
                finished_animations.push((*id, animation.name.clone(), animation.target, elapsed));
            }

            any_running |= is_running;
        }

        // Emit animation end events
        for (_id, name, target, elapsed_time) in finished_animations {
            self.pending_events.push(AnimationEvent {
                event_type: AnimationEventType::AnimationEnd,
                target,
                animation_name: Some(name),
                property_name: None,
                elapsed_time,
                pseudo_element: String::new(),
            });
        }

        // Update transitions
        let mut finished_transitions = Vec::new();
        for (id, transition) in &mut self.transitions {
            let was_running = transition.state == TransitionState::Running;
            let is_running = transition.tick(now);

            if was_running && transition.state == TransitionState::Completed {
                let elapsed = if let Some(start) = transition.start_time {
                    (now - start).as_secs_f64()
                } else {
                    transition.duration.as_secs_f64()
                };
                finished_transitions.push((*id, transition.property, transition.target, elapsed));
            }

            any_running |= is_running;
        }

        // Emit transition end events
        for (_id, property, target, elapsed_time) in finished_transitions {
            self.pending_events.push(AnimationEvent {
                event_type: AnimationEventType::TransitionEnd,
                target,
                animation_name: None,
                property_name: Some(format!("{:?}", property).to_lowercase()),
                elapsed_time,
                pseudo_element: String::new(),
            });
        }

        // Clean up finished animations and transitions
        self.animations.retain(|_, a| a.play_state != AnimationPlayState::Finished);
        self.transitions.retain(|_, t| t.state != TransitionState::Completed && t.state != TransitionState::Cancelled);

        any_running
    }

    /// Get pending events and clear the queue.
    pub fn take_events(&mut self) -> Vec<AnimationEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Get an animation by ID.
    pub fn get_animation(&self, id: AnimationId) -> Option<&Animation> {
        self.animations.get(&id)
    }

    /// Get a transition by ID.
    pub fn get_transition(&self, id: TransitionId) -> Option<&Transition> {
        self.transitions.get(&id)
    }

    /// Get all animations for an element.
    pub fn get_element_animations(&self, target: NodeId) -> Vec<&Animation> {
        self.animations.values().filter(|a| a.target == target).collect()
    }

    /// Get all transitions for an element.
    pub fn get_element_transitions(&self, target: NodeId) -> Vec<&Transition> {
        self.transitions.values().filter(|t| t.target == target).collect()
    }

    /// Get computed values for all running animations/transitions on an element.
    pub fn get_computed_values(&self, target: NodeId) -> HashMap<AnimatableProperty, AnimatableValue> {
        let mut values = HashMap::new();

        // Animations take precedence over transitions
        for animation in self.get_element_animations(target) {
            for (prop, value) in &animation.computed_values {
                values.insert(*prop, value.clone());
            }
        }

        for transition in self.get_element_transitions(target) {
            values.entry(transition.property).or_insert_with(|| transition.current_value.clone());
        }

        values
    }

    /// Pause an animation.
    pub fn pause_animation(&mut self, id: AnimationId) {
        if let Some(animation) = self.animations.get_mut(&id) {
            animation.pause();
        }
    }

    /// Play an animation.
    pub fn play_animation(&mut self, id: AnimationId) {
        if let Some(animation) = self.animations.get_mut(&id) {
            animation.play();
        }
    }

    /// Cancel an animation.
    pub fn cancel_animation(&mut self, id: AnimationId) {
        if let Some(animation) = self.animations.remove(&id) {
            self.pending_events.push(AnimationEvent {
                event_type: AnimationEventType::AnimationCancel,
                target: animation.target,
                animation_name: Some(animation.name),
                property_name: None,
                elapsed_time: 0.0,
                pseudo_element: String::new(),
            });
        }
    }

    /// Cancel a transition.
    pub fn cancel_transition(&mut self, id: TransitionId) {
        if let Some(transition) = self.transitions.get_mut(&id) {
            transition.cancel();
            self.pending_events.push(AnimationEvent {
                event_type: AnimationEventType::TransitionCancel,
                target: transition.target,
                animation_name: None,
                property_name: Some(format!("{:?}", transition.property).to_lowercase()),
                elapsed_time: 0.0,
                pseudo_element: String::new(),
            });
        }
    }

    /// Get number of active animations.
    pub fn animation_count(&self) -> usize {
        self.animations.len()
    }

    /// Get number of active transitions.
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }
}

// ==================== CSS Transition Definition ====================

/// CSS transition definition (from style).
#[derive(Debug, Clone, Default)]
pub struct TransitionDefinition {
    /// Properties to transition ('all' means all animatable properties).
    pub properties: Vec<String>,
    /// Duration.
    pub duration: Duration,
    /// Delay.
    pub delay: Duration,
    /// Timing function.
    pub timing_function: TimingFunction,
}

/// CSS animation definition (from style).
#[derive(Debug, Clone, Default)]
pub struct AnimationDefinition {
    /// Animation name (keyframes reference).
    pub name: String,
    /// Duration.
    pub duration: Duration,
    /// Delay.
    pub delay: Duration,
    /// Timing function.
    pub timing_function: TimingFunction,
    /// Iteration count.
    pub iteration_count: f64,
    /// Direction.
    pub direction: AnimationDirection,
    /// Fill mode.
    pub fill_mode: AnimationFillMode,
    /// Play state.
    pub play_state: AnimationPlayState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timing_function_linear() {
        let tf = TimingFunction::Linear;
        assert_eq!(tf.evaluate(0.0), 0.0);
        assert_eq!(tf.evaluate(0.5), 0.5);
        assert_eq!(tf.evaluate(1.0), 1.0);
    }

    #[test]
    fn test_timing_function_ease() {
        let tf = TimingFunction::Ease;
        assert_eq!(tf.evaluate(0.0), 0.0);
        // Ease should be slower at start and end
        assert!(tf.evaluate(0.5) > 0.5);
        assert_eq!(tf.evaluate(1.0), 1.0);
    }

    #[test]
    fn test_timing_function_steps() {
        let tf = TimingFunction::Steps(4, StepPosition::End);
        assert_eq!(tf.evaluate(0.0), 0.0);
        assert_eq!(tf.evaluate(0.24), 0.0);
        assert_eq!(tf.evaluate(0.26), 0.25);
        assert_eq!(tf.evaluate(0.5), 0.5);
    }

    #[test]
    fn test_timing_function_parse() {
        assert!(matches!(TimingFunction::parse("linear"), Ok(TimingFunction::Linear)));
        assert!(matches!(TimingFunction::parse("ease"), Ok(TimingFunction::Ease)));
        assert!(matches!(TimingFunction::parse("ease-in"), Ok(TimingFunction::EaseIn)));
        assert!(matches!(TimingFunction::parse("ease-out"), Ok(TimingFunction::EaseOut)));
        assert!(matches!(TimingFunction::parse("ease-in-out"), Ok(TimingFunction::EaseInOut)));

        if let Ok(TimingFunction::CubicBezier(x1, y1, x2, y2)) = TimingFunction::parse("cubic-bezier(0.1, 0.2, 0.3, 0.4)") {
            assert!((x1 - 0.1).abs() < 0.001);
            assert!((y1 - 0.2).abs() < 0.001);
            assert!((x2 - 0.3).abs() < 0.001);
            assert!((y2 - 0.4).abs() < 0.001);
        } else {
            panic!("Failed to parse cubic-bezier");
        }
    }

    #[test]
    fn test_animatable_value_interpolate() {
        let a = AnimatableValue::Length(0.0);
        let b = AnimatableValue::Length(100.0);

        if let AnimatableValue::Length(v) = a.interpolate(&b, 0.0) {
            assert_eq!(v, 0.0);
        }
        if let AnimatableValue::Length(v) = a.interpolate(&b, 0.5) {
            assert_eq!(v, 50.0);
        }
        if let AnimatableValue::Length(v) = a.interpolate(&b, 1.0) {
            assert_eq!(v, 100.0);
        }
    }

    #[test]
    fn test_color_interpolation() {
        let a = AnimatableValue::Color(Color::new(0, 0, 0, 1.0));
        let b = AnimatableValue::Color(Color::new(255, 255, 255, 1.0));

        if let AnimatableValue::Color(c) = a.interpolate(&b, 0.5) {
            assert_eq!(c.r, 128);
            assert_eq!(c.g, 128);
            assert_eq!(c.b, 128);
        }
    }

    #[test]
    fn test_keyframe_bracketing() {
        let mut rule = KeyframesRule::new("test");
        rule.add_keyframe(Keyframe::new(0.0).with_property(AnimatableProperty::Opacity, AnimatableValue::Opacity(0.0)));
        rule.add_keyframe(Keyframe::new(0.5).with_property(AnimatableProperty::Opacity, AnimatableValue::Opacity(1.0)));
        rule.add_keyframe(Keyframe::new(1.0).with_property(AnimatableProperty::Opacity, AnimatableValue::Opacity(0.5)));

        let (prev, next, progress) = rule.get_bracketing_keyframes(0.25).unwrap();
        assert_eq!(prev.offset, 0.0);
        assert_eq!(next.offset, 0.5);
        assert_eq!(progress, 0.5);
    }

    #[test]
    fn test_animation_timeline() {
        let mut timeline = AnimationTimeline::new();

        // Register keyframes
        let mut rule = KeyframesRule::new("fade");
        rule.add_keyframe(Keyframe::new(0.0).with_property(AnimatableProperty::Opacity, AnimatableValue::Opacity(0.0)));
        rule.add_keyframe(Keyframe::new(1.0).with_property(AnimatableProperty::Opacity, AnimatableValue::Opacity(1.0)));
        timeline.register_keyframes(rule);

        // Start animation
        let target = NodeId::new(1);
        let timing = AnimationTiming {
            duration: Duration::from_millis(100),
            ..Default::default()
        };

        let _id = timeline.animate(target, "fade", timing).unwrap();
        assert_eq!(timeline.animation_count(), 1);

        // Tick should be running
        assert!(timeline.tick());
    }

    #[test]
    fn test_transition() {
        let target = NodeId::new(1);
        let mut transition = Transition::new(
            target,
            AnimatableProperty::Opacity,
            AnimatableValue::Opacity(0.0),
            AnimatableValue::Opacity(1.0),
            Duration::from_millis(100),
            Duration::ZERO,
            TimingFunction::Linear,
        );

        // Initially pending
        assert_eq!(transition.state, TransitionState::Pending);

        // After tick, should be running
        let now = Instant::now();
        transition.tick(now);
        assert_eq!(transition.state, TransitionState::Running);
    }

    #[test]
    fn test_animatable_property_parse() {
        assert!(matches!(AnimatableProperty::parse("opacity"), Some(AnimatableProperty::Opacity)));
        assert!(matches!(AnimatableProperty::parse("width"), Some(AnimatableProperty::Width)));
        assert!(matches!(AnimatableProperty::parse("background-color"), Some(AnimatableProperty::BackgroundColor)));
        assert!(AnimatableProperty::parse("invalid").is_none());
    }

    #[test]
    fn test_animatable_property_compositor() {
        assert!(AnimatableProperty::Opacity.is_compositor_only());
        assert!(AnimatableProperty::Transform.is_compositor_only());
        assert!(!AnimatableProperty::Width.is_compositor_only());
    }

    #[test]
    fn test_animatable_property_layout() {
        assert!(AnimatableProperty::Width.triggers_layout());
        assert!(AnimatableProperty::MarginTop.triggers_layout());
        assert!(!AnimatableProperty::Opacity.triggers_layout());
        assert!(!AnimatableProperty::Color.triggers_layout());
    }
}

