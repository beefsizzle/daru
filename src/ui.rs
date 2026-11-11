use std::collections::HashMap;
use std::time::Duration;

use amane::{
    Animation, Button, Center, Color, Column, Image, Point, Pointer, Rectangle, Row, Service, Stack, Start, Text, Weight,
    Widget, children,
};

use crate::theme::{CHIP_RADIUS, ICON_FONT, RADIUS_SMALL, TEXT_FONT, Theme, alpha, mix};

// ---------------------------------------------------------------- text

pub fn icon(glyph: &str, size: f32, color: Color) -> Text {
    Text::new(glyph).font(ICON_FONT).size(size).color(color).tight()
}

pub fn label(content: impl Into<String>, size: f32, color: Color) -> Text {
    Text::new(content).font(TEXT_FONT).size(size).color(color)
}

pub fn bold(content: impl Into<String>, size: f32, color: Color) -> Text {
    label(content, size, color).weight(Weight::Bold)
}

// ---------------------------------------------------------------- pictures

// the pixels app icons are decoded at: big enough for the largest place one is drawn (44px in the launcher grid) on
// a monitor scaled up to 2x. one size everywhere, so an icon shown in several places is decoded once
const ICON_PIXELS: u32 = 96;

/*
 * an app or theme icon, shrunk to ICON_PIXELS once it's decoded. icon files are often 256px or bigger, a quarter of
 * a megabyte each once decoded, and the launcher shows dozens. Amane keeps small copies like these for good (they
 * cost little), so only use it for icons from the installed themes, not for pictures that keep changing
 */
pub fn app_icon(path: impl AsRef<std::path::Path>) -> Image {
    Image::contain(path.as_ref()).thumbnail(ICON_PIXELS, ICON_PIXELS)
}

// ---------------------------------------------------------------- hover

/*
 * one eased 0..1 value per hover id, so every chip fades in and out
 * instead of snapping. a view reads `hover(id)`, a handler calls `set_hover`
 */
struct Hover {
    items: HashMap<String, Animation>,
}

impl Service for Hover {
    fn new() -> Self {
        Self { items: HashMap::new() }
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }
}

/*
 * the bar is built once per monitor from the same code, so its chips would share one hover id and light up on every
 * monitor at once. while a bar is being built its monitor's name is the scope, and `chip` puts it in front of its id
 */
