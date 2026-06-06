//! The keypad widget — the calculator's display and its grid of buttons.
//!
//! It's a single custom [`Widget`] rather than a tree of saudade `Button`s, for
//! two reasons: the Win 3.1 calculator colours its keys (blue digits, maroon
//! functions) which the stock button doesn't, and routing *every* keystroke to
//! one focused widget makes the keyboard map trivial — there's no per-button
//! focus to chase. The widget owns the shared [`Engine`] and translates pointer
//! presses and key presses into engine calls.

use std::cell::RefCell;
use std::rc::Rc;

use saudade::{
    Color, Event, EventCtx, Key, MouseButton, NamedKey, Painter, Point, Rect, Theme, Widget,
};

use crate::clipboard;
use crate::engine::{Engine, Op};

// --- palette ---------------------------------------------------------------

/// Digit / decimal / sign keys — Win 3.1 painted these dark blue.
const BLUE: Color = Color::NAVY;
/// Operator, function and memory keys — painted maroon.
const MAROON: Color = Color::rgb(0x80, 0x00, 0x00);

// --- metrics (logical pixels) ----------------------------------------------

const MARGIN: i32 = 10;
const BTN_W: i32 = 36;
const BTN_H: i32 = 28;
const GAP: i32 = 6;
const DISPLAY_H: i32 = 32;
const FUNC_H: i32 = 28;
const SECTION_GAP: i32 = 10;
/// Point size for the big readout.
const DISPLAY_FONT: f32 = 20.0;

/// Width of the main 6-column grid (and so the display above it).
const GRID_W: i32 = 6 * BTN_W + 5 * GAP;

/// Natural window-content width: the grid plus side margins.
pub const NATURAL_WIDTH: i32 = 2 * MARGIN + GRID_W;
/// Natural keypad height: margins, the display, the Back/CE/C strip, and the
/// four button rows.
pub const NATURAL_HEIGHT: i32 =
    MARGIN + DISPLAY_H + SECTION_GAP + FUNC_H + SECTION_GAP + (4 * BTN_H + 3 * GAP) + MARGIN;

// --- buttons ---------------------------------------------------------------

/// Every key on the pad. Named `Btn` to avoid colliding with saudade's `Key`
/// (the keyboard-event enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Btn {
    Digit(u8),
    Dot,
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Sqrt,
    Percent,
    Recip,
    Back,
    ClearEntry,
    ClearAll,
    MemClear,
    MemRecall,
    MemStore,
    MemAdd,
}

impl Btn {
    /// The face label.
    fn label(self) -> &'static str {
        match self {
            Btn::Digit(0) => "0",
            Btn::Digit(1) => "1",
            Btn::Digit(2) => "2",
            Btn::Digit(3) => "3",
            Btn::Digit(4) => "4",
            Btn::Digit(5) => "5",
            Btn::Digit(6) => "6",
            Btn::Digit(7) => "7",
            Btn::Digit(8) => "8",
            Btn::Digit(9) => "9",
            Btn::Digit(_) => "?",
            Btn::Dot => ".",
            Btn::Neg => "+/-",
            Btn::Add => "+",
            Btn::Sub => "-",
            Btn::Mul => "*",
            Btn::Div => "/",
            Btn::Eq => "=",
            Btn::Sqrt => "sqrt",
            Btn::Percent => "%",
            Btn::Recip => "1/x",
            Btn::Back => "Back",
            Btn::ClearEntry => "CE",
            Btn::ClearAll => "C",
            Btn::MemClear => "MC",
            Btn::MemRecall => "MR",
            Btn::MemStore => "MS",
            Btn::MemAdd => "M+",
        }
    }

    /// Digit / dot / sign keys read blue; everything else reads maroon.
    fn color(self) -> Color {
        match self {
            Btn::Digit(_) | Btn::Dot | Btn::Neg => BLUE,
            _ => MAROON,
        }
    }
}

