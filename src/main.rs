//! retrocalc — a retro Windows 3.1-styled calculator.
//!
//! The window is a [`Column`](saudade::Column): a menu bar pinned to the top and
//! the [`Keypad`] filling the rest, with a shared "About" [`Dialog`] riding along
//! as an overlay. The keypad owns the calculator [`Engine`]; the Edit menu shares
//! a handle to it so Copy / Paste can reach the current value. Built on the
//! [saudade](https://crates.io/crates/saudade) toolkit — the same one behind
//! retrofetch and control-panel.

mod clipboard;
mod engine;
mod keypad;

use std::cell::RefCell;
use std::rc::Rc;

use saudade::{
    App, Column, Dialog, Event, EventCtx, Menu, MenuBar, MenuItem, Painter, PopupRequest, Rect,
    Theme, Widget, WindowConfig,
};

use engine::Engine;
use keypad::Keypad;

/// Menu-bar height.
const MENU_H: i32 = 20;
/// Window-content width: the keypad's natural width.
const WIDTH: i32 = keypad::NATURAL_WIDTH;
/// Window-content height: the menu bar plus the keypad.
const HEIGHT: i32 = MENU_H + keypad::NATURAL_HEIGHT;

const ABOUT_TEXT: &str = concat!(
    "RetroCalc ",
    env!("CARGO_PKG_VERSION"),
    "\n\nA Windows 3.1-styled calculator built\nwith the Saudade toolkit.\n\nStandard view, with full keyboard and\nmouse support.",
);

fn main() {
    let engine = Rc::new(RefCell::new(Engine::new()));
    let about = Rc::new(RefCell::new(Dialog::new()));
    let root = build_root(engine, about);

    App::new(WindowConfig::new("Calculator", WIDTH, HEIGHT), root)
        .with_theme(Theme::windows_31())
        .run();
}

/// Assemble the whole widget tree. Split out from [`main`] so headless tests can
/// drive the exact same root the live app does.
fn build_root(engine: Rc<RefCell<Engine>>, about: Rc<RefCell<Dialog>>) -> Column {
    let menu = MenuBar::new(Rect::new(0, 0, WIDTH, MENU_H))
        .add_menu(Menu::new(
            "&Edit",
            vec![
                MenuItem::action("&Copy", {
                    let engine = engine.clone();
                    move |cx| {
                        clipboard::copy(&engine.borrow().clipboard_text());
                        cx.request_paint();
                    }
                })
                .with_accel("Ctrl+C"),
                MenuItem::action("&Paste", {
                    let engine = engine.clone();
                    move |cx| {
                        if let Some(text) = clipboard::paste() {
                            engine.borrow_mut().paste(&text);
                        }
                        cx.request_paint();
                    }
                })
                .with_accel("Ctrl+V"),
            ],
        ))
        .add_menu(Menu::new(
            "&Help",
            vec![MenuItem::action("&About RetroCalc...", {
                let about = about.clone();
                move |cx| {
                    about.borrow_mut().show_info("About RetroCalc", ABOUT_TEXT);
                    cx.request_paint();
                }
            })],
        ));

    Column::new()
        .with_background(Theme::windows_31().face)
        .add_fixed(menu, MENU_H)
        .add_fill(Keypad::new(engine))
        .add_overlay(SharedDialog(about))
}

/// Lets the About [`Dialog`] live in the widget tree as an overlay while the
/// Help menu keeps another handle to show it — the same shared-overlay adapter
/// control-panel uses. Every `Widget` method delegates straight through.
struct SharedDialog(Rc<RefCell<Dialog>>);

