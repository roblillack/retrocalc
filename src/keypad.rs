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

    /// Route a keyboard event to the right handler, gated on focus. Returns
    /// whether it was consumed.
    fn handle_keyboard(&self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }
        match event {
            Event::KeyDown { key, modifiers } => self.on_key_down(*key, *modifiers),
            Event::Char { ch, modifiers } => !modifiers.has_command() && self.on_char(*ch),
            _ => false,
        }
    }

    /// Handle a named/keyboard key. Returns whether it was consumed.
    fn on_key_down(&self, key: Key, modifiers: saudade::Modifiers) -> bool {
        // Control chords: clipboard + memory, matching the Win 3.1 / Windows
        // shortcuts. AltGr (which reports as Ctrl+Alt) is excluded so composing
        // characters never triggers them.
        if modifiers.control && !modifiers.alt_graph {
            if let Key::Char(c) = key {
                match c.to_ascii_lowercase() {
                    'c' => self.copy(),
                    'v' => self.paste(),
                    'l' => self.activate(Btn::MemClear),
                    'r' => self.activate(Btn::MemRecall),
                    'm' => self.activate(Btn::MemStore),
                    'p' => self.activate(Btn::MemAdd),
                    _ => return false,
                }
                return true;
            }
            return false;
        }

        if modifiers.has_command() {
            return false;
        }

        match key {
            Key::Named(NamedKey::Enter) => self.activate(Btn::Eq),
            Key::Named(NamedKey::Backspace) => self.activate(Btn::Back),
            Key::Named(NamedKey::Delete) => self.activate(Btn::ClearEntry),
            Key::Named(NamedKey::Escape) => self.activate(Btn::ClearAll),
            _ => return false,
        }
        true
    }

    /// Handle a text character. Returns whether it was consumed.
    fn on_char(&self, ch: char) -> bool {
        match ch {
            '0'..='9' => self.activate(Btn::Digit(ch as u8 - b'0')),
            '.' | ',' => self.activate(Btn::Dot),
            '+' => self.activate(Btn::Add),
            '-' => self.activate(Btn::Sub),
            '*' => self.activate(Btn::Mul),
            '/' => self.activate(Btn::Div),
            '=' => self.activate(Btn::Eq),
            '%' => self.activate(Btn::Percent),
            'r' | 'R' => self.activate(Btn::Recip),
            '@' | 'q' | 'Q' => self.activate(Btn::Sqrt),
            _ => return false,
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

        let pressed = self.pressed;
        let over = self.over;
        for (btn, rect) in &self.keys {
            let is_down = pressed == Some(*btn) && over;
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
            Event::KeyDown { .. } | Event::Char { .. } => {
                let handled = self.handle_keyboard(event);
                if handled {
                    ctx.consume_event();
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
}