/// The 4×6 main grid, laid out exactly like the Win 3.1 standard view: the
/// memory column on the left, the digit pad in the middle, operators and
/// functions on the right.
const GRID: [[Btn; 6]; 4] = [
    [
        Btn::MemClear,
        Btn::Digit(7),
        Btn::Digit(8),
        Btn::Digit(9),
        Btn::Div,
        Btn::Sqrt,
    ],
    [
        Btn::MemRecall,
        Btn::Digit(4),
        Btn::Digit(5),
        Btn::Digit(6),
        Btn::Mul,
        Btn::Percent,
    ],
    [
        Btn::MemStore,
        Btn::Digit(1),
        Btn::Digit(2),
        Btn::Digit(3),
        Btn::Sub,
        Btn::Recip,
    ],
    [
        Btn::MemAdd,
        Btn::Digit(0),
        Btn::Neg,
        Btn::Dot,
        Btn::Add,
        Btn::Eq,
    ],
];

// --- widget ----------------------------------------------------------------

pub struct Keypad {
    engine: Rc<RefCell<Engine>>,
    rect: Rect,
    focused: bool,
    /// Every clickable key paired with its current rectangle, rebuilt on
    /// `layout`. Holds the 24 grid keys plus Back / CE / C.
    keys: Vec<(Btn, Rect)>,
    display_rect: Rect,
    indicator_rect: Rect,
    /// The key currently held down by the mouse, and whether the cursor is still
    /// over it (so it draws armed). Mirrors a real push button: drag off and it
    /// pops back up, drag back and it re-arms, release inside to fire.
    pressed: Option<Btn>,
    over: bool,
    /// The key shown depressed because a keyboard key is being held, plus the
    /// base key whose `KeyUp` pops it back up. `KeyDown`/`KeyUp` report the key
    /// with modifiers stripped, so a shifted operator like `+` arrives as
    /// `Char('+')` but releases as `KeyUp(Char('='))` — hence we remember the
    /// base key (`=`) rather than the glyph.
    held: Option<Btn>,
    held_key: Option<Key>,
    /// Base key of the most recent `KeyDown`, so the `Char` it produces can be
    /// tied back to the physical key that will release it.
    pending_down: Option<Key>,
    /// Whether the most recent `KeyDown` was a fresh press rather than OS
    /// auto-repeat (the same key firing again while still held). A calculator
    /// acts once per press, so the activation paths consult this and skip the
    /// repeat events — the key stays depressed, but nothing re-triggers.
    fresh_press: bool,
}

impl Keypad {
    pub fn new(engine: Rc<RefCell<Engine>>) -> Self {
        Self {
            engine,
            rect: Rect::new(0, 0, 0, 0),
            focused: false,
            keys: Vec::new(),
            display_rect: Rect::new(0, 0, 0, 0),
            indicator_rect: Rect::new(0, 0, 0, 0),
            pressed: None,
            over: false,
            held: None,
            held_key: None,
            pending_down: None,
            fresh_press: true,
        }
    }

    /// Recompute every rectangle from the allocated `bounds`, centring the
    /// fixed-size calculator block horizontally.
    fn rebuild(&mut self, bounds: Rect) {
        self.rect = bounds;
        let gx = bounds.x + ((bounds.w - GRID_W) / 2).max(MARGIN);
        let top = bounds.y + MARGIN;

        self.display_rect = Rect::new(gx, top, GRID_W, DISPLAY_H);

        // Back/CE/C strip: a memory indicator the width of the memory column,
        // then three equal buttons filling the rest.
        let func_y = top + DISPLAY_H + SECTION_GAP;
        self.indicator_rect = Rect::new(gx, func_y, BTN_W, FUNC_H);
        let strip_x = gx + BTN_W + GAP;
        let strip_w = GRID_W - BTN_W - GAP;
        let fbw = (strip_w - 2 * GAP) / 3;

        self.keys.clear();
        self.keys
            .push((Btn::Back, Rect::new(strip_x, func_y, fbw, FUNC_H)));
        self.keys.push((
            Btn::ClearEntry,
            Rect::new(strip_x + fbw + GAP, func_y, fbw, FUNC_H),
        ));
        // The last button absorbs any rounding remainder so it reaches the edge.
        let c_x = strip_x + 2 * (fbw + GAP);
        self.keys.push((
            Btn::ClearAll,
            Rect::new(c_x, func_y, gx + GRID_W - c_x, FUNC_H),
        ));

        let grid_top = func_y + FUNC_H + SECTION_GAP;
        for (r, row) in GRID.iter().enumerate() {
            for (c, &btn) in row.iter().enumerate() {
                let x = gx + c as i32 * (BTN_W + GAP);
                let y = grid_top + r as i32 * (BTN_H + GAP);
                self.keys.push((btn, Rect::new(x, y, BTN_W, BTN_H)));
            }
        }
    }