impl Widget for SharedDialog {
    fn bounds(&self) -> Rect {
        self.0.borrow().bounds()
    }
    fn paint(&mut self, painter: &mut Painter, theme: &Theme) {
        self.0.borrow_mut().paint(painter, theme);
    }
    fn paint_overlay(&mut self, painter: &mut Painter, theme: &Theme) {
        self.0.borrow_mut().paint_overlay(painter, theme);
    }
    fn event(&mut self, event: &Event, ctx: &mut EventCtx) {
        self.0.borrow_mut().event(event, ctx);
    }
    fn captures_pointer(&self) -> bool {
        self.0.borrow().captures_pointer()
    }
    fn accepts_accelerators(&self) -> bool {
        self.0.borrow().accepts_accelerators()
    }
    fn layout(&mut self, bounds: Rect) {
        self.0.borrow_mut().layout(bounds);
    }
    fn popup_request(&self) -> Option<PopupRequest> {
        self.0.borrow().popup_request()
    }
    fn collect_popups(&self, out: &mut Vec<PopupRequest>) {
        self.0.borrow().collect_popups(out);
    }
    fn wants_ticks(&self) -> bool {
        self.0.borrow().wants_ticks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saudade::mock::MockBackend;
    use saudade::{Key, Modifiers, NamedKey};

    fn app() -> (Column, Rc<RefCell<Engine>>) {
        let engine = Rc::new(RefCell::new(Engine::new()));
        let about = Rc::new(RefCell::new(Dialog::new()));
        let mut root = build_root(engine.clone(), about);
        root.layout(Rect::new(0, 0, WIDTH, HEIGHT));
        // Mirror the runtime: focus the first focusable child (the keypad).
        root.focus_first();
        (root, engine)
    }

    fn char_event(ch: char) -> Event {
        Event::Char {
            ch,
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn typing_reaches_the_engine_through_the_whole_tree() {
        let (mut root, engine) = app();
        let backend = MockBackend::new(WIDTH, HEIGHT);
        for ev in [char_event('7'), char_event('+'), char_event('8')] {
            backend.dispatch(&mut root, &ev);
        }
        backend.dispatch(
            &mut root,
            &Event::KeyDown {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
            },
        );
        assert_eq!(engine.borrow().display(), "15");
    }

    #[test]
    fn help_about_opens_a_single_dialog_window() {
        let (mut root, _engine) = app();
        let backend = MockBackend::new(WIDTH, HEIGHT);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        // Alt+H opens Help (pre-highlighting its only item, About); 'a' fires it.
        backend.dispatch(
            &mut root,
            &Event::KeyDown {
                key: Key::Char('h'),
                modifiers: alt,
            },
        );
        // The open menu fires its item from a KeyDown, the same way control-panel
        // drives this (a `Char` event wouldn't reach the menu).
        backend.dispatch(
            &mut root,
            &Event::KeyDown {
                key: Key::Char('a'),
                modifiers: Modifiers::default(),
            },
        );
        let mut popups = Vec::new();
        root.collect_popups(&mut popups);
        assert_eq!(popups.len(), 1, "About should open exactly one dialog");
    }

    #[test]
    fn the_window_renders_headlessly() {
        let (mut root, _engine) = app();
        // Paint with no font loaded — must not panic.
        let backend = MockBackend::new(WIDTH, HEIGHT);
        backend.render(&mut root);
    }

    /// Render the calculator to `screenshot.png` for the README. Ignored by
    /// default (writes a file, needs a system font); run with
    /// `cargo test -- --ignored render_screenshot`.
    #[test]
    #[ignore]
    fn render_screenshot() {
        use saudade::Font;

        let (mut root, _engine) = app();
        let mut backend = MockBackend::new(WIDTH, HEIGHT).with_scale(2.0);
        if let Some(font) = Font::load_system() {
            backend = backend.with_font(font);
        }

        // Type a sample value and store it to memory so the "M" indicator lights.
        for ch in "1234.5678".chars() {
            backend.dispatch(&mut root, &char_event(ch));
        }
        backend.dispatch(
            &mut root,
            &Event::KeyDown {
                key: Key::Char('m'),
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            },
        );

        let snap = backend.render(&mut root);
        std::fs::write("screenshot.png", snap.to_png()).unwrap();
    }
}