thread_local! {
    static SCOPE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub struct Scope;

impl Scope {
    pub fn enter(name: &str) -> Self {
        SCOPE.with(|scope| *scope.borrow_mut() = name.to_string());

        Scope
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        SCOPE.with(|scope| scope.borrow_mut().clear());
    }
}

fn scoped(id: &str) -> String {
    SCOPE.with(|scope| {
        let scope = scope.borrow();

        if scope.is_empty() { id.to_string() } else { format!("{scope}:{id}") }
    })
}

pub fn hover(id: &str) -> f32 {
    Hover::read().items.get(id).map_or(0.0, Animation::value)
}

pub fn set_hover(id: &str, inside: bool) {
    let mut state = Hover::write();

    let animation = state
        .items
        .entry(id.to_string())
        .or_insert_with(|| Animation::new(0.0).duration(Duration::from_millis(140)));

    animation.to(if inside { 1.0 } else { 0.0 });
}

// ---------------------------------------------------------------- chips

/// a pill that lights up under the cursor. a rectangle needs a width, so the caller picks one.
pub fn chip(id: &str, theme: &Theme, width: f32, content: impl Widget + 'static) -> Rectangle {
    let id = scoped(id);

    let t = hover(&id);

    let fill = mix(alpha(theme.surface, 0.45), theme.surface_hi, t);

    Rectangle::new()
        .width(width)
        .height(26.0)
        .radius(CHIP_RADIUS)
        .fill(fill)
        .align_child(Center, Center)
        .cursor(Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        .child(content)
}

// ---------------------------------------------------------------- controls

/// a horizontal slider. `value` is 0..1, `changed` gets the new 0..1 while dragging, and `scrolled` gets whole wheel
/// notches while the pointer is over it, up positive
pub fn slider(t: &Theme, width: f32, value: f32, changed: impl Fn(f32) + 'static, scrolled: impl Fn(i32) + 'static) -> Rectangle {
    let value = value.clamp(0.0, 1.0);

    let layer = |child: Rectangle| {
        Rectangle::new().width(width).height(18.0).align_child(Start, Center).child(child)
    };

    let track = Rectangle::new().width(width).height(6.0).radius(0.0).fill(t.surface_hi);
    let filled = Rectangle::new().width((width * value).max(6.0)).height(6.0).radius(0.0).fill(t.accent);

    let knob = Rectangle::new()
        .width(14.0)
        .height(14.0)
        .radius(0.0)
        // white, but fading with the panel it's in
        .fill(Color::rgba(255, 255, 255, t.fg.alpha()))
        .border(1.0, alpha(Color::BLACK, 0.45 * f32::from(t.fg.alpha()) / 255.0))
        .translate((width - 14.0) * value, 2.0);

    Rectangle::new()
        .width(width)
        .height(18.0)
        .cursor(Pointer)
        .on_drag(move |point: Point| changed((point.x / width).clamp(0.0, 1.0)))
        // up (negative) raises it
        .on_scroll(move |scroll| scrolled(wheel_steps(-scroll.y)))
        .child(Stack::new(children![layer(track), layer(filled), knob]))
}

/*
 * recent numbers as thin bars, the newest on the right and brightest. the scale is the biggest of them, but at least
 * `floor`, so a quiet line stays flat instead of blowing tiny numbers up to full height
 */
pub fn sparkline(t: &Theme, width: f32, height: f32, data: &[f32], color: Color, floor: f32) -> Row {
    let top = data.iter().copied().fold(floor, f32::max);

    let count = data.len().max(2);
    let bar = 3.0;
    let gap = ((width - bar * count as f32) / (count - 1) as f32).max(0.0);

    let mut bars: Vec<Box<dyn Widget>> = Vec::new();

    for (index, value) in data.iter().enumerate() {
        let newest = index == data.len() - 1;

        let fill = Rectangle::new()
            .width(bar)
            .height((height * value / top).max(1.0))
            .fill(if newest { color } else { alpha(color, 0.55) });

        bars.push(Box::new(Rectangle::new().width(bar).height(height).fill(alpha(t.fg, 0.04)).align_child(Center, amane::End).child(fill)));
    }

    Row::new(bars).gap(gap)
}

/// a vertical meter made of small blocks, lit from the bottom. blocks are plain rectangles,
/// which Amane draws cheaply, unlike canvas shapes (the first one starts a much heavier renderer)
pub fn meter(t: &Theme, width: f32, value: f32, color: Color) -> Column {
    const BLOCKS: usize = 12;
    const BLOCK: f32 = 3.5;

    let lit = (value.clamp(0.0, 1.0) * BLOCKS as f32).round() as usize;

    let mut blocks: Vec<Box<dyn Widget>> = Vec::new();

    // top to bottom, so the unlit ones come first
    for index in 0..BLOCKS {
        let on = BLOCKS - index <= lit;

        blocks.push(Box::new(Rectangle::new().width(width).height(BLOCK).fill(if on { color } else { t.surface_hi })));
    }

    Column::new(blocks).gap(1.0)
}

/// a small round icon button
pub fn round_button(id: &str, t: &Theme, glyph: &str, color: Color, on_click: impl Fn(Button) + 'static) -> Rectangle {
    let hot = hover(id);

    let id = id.to_string();

    Rectangle::new()
        .width(30.0)
        .height(30.0)
        .radius(RADIUS_SMALL)
        .fill(mix(alpha(t.surface, 0.0), t.surface_hi, hot))
        .align_child(Center, Center)
        .cursor(Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        .on_click(on_click)
        .child(icon(glyph, 13.0, color))
}

pub fn section(text: &str, t: &Theme) -> Text {
    label(text.to_uppercase(), 10.5, t.faint).weight(Weight::Bold)
}

/// "now", "5m", "3h", "2d"
pub fn ago(time: std::time::SystemTime) -> String {
    let seconds = std::time::SystemTime::now().duration_since(time).map_or(0, |duration| duration.as_secs());

    match seconds {
        0..=59 => String::from("now"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// turns wheel movement into whole notches. a mouse wheel sends 1.0 per notch, a touchpad sends many small amounts,
/// so what hasn't added up to a notch yet is kept for the next event. one per scrolling thing
#[derive(Default)]
pub struct Wheel {
    pending: f32,
}

impl Wheel {
    pub const fn new() -> Self {
        Self { pending: 0.0 }
    }

    /// the whole notches in `amount` and what was left over before. negative is up (or left)
    pub fn steps(&mut self, amount: f32) -> i32 {
        self.pending += amount;

        let steps = self.pending.trunc();

        self.pending -= steps;

        steps as i32
    }
}

/// for the volume chip and the sliders, which have no service of their own to keep a `Wheel` in (only one of them is
/// under the pointer at a time)
pub fn wheel_steps(amount: f32) -> i32 {
    static WHEEL: std::sync::Mutex<Wheel> = std::sync::Mutex::new(Wheel::new());

    crate::sync::lock(&WHEEL).steps(amount)
}

#[cfg(test)]
mod tests {
    use super::Wheel;

    #[test]
    fn a_mouse_notch_is_one_step() {
        let mut wheel = Wheel::new();

        assert_eq!(wheel.steps(1.0), 1);
        assert_eq!(wheel.steps(-1.0), -1);
        assert_eq!(wheel.steps(3.0), 3);
    }

    #[test]
    fn small_touchpad_amounts_add_up() {
        let mut wheel = Wheel::new();

        let steps: Vec<i32> = (0..10).map(|_| wheel.steps(0.25)).collect();

        assert_eq!(steps, [0, 0, 0, 1, 0, 0, 0, 1, 0, 0]);
    }

    #[test]
    fn turning_back_cancels_what_was_pending() {
        let mut wheel = Wheel::new();

        assert_eq!(wheel.steps(0.75), 0);
        assert_eq!(wheel.steps(-0.5), 0);
        assert_eq!(wheel.steps(0.75), 1);
        assert_eq!(wheel.steps(-1.75), -1);
    }
}