    fn key_at(&self, pos: Point) -> Option<Btn> {
        self.keys
            .iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(b, _)| *b)
    }

    fn key_rect(&self, btn: Btn) -> Option<Rect> {
        self.keys.iter().find(|(b, _)| *b == btn).map(|(_, r)| *r)
    }

    /// Run a key's action against the engine.
    fn activate(&self, btn: Btn) {
        let mut e = self.engine.borrow_mut();
        match btn {
            Btn::Digit(d) => e.input_digit(d),
            Btn::Dot => e.input_dot(),
            Btn::Neg => e.negate(),
            Btn::Add => e.input_op(Op::Add),
            Btn::Sub => e.input_op(Op::Sub),
            Btn::Mul => e.input_op(Op::Mul),
            Btn::Div => e.input_op(Op::Div),
            Btn::Eq => e.equals(),
            Btn::Sqrt => e.sqrt(),
            Btn::Percent => e.percent(),
            Btn::Recip => e.reciprocal(),
            Btn::Back => e.backspace(),
            Btn::ClearEntry => e.clear_entry(),
            Btn::ClearAll => e.clear_all(),
            Btn::MemClear => e.memory_clear(),
            Btn::MemRecall => e.memory_recall(),
            Btn::MemStore => e.memory_store(),
            Btn::MemAdd => e.memory_add(),
        }
    }

    /// Copy the current value to the clipboard (Edit ▸ Copy / Ctrl+C).
    pub fn copy(&self) {
        clipboard::copy(&self.engine.borrow().clipboard_text());
    }

    /// Paste a number from the clipboard onto the display (Edit ▸ Paste / Ctrl+V).
    pub fn paste(&self) {
        if let Some(text) = clipboard::paste() {
            self.engine.borrow_mut().paste(&text);
        }
    }

    // -- keyboard ----------------------------------------------------------

    /// Mark `btn` as depressed until the base `key` (the one a matching `KeyUp`
    /// will carry) is released.
    fn set_held(&mut self, btn: Btn, key: Key) {
        self.held = Some(btn);
        self.held_key = Some(key);
    }

    /// Handle a `KeyDown`: named keys plus the Ctrl chords (clipboard + memory),
    /// matching the Win 3.1 / Windows shortcuts. AltGr (which reports as
    /// Ctrl+Alt) is excluded so composing characters never triggers them.
    /// Returns whether the event was consumed.
    fn handle_key_down(&mut self, key: Key, modifiers: saudade::Modifiers) -> bool {
        if modifiers.control && !modifiers.alt_graph {
            let Key::Char(c) = key else { return false };
            match c.to_ascii_lowercase() {
                'c' => {
                    if self.fresh_press {
                        self.copy();
                    }
                }
                'v' => {
                    if self.fresh_press {
                        self.paste();
                    }
                }
                other => {
                    let Some(btn) = btn_for_ctrl(other) else {
                        return false;
                    };
                    if self.fresh_press {
                        self.activate(btn);
                    }
                    self.set_held(btn, key);
                }
            }
            return true;
        }
        if modifiers.has_command() {
            return false;
        }
        let Key::Named(named) = key else { return false };
        let Some(btn) = btn_for_named(named) else {
            return false;
        };
        if self.fresh_press {
            self.activate(btn);
        }
        self.set_held(btn, key);
        true
    }

    /// Handle a text character (a `Char` event). Returns whether it was consumed.
    fn handle_char(&mut self, ch: char) -> bool {
        let Some(btn) = btn_for_char(ch) else {
            return false;
        };
        // Fire once per press: the `Char`s that auto-repeat while the key is held
        // (flagged by the preceding `KeyDown`) keep the key depressed but don't
        // re-trigger the action.
        if self.fresh_press {
            self.activate(btn);
        }
        // Depress the key until the base key from the preceding `KeyDown` is
        // released. If there was none (e.g. a synthesised `Char` in a test),
        // just show it pressed with nothing to pop it back up.
        match self.pending_down {
            Some(key) => self.set_held(btn, key),
            None => {
                self.held = Some(btn);
                self.held_key = None;
            }
        }
        true
    }

    // -- painting ----------------------------------------------------------

    fn paint_display(&self, p: &mut Painter, theme: &Theme) {
        let r = self.display_rect;
        p.fill_rect(r, Color::WHITE);
        p.sunken_bevel(r, theme.highlight, theme.shadow);
        p.stroke_rect(r, theme.border);

        let engine = self.engine.borrow();
        let text = engine.display();
        let is_error = engine.is_error();
        drop(engine);

        let inner = r.inset(6);
        let saved = p.push_clip(inner);
        if is_error {
            // Error messages read as a label: left-aligned, in maroon.
            let size = p.measure_text(&text, theme.font_size);
            let ty = inner.y + (inner.h - size.h) / 2;
            p.text(inner.x, ty, &text, theme.font_size, MAROON);
        } else {
            // Numbers are right-aligned, in the monospace face when there is one
            // (an LCD feel) and the proportional face otherwise.
            let mono = p.measure_mono_text(&text, DISPLAY_FONT);
            if mono.w > 0 {
                let tx = inner.right() - mono.w;
                let ty = inner.y + (inner.h - mono.h) / 2;
                p.mono_text(tx, ty, &text, DISPLAY_FONT, Color::BLACK);
            } else {
                let size = p.measure_text(&text, DISPLAY_FONT);
                let tx = inner.right() - size.w;
                let ty = inner.y + (inner.h - size.h) / 2;
                p.text(tx, ty, &text, DISPLAY_FONT, Color::BLACK);
            }
        }
        p.restore_clip(saved);
    }

    fn paint_indicator(&self, p: &mut Painter, theme: &Theme) {
        let r = self.indicator_rect;
        p.fill_rect(r, Color::WHITE);
        p.sunken_bevel(r, theme.highlight, theme.shadow);
        p.stroke_rect(r, theme.border);
        if self.engine.borrow().memory_active() {
            p.text_centered(r, "M", theme.font_size, MAROON);
        }
    }
}

/// The keypad button a text character maps to (driving `Char` events).
fn btn_for_char(ch: char) -> Option<Btn> {
    Some(match ch {
        '0'..='9' => Btn::Digit(ch as u8 - b'0'),
        '.' | ',' => Btn::Dot,
        '+' => Btn::Add,
        '-' => Btn::Sub,
        '*' => Btn::Mul,
        '/' => Btn::Div,
        '=' => Btn::Eq,
        '%' => Btn::Percent,
        'r' | 'R' => Btn::Recip,
        '@' | 'q' | 'Q' => Btn::Sqrt,
        _ => return None,
    })
}

/// The keypad button a named key maps to (driving `KeyDown` events).
fn btn_for_named(named: NamedKey) -> Option<Btn> {
    Some(match named {
        NamedKey::Enter => Btn::Eq,
        NamedKey::Backspace => Btn::Back,
        NamedKey::Delete => Btn::ClearEntry,
        NamedKey::Escape => Btn::ClearAll,
        _ => return None,
    })
}

/// The memory keypad button a (lowercased) Ctrl-chord letter maps to.
fn btn_for_ctrl(c: char) -> Option<Btn> {
    Some(match c {
        'l' => Btn::MemClear,
        'r' => Btn::MemRecall,
        'm' => Btn::MemStore,
        'p' => Btn::MemAdd,
        _ => return None,
    })
}

/// Paint one beveled key with its centred, coloured label, nudged down-right a
/// pixel while pressed.
fn paint_key(p: &mut Painter, theme: &Theme, rect: Rect, label: &str, color: Color, pressed: bool) {
    p.button(rect, theme, pressed, false);
    let face = if pressed {
        Rect::new(rect.x + 1, rect.y + 1, rect.w, rect.h)
    } else {
        rect
    };
    p.text_centered(face, label, theme.font_size, color);
}

impl Widget for Keypad {
    fn bounds(&self) -> Rect {
        self.rect
    }

    fn layout(&mut self, bounds: Rect) {
        self.rebuild(bounds);
    }

    fn paint(&mut self, p: &mut Painter, theme: &Theme) {
        // Solid light-gray body, like the classic dialog chrome (no workspace
        // pattern peeking through behind the keys).
        p.fill_rect(self.rect, theme.face);
        self.paint_display(p, theme);
        self.paint_indicator(p, theme);

        // A key reads as pressed if the mouse is holding it (and still over it)
        // or a keyboard key bound to it is being held down.
        let mouse_down = self.pressed.filter(|_| self.over);
        let held = self.held;
        for (btn, rect) in &self.keys {
            let is_down = mouse_down == Some(*btn) || held == Some(*btn);
            paint_key(p, theme, *rect, btn.label(), btn.color(), is_down);
        }
    }

    fn event(&mut self, event: &Event, ctx: &mut EventCtx) {
        match event {
            Event::PointerDown {
                pos,
                button: MouseButton::Left,
            } => {
                ctx.request_focus();
                if let Some(btn) = self.key_at(*pos) {
                    self.pressed = Some(btn);
                    self.over = true;
                    ctx.request_paint();
                }
            }
            Event::PointerMove { pos } => {
                if let Some(btn) = self.pressed {
                    let now_over = self.key_rect(btn).is_some_and(|r| r.contains(*pos));
                    if now_over != self.over {
                        self.over = now_over;
                        ctx.request_paint();
                    }
                }
            }
            Event::PointerUp {
                pos,
                button: MouseButton::Left,
            } => {
                if let Some(btn) = self.pressed.take() {
                    if self.key_rect(btn).is_some_and(|r| r.contains(*pos)) {
                        self.activate(btn);
                    }
                    self.over = false;
                    ctx.request_paint();
                }
            }
            Event::KeyDown { key, modifiers } => {
                if self.focused {
                    // A key that's already down firing again is OS auto-repeat;
                    // a calculator acts once per press, so flag whether this is a
                    // fresh press (the activation paths skip repeats). The base
                    // key is also remembered so the `Char` it generates can be
                    // tied back to the `KeyUp` that releases it.
                    self.fresh_press = self.pending_down != Some(*key);
                    self.pending_down = Some(*key);
                    if self.handle_key_down(*key, *modifiers) {
                        ctx.consume_event();
                        ctx.request_paint();
                    }
                }
            }
            Event::Char { ch, modifiers } => {
                if self.focused && !modifiers.has_command() && self.handle_char(*ch) {
                    ctx.consume_event();
                    ctx.request_paint();
                }
            }
            Event::KeyUp { key, .. } => {
                if self.pending_down == Some(*key) {
                    self.pending_down = None;
                }
                // Pop the key back up once the physical key that armed it is let go.
                if self.held_key == Some(*key) {
                    self.held = None;
                    self.held_key = None;
                    ctx.request_paint();
                }
            }
            _ => {}
        }
    }

    fn captures_pointer(&self) -> bool {
        self.pressed.is_some()
    }

    fn focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            // Drop any keyboard-held press so a key doesn't stay stuck down
            // after focus moves away (e.g. when a menu opens).
            self.held = None;
            self.held_key = None;
            self.pending_down = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saudade::Modifiers;
    use saudade::mock::MockBackend;

    fn keypad() -> Keypad {
        let mut kp = Keypad::new(Rc::new(RefCell::new(Engine::new())));
        kp.layout(Rect::new(0, 0, NATURAL_WIDTH, NATURAL_HEIGHT));
        kp.set_focused(true);
        kp
    }

    fn char_event(ch: char) -> Event {
        Event::Char {
            ch,
            modifiers: Modifiers::default(),
        }
    }

    fn key_event(named: NamedKey) -> Event {
        Event::KeyDown {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
        }
    }

    fn key_up(named: NamedKey) -> Event {
        Event::KeyUp {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
        }
    }

    fn key_down_char(c: char) -> Event {
        Event::KeyDown {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
        }
    }

    fn key_up_char(c: char) -> Event {
        Event::KeyUp {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn layout_places_every_key_without_overlap() {
        let kp = keypad();
        // 24 grid keys + Back/CE/C.
        assert_eq!(kp.keys.len(), 27);
        // Every key sits inside the widget bounds.
        for (_, r) in &kp.keys {
            assert!(r.x >= kp.rect.x && r.right() <= kp.rect.right());
            assert!(r.y >= kp.rect.y && r.bottom() <= kp.rect.bottom());
        }
        // The "5" key is reachable by a click at its centre.
        let five = kp.key_rect(Btn::Digit(5)).unwrap();
        assert_eq!(
            kp.key_at(Point::new(five.x + five.w / 2, five.y + five.h / 2)),
            Some(Btn::Digit(5))
        );
    }

    #[test]
    fn keyboard_drives_a_calculation() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        for ev in [
            char_event('1'),
            char_event('2'),
            char_event('+'),
            char_event('3'),
        ] {
            backend.dispatch(&mut kp, &ev);
        }
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter));
        assert_eq!(kp.engine.borrow().display(), "15");
    }

    #[test]
    fn function_letters_map_to_keys() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        // 9, then 'q' (sqrt) → 3.
        backend.dispatch(&mut kp, &char_event('9'));
        backend.dispatch(&mut kp, &char_event('q'));
        assert_eq!(kp.engine.borrow().display(), "3");
    }

    #[test]
    fn escape_clears_and_delete_clears_entry() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        backend.dispatch(&mut kp, &char_event('7'));
        backend.dispatch(&mut kp, &char_event('+'));
        backend.dispatch(&mut kp, &char_event('5'));
        // Delete = CE: clears the 5 but keeps the pending +.
        backend.dispatch(&mut kp, &key_event(NamedKey::Delete));
        backend.dispatch(&mut kp, &char_event('2'));
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter));
        assert_eq!(kp.engine.borrow().display(), "9");
        // Escape = C: wipes everything.
        backend.dispatch(&mut kp, &key_event(NamedKey::Escape));
        assert_eq!(kp.engine.borrow().display(), "0");
    }

    #[test]
    fn ctrl_chords_reach_memory() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        // Type 8, Ctrl+M stores it to memory.
        backend.dispatch(&mut kp, &char_event('8'));
        backend.dispatch(
            &mut kp,
            &Event::KeyDown {
                key: Key::Char('m'),
                modifiers: ctrl,
            },
        );
        assert!(kp.engine.borrow().memory_active());
    }

    #[test]
    fn mouse_press_and_release_fires_a_key() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        let seven = kp.key_rect(Btn::Digit(7)).unwrap();
        let center = Point::new(seven.x + seven.w / 2, seven.y + seven.h / 2);
        backend.dispatch(
            &mut kp,
            &Event::PointerDown {
                pos: center,
                button: MouseButton::Left,
            },
        );
        assert_eq!(kp.pressed, Some(Btn::Digit(7)));
        backend.dispatch(
            &mut kp,
            &Event::PointerUp {
                pos: center,
                button: MouseButton::Left,
            },
        );
        assert_eq!(kp.engine.borrow().display(), "7");
        assert_eq!(kp.pressed, None);
    }

    #[test]
    fn releasing_off_the_key_cancels_it() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        let seven = kp.key_rect(Btn::Digit(7)).unwrap();
        let center = Point::new(seven.x + seven.w / 2, seven.y + seven.h / 2);
        backend.dispatch(
            &mut kp,
            &Event::PointerDown {
                pos: center,
                button: MouseButton::Left,
            },
        );
        // Release far away (off the button) — nothing fires.
        let far = Point::new(kp.rect.right() - 1, kp.rect.bottom() - 1);
        backend.dispatch(
            &mut kp,
            &Event::PointerUp {
                pos: far,
                button: MouseButton::Left,
            },
        );
        assert_eq!(kp.engine.borrow().display(), "0");
    }

    #[test]
    fn a_held_digit_key_shows_depressed_until_release() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        // A real keystroke is KeyDown(base key) followed by Char(glyph).
        backend.dispatch(&mut kp, &key_down_char('5'));
        backend.dispatch(&mut kp, &char_event('5'));
        assert_eq!(kp.held, Some(Btn::Digit(5)), "the 5 key is held down");
        // The matching KeyUp pops it back up.
        backend.dispatch(&mut kp, &key_up_char('5'));
        assert_eq!(kp.held, None);
    }

    #[test]
    fn a_shifted_operator_pops_up_on_its_base_key() {
        // `+` is typed as Shift+`=`: KeyDown/KeyUp carry the base key `=`, while
        // the Char carries `+`. The depress must clear on KeyUp of `=`.
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        backend.dispatch(&mut kp, &key_down_char('=')); // base key down
        backend.dispatch(&mut kp, &char_event('+')); // composed glyph
        assert_eq!(kp.held, Some(Btn::Add));
        backend.dispatch(&mut kp, &key_up_char('=')); // base key up
        assert_eq!(kp.held, None, "releasing the base key pops `+` back up");
    }

    #[test]
    fn a_held_named_key_shows_depressed_until_release() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter));
        assert_eq!(kp.held, Some(Btn::Eq));
        backend.dispatch(&mut kp, &key_up(NamedKey::Enter));
        assert_eq!(kp.held, None);
    }

    #[test]
    fn losing_focus_pops_a_held_key_up() {
        // If focus moves away (a menu opens) mid-hold, the key must not stay stuck.
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter));
        assert_eq!(kp.held, Some(Btn::Eq));
        kp.set_focused(false);
        assert_eq!(kp.held, None);
    }

    #[test]
    fn holding_a_digit_key_does_not_auto_repeat() {
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        // Press and hold "5": the initial KeyDown+Char types it once; the
        // auto-repeat KeyDown+Char pairs that follow (no KeyUp between) must not
        // type more digits.
        backend.dispatch(&mut kp, &key_down_char('5'));
        backend.dispatch(&mut kp, &char_event('5'));
        backend.dispatch(&mut kp, &key_down_char('5')); // auto-repeat
        backend.dispatch(&mut kp, &char_event('5'));
        backend.dispatch(&mut kp, &key_down_char('5')); // auto-repeat
        backend.dispatch(&mut kp, &char_event('5'));
        assert_eq!(kp.engine.borrow().display(), "5", "a held key types once");
        // Release and press again: a distinct press types another digit.
        backend.dispatch(&mut kp, &key_up_char('5'));
        backend.dispatch(&mut kp, &key_down_char('5'));
        backend.dispatch(&mut kp, &char_event('5'));
        assert_eq!(
            kp.engine.borrow().display(),
            "55",
            "a fresh press types again"
        );
    }

    #[test]
    fn holding_enter_evaluates_once() {
        // 2 + 3, then hold Enter: equals fires once (→ 5). If auto-repeat leaked
        // through it would keep replaying repeat-equals (5 → 8 → 11 …).
        let mut kp = keypad();
        let backend = MockBackend::new(NATURAL_WIDTH, NATURAL_HEIGHT);
        for ev in [char_event('2'), char_event('+'), char_event('3')] {
            backend.dispatch(&mut kp, &ev);
        }
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter)); // fresh: = → 5
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter)); // auto-repeat: ignored
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter)); // auto-repeat: ignored
        assert_eq!(kp.engine.borrow().display(), "5");
        // Release and press Enter again: a fresh press replays repeat-equals (+3).
        backend.dispatch(&mut kp, &key_up(NamedKey::Enter));
        backend.dispatch(&mut kp, &key_event(NamedKey::Enter));
        assert_eq!(kp.engine.borrow().display(), "8");
    }
}
