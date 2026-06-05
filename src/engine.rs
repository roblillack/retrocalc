//! The calculator engine — all of the arithmetic, none of the pixels.
//!
//! This is the brain behind the keypad: a small state machine that models the
//! Windows 3.1 `CALC.EXE` standard view. It uses the same **immediate
//! execution** semantics that calculator did (and that pocket calculators still
//! do): there is no operator precedence, each operator key resolves the one
//! before it, so `2 + 3 * 4 =` is `(2 + 3) * 4 = 20`, not `14`.
//!
//! The engine is deliberately free of any UI types so it can be unit-tested in
//! isolation (see the tests at the bottom); [`crate::keypad`] owns one and pokes
//! at it from pointer / keyboard handlers.

/// The four binary operators. `%`, `sqrt`, `1/x` and `+/-` act on the current
/// display immediately and so aren't modelled here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    /// Apply the operator, surfacing the two failure modes the display has to
    /// report: division by zero and a result that overflowed to infinity.
    fn apply(self, a: f64, b: f64) -> Result<f64, CalcError> {
        let r = match self {
            Op::Add => a + b,
            Op::Sub => a - b,
            Op::Mul => a * b,
            Op::Div => {
                if b == 0.0 {
                    return Err(CalcError::DivByZero);
                }
                a / b
            }
        };
        finite(r)
    }
}

/// Why the engine is wedged. While an error is latched the display shows the
/// message and the engine ignores everything except a digit, `C`, or `CE` —
/// exactly like the Win 3.1 calculator, which made you clear before carrying on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcError {
    DivByZero,
    /// `sqrt` of a negative number.
    Invalid,
    Overflow,
}

impl CalcError {
    /// The text shown in the display while this error is latched.
    pub fn message(self) -> &'static str {
        match self {
            CalcError::DivByZero => "Cannot divide by zero",
            CalcError::Invalid => "Invalid input",
            CalcError::Overflow => "Overflow",
        }
    }
}

/// Largest number of significant digits an entry may hold — the readout caps
/// out here just like the original's fixed-width LCD.
const MAX_DIGITS: usize = 16;

/// `Ok(v)` if `v` is a finite number, `Err(Overflow)` otherwise — the single
/// gate every computed value passes through before it can reach the display.
fn finite(v: f64) -> Result<f64, CalcError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CalcError::Overflow)
    }
}

pub struct Engine {
    /// Running left-hand total: what a pending operator will combine with the
    /// current display when it resolves.
    acc: f64,
    /// Operator waiting for its right-hand operand, if any.
    pending: Option<Op>,
    /// Whether the current display is a right-hand operand ready to be folded
    /// into `acc` by `pending`. Cleared the instant an operator is pressed (the
    /// display then shows `acc`, not a fresh operand) and set again as soon as a
    /// digit is typed or a value is recalled / computed. This is what lets
    /// `5 + + =` mean `5 + 5` rather than double-applying.
    operand_ready: bool,
    /// The last `(operator, right operand)` pair, replayed when `=` is pressed
    /// repeatedly: `5 + 3 =` then `=` again yields `11`.
    repeat: Option<(Op, f64)>,
    /// The current numeric value on the display. Kept in sync with `entry` while
    /// the user is typing; holds the computed value otherwise.
    value: f64,
    /// `Some` while the user is typing a number, holding the literal characters
    /// so the display can echo a trailing `.` and Backspace can edit them.
    /// `None` means the display is showing a finished value (a result, a recall,
    /// the running total after an operator).
    entry: Option<String>,
    memory: f64,
    error: Option<CalcError>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            acc: 0.0,
            pending: None,
            operand_ready: false,
            repeat: None,
            value: 0.0,
            entry: None,
            memory: 0.0,
            error: None,
        }
    }

    // -- queries -----------------------------------------------------------

    /// The text to paint in the readout: the error message while latched, the
    /// literal keystrokes while typing, or the formatted value otherwise.
    pub fn display(&self) -> String {
        if let Some(err) = self.error {
            return err.message().to_string();
        }
        match &self.entry {
            Some(s) => s.clone(),
            None => format_number(self.value),
        }
    }

    /// Whether the readout is currently showing an error message (the keypad
    /// renders it left-aligned, like a label, rather than as a right-aligned
    /// number).
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Whether the memory register holds a non-zero value — drives the little
    /// `M` indicator next to the keypad.
    pub fn memory_active(&self) -> bool {
        self.memory != 0.0
    }

    /// A clean string for the clipboard: the finished value, never a half-typed
    /// entry with a dangling decimal point.
    pub fn clipboard_text(&self) -> String {
        if self.error.is_some() {
            return String::new();
        }
        format_number(self.value)
    }

    // -- number entry ------------------------------------------------------

    /// Append a decimal digit (0–9) to the current entry, starting a fresh one
    /// if the display was showing a finished value.
    pub fn input_digit(&mut self, d: u8) {
        self.recover_from_error();
        let s = self.entry.get_or_insert_with(|| "0".to_string());
        if s.chars().filter(|c| c.is_ascii_digit()).count() >= MAX_DIGITS {
            return;
        }
        // Replace a lone leading zero so "0" + 5 reads "5", not "05" — but keep
        // it once a decimal point is in play ("0." + 5 -> "0.5").
        if s == "0" {
            s.clear();
        } else if s == "-0" {
            *s = "-".to_string();
        }
        s.push((b'0' + d) as char);
        self.sync_value_from_entry();
        self.operand_ready = true;
    }

    /// Begin (or continue) the fractional part of the entry.
    pub fn input_dot(&mut self) {
        self.recover_from_error();
        match &mut self.entry {
            Some(s) => {
                if !s.contains('.') {
                    s.push('.');
                }
            }
            None => self.entry = Some("0.".to_string()),
        }
        self.sync_value_from_entry();
        self.operand_ready = true;
    }

    /// Delete the last typed character. A no-op unless a number is being typed —
    /// you can't Backspace into a finished result, matching the original.
    pub fn backspace(&mut self) {
        if self.error.is_some() {
            return;
        }
        let Some(s) = &mut self.entry else {
            return;
        };
        s.pop();
        if s.is_empty() || s == "-" {
            *s = "0".to_string();
        }
        self.sync_value_from_entry();
    }

    // -- binary operators --------------------------------------------------

    /// Press one of `+ - * /`. Resolves any pending operator first (so chains
    /// compute left-to-right) and then arms the new one.
    pub fn input_op(&mut self, op: Op) {
        if self.error.is_some() {
            return;
        }
        match self.pending {
            // A pending operator with an operand ready resolves now.
            Some(p) if self.operand_ready => {
                if !self.commit(p, self.acc, self.value) {
                    return;
                }
            }
            // A pending operator but no new operand: the user pressed two
            // operators in a row — just swap which one is armed.
            Some(_) => {}
            // First operator of a chain: seed the accumulator from the display.
            None => self.acc = self.value,
        }
        self.pending = Some(op);
        self.operand_ready = false;
        self.entry = None;
    }

    /// Press `=`. Resolves a pending operator, or — if there's nothing pending —
    /// replays the last operator/operand so repeated `=` keeps going.
    pub fn equals(&mut self) {
        if self.error.is_some() {
            return;
        }
        if let Some(op) = self.pending {
            let rhs = self.value;
            if self.commit(op, self.acc, rhs) {
                self.repeat = Some((op, rhs));
            }
            self.pending = None;
        } else if let Some((op, rhs)) = self.repeat {
            self.commit(op, self.value, rhs);
        }
        self.operand_ready = false;
        self.entry = None;
    }

    // -- unary operators ---------------------------------------------------

    /// Toggle the sign. While typing it edits the entry in place so you can keep
    /// going; on a finished value it just negates it.
    pub fn negate(&mut self) {
        if self.error.is_some() {
            return;
        }
        match &mut self.entry {
            Some(s) if s != "0" && !s.is_empty() => {
                if let Some(rest) = s.strip_prefix('-') {
                    *s = rest.to_string();
                } else {
                    s.insert(0, '-');
                }
                self.sync_value_from_entry();
            }
            _ => self.value = -self.value,
        }
    }

    /// Square root of the current value (`sqrt` key / `@` / `q`).
    pub fn sqrt(&mut self) {
        self.unary(|v| {
            if v < 0.0 {
                Err(CalcError::Invalid)
            } else {
                Ok(v.sqrt())
            }
        });
    }

    /// Reciprocal `1/x`.
    pub fn reciprocal(&mut self) {
        self.unary(|v| {
            if v == 0.0 {
                Err(CalcError::DivByZero)
            } else {
                finite(1.0 / v)
            }
        });
    }

    /// Percent. With an operator pending it reads the display as a percentage of
    /// the accumulator — `200 + 10 % =` gives `220` — and otherwise yields zero,
    /// exactly as the Win 3.1 calculator behaved.
    pub fn percent(&mut self) {
        if self.error.is_some() {
            return;
        }
        let result = if self.pending.is_some() {
            self.acc * self.value / 100.0
        } else {
            0.0
        };
        match finite(result) {
            Ok(v) => {
                self.value = v;
                self.entry = None;
                self.operand_ready = true;
            }
            Err(e) => self.fail(e),
        }
    }

    // -- memory ------------------------------------------------------------

    /// `MC` — clear the memory register.
    pub fn memory_clear(&mut self) {
        self.memory = 0.0;
    }

    /// `MR` — recall the memory register onto the display.
    pub fn memory_recall(&mut self) {
        if self.error.is_some() {
            return;
        }
        self.value = self.memory;
        self.entry = None;
        self.operand_ready = true;
    }

    /// `MS` — store the current display into memory.
    pub fn memory_store(&mut self) {
        if self.error.is_some() {
            return;
        }
        self.memory = self.value;
        self.entry = None;
    }

    /// `M+` — add the current display to memory.
    pub fn memory_add(&mut self) {
        if self.error.is_some() {
            return;
        }
        self.memory += self.value;
        self.entry = None;
    }

    // -- clearing ----------------------------------------------------------

    /// `CE` — clear just the current entry, leaving any pending operator and the
    /// running total intact so you can re-type the operand.
    pub fn clear_entry(&mut self) {
        self.error = None;
        self.value = 0.0;
        self.entry = None;
        self.operand_ready = false;
    }

    /// `C` — clear everything except the memory register, just like the original.
    pub fn clear_all(&mut self) {
        let memory = self.memory;
        *self = Self::new();
        self.memory = memory;
    }

    /// Replace the display with an externally supplied number (an `Edit ▸ Paste`).
    /// Anything that doesn't parse as a finite number is ignored.
    pub fn paste(&mut self, text: &str) {
        let trimmed = text.trim();
        if let Ok(v) = trimmed.parse::<f64>()
            && v.is_finite()
        {
            self.error = None;
            self.value = v;
            self.entry = None;
            self.operand_ready = true;
        }
    }

    // -- internals ---------------------------------------------------------

    /// Resolve `op(a, b)` into the accumulator and display, latching an error if
    /// it failed. Returns whether it succeeded.
    fn commit(&mut self, op: Op, a: f64, b: f64) -> bool {
        match op.apply(a, b) {
            Ok(v) => {
                self.acc = v;
                self.value = v;
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    /// Shared body of `sqrt` / `1/x`: act on the current value and show the
    /// result (or an error). The result reads as a ready operand, so
    /// `5 + 9 sqrt =` computes `5 + 3`.
    fn unary(&mut self, f: impl FnOnce(f64) -> Result<f64, CalcError>) {
        if self.error.is_some() {
            return;
        }
        match f(self.value) {
            Ok(v) => {
                self.value = v;
                self.entry = None;
                self.operand_ready = true;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Latch an error and blank the in-progress entry.
    fn fail(&mut self, e: CalcError) {
        self.error = Some(e);
        self.entry = None;
    }

    /// A digit / dot pressed while an error is latched clears it first, starting
    /// a clean slate (memory is preserved) — the only way to type past an error.
    fn recover_from_error(&mut self) {
        if self.error.is_some() {
            self.clear_all();
        }
    }

    fn sync_value_from_entry(&mut self) {
        if let Some(s) = &self.entry {
            self.value = parse_entry(s);
        }
    }
}

/// Parse a partially-typed entry (which may end in `.`, be a lone `-`, etc.)
/// into a number, treating anything not-yet-a-number as zero.
fn parse_entry(s: &str) -> f64 {
    let t = s.trim_end_matches('.');
    match t {
        "" | "-" | "-0" => 0.0,
        _ => t.parse::<f64>().unwrap_or(0.0),
    }
}

/// Render a finished value the way the readout shows it: plain decimal within a
/// comfortable magnitude band, scientific notation outside it, trailing zeros
/// trimmed, and limited to 15 significant digits so binary floating-point noise
/// (`0.1 + 0.2`) never leaks onto the display.
fn format_number(v: f64) -> String {
    if !v.is_finite() {
        return "Overflow".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }

    let abs = v.abs();
    if !(1e-4..1e16).contains(&abs) {
        return format_scientific(v);
    }

    // Choose the decimal count that yields 15 significant digits at this
    // magnitude, so large values don't expose float noise in their low digits.
    let exp = abs.log10().floor() as i32;
    let decimals = (15 - 1 - exp).clamp(0, 15) as usize;
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// Scientific notation with a trimmed 15-significant-digit mantissa and an
/// explicitly-signed exponent, e.g. `1.5e+20` or `1.234e-7`.
fn format_scientific(v: f64) -> String {
    let s = format!("{v:.14e}");
    let Some(pos) = s.find('e') else {
        return s;
    };
    let (mantissa, exp) = s.split_at(pos);
    let mut mantissa = mantissa.to_string();
    if mantissa.contains('.') {
        while mantissa.ends_with('0') {
            mantissa.pop();
        }
        if mantissa.ends_with('.') {
            mantissa.pop();
        }
    }
    // `exp` is like "e20" or "e-7"; make the positive case explicit ("e+20").
    let exp = if exp.starts_with("e-") {
        exp.to_string()
    } else {
        format!("e+{}", &exp[1..])
    };
    format!("{mantissa}{exp}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed an engine a compact script of keystrokes and return what the display
    /// reads afterward. Each character is one key: digits, `.`, the four
    /// operators, `=`, plus `r` (1/x), `q` (sqrt), `%`, `~` (+/-), `b`
    /// (Backspace), `c` (C), `e` (CE).
    fn run(script: &str) -> String {
        let mut eng = Engine::new();
        for ch in script.chars() {
            match ch {
                '0'..='9' => eng.input_digit(ch as u8 - b'0'),
                '.' => eng.input_dot(),
                '+' => eng.input_op(Op::Add),
                '-' => eng.input_op(Op::Sub),
                '*' => eng.input_op(Op::Mul),
                '/' => eng.input_op(Op::Div),
                '=' => eng.equals(),
                'r' => eng.reciprocal(),
                'q' => eng.sqrt(),
                '%' => eng.percent(),
                '~' => eng.negate(),
                'b' => eng.backspace(),
                'c' => eng.clear_all(),
                'e' => eng.clear_entry(),
                ' ' => {}
                other => panic!("unknown key in script: {other:?}"),
            }
        }
        eng.display()
    }

    #[test]
    fn types_and_shows_a_number() {
        assert_eq!(run("123"), "123");
        assert_eq!(run("0"), "0");
        assert_eq!(run("007"), "7");
        assert_eq!(run("3.14"), "3.14");
        assert_eq!(run("0.5"), "0.5");
        // A bare decimal point opens "0."
        assert_eq!(run("."), "0.");
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(run("2+3="), "5");
        assert_eq!(run("10-4="), "6");
        assert_eq!(run("6*7="), "42");
        assert_eq!(run("20/4="), "5");
    }

    #[test]
    fn immediate_execution_has_no_precedence() {
        // (2 + 3) * 4 = 20, the Win 3.1 standard-view behaviour.
        assert_eq!(run("2+3*4="), "20");
    }

    #[test]
    fn chained_operators_resolve_left_to_right() {
        assert_eq!(run("1+2+3+4="), "10");
        assert_eq!(run("100-1-2-3="), "94");
    }

    #[test]
    fn pressing_two_operators_swaps_the_pending_one() {
        // 5 +, then changed mind to ×: 5 × 3 = 15, the + never applied.
        assert_eq!(run("5+*3="), "15");
    }

    #[test]
    fn repeated_equals_replays_the_last_operation() {
        assert_eq!(run("5+3=="), "11");
        assert_eq!(run("5+3==="), "14");
        // 2 × 3 = 6, = again → 6 × 3 = 18.
        assert_eq!(run("2*3=="), "18");
    }

    #[test]
    fn equals_then_new_number_repeats_on_it() {
        // 5 + 3 = 8; type 10, =, → 10 + 3 = 13.
        assert_eq!(run("5+3=10="), "13");
    }

    #[test]
    fn divide_by_zero_latches_an_error() {
        assert_eq!(run("5/0="), "Cannot divide by zero");
        // Latched: an operator (or another `=`) is ignored, the error stays…
        assert_eq!(run("5/0=+"), "Cannot divide by zero");
        assert_eq!(run("5/0=="), "Cannot divide by zero");
        // …but a digit clears it and starts fresh.
        assert_eq!(run("5/0=7"), "7");
        // C also clears it.
        assert_eq!(run("5/0=c"), "0");
    }

    #[test]
    fn sign_toggle() {
        assert_eq!(run("5~"), "-5");
        assert_eq!(run("5~~"), "5");
        // While typing, the sign flips in place and you can keep going.
        assert_eq!(run("5~3"), "-53");
        // Negating zero stays zero (never "-0").
        assert_eq!(run("0~"), "0");
        assert_eq!(run("3-8=~"), "5");
    }

    #[test]
    fn square_root() {
        assert_eq!(run("9q"), "3");
        assert_eq!(run("2q"), "1.4142135623731");
        // sqrt feeds the pending operator: 5 + sqrt(9) = 8.
        assert_eq!(run("5+9q="), "8");
        assert_eq!(run("4~q"), "Invalid input");
    }

    #[test]
    fn reciprocal() {
        assert_eq!(run("4r"), "0.25");
        assert_eq!(run("0r"), "Cannot divide by zero");
    }

    #[test]
    fn percent_of_the_accumulator() {
        // 200 + 10% = 200 + 20 = 220.
        assert_eq!(run("200+10%="), "220");
        // 50 - 10% = 50 - 5 = 45.
        assert_eq!(run("50-10%="), "45");
        // No pending operator: percent yields zero.
        assert_eq!(run("50%"), "0");
    }

    #[test]
    fn backspace_edits_the_entry_only() {
        assert_eq!(run("123b"), "12");
        assert_eq!(run("123bb"), "1");
        assert_eq!(run("123bbb"), "0");
        // Backspacing past the start parks at 0.
        assert_eq!(run("5bb"), "0");
        // A finished result can't be backspaced into.
        assert_eq!(run("2+3=b"), "5");
        // Backspace then keep typing.
        assert_eq!(run("12b3"), "13");
    }

    #[test]
    fn clear_entry_keeps_the_pending_operation() {
        // 9 + 5, CE clears the 5, type 1, = → 9 + 1 = 10.
        assert_eq!(run("9+5e1="), "10");
    }

    #[test]
    fn clear_all_resets_to_zero() {
        assert_eq!(run("9+5c"), "0");
        assert_eq!(run("9+5c3="), "3");
    }

    #[test]
    fn memory_register() {
        let mut eng = Engine::new();
        assert!(!eng.memory_active());

        // Store 7.
        eng.input_digit(7);
        eng.memory_store();
        assert!(eng.memory_active());

        // Recall it after clearing.
        eng.clear_all();
        eng.memory_recall();
        assert_eq!(eng.display(), "7");

        // M+ accumulates: 7 + 3 in memory = 10.
        eng.clear_all();
        eng.input_digit(3);
        eng.memory_add();
        eng.clear_all();
        eng.memory_recall();
        assert_eq!(eng.display(), "10");

        // MC empties it.
        eng.memory_clear();
        assert!(!eng.memory_active());
    }

    #[test]
    fn clear_all_preserves_memory() {
        let mut eng = Engine::new();
        eng.input_digit(5);
        eng.memory_store();
        eng.clear_all();
        assert!(eng.memory_active(), "C must not wipe memory");
    }

    #[test]
    fn formatting_hides_float_noise() {
        // The canonical floating-point gotcha reads cleanly.
        assert_eq!(run("0.1+0.2="), "0.3");
        assert_eq!(run("1/3="), "0.333333333333333");
    }

    #[test]
    fn very_large_results_use_scientific_notation() {
        // 1e8 * 1e8 = 1e16, just past the fixed-notation band.
        let out = run("100000000*100000000=");
        assert_eq!(out, "1e+16");
    }

    #[test]
    fn clipboard_text_is_a_clean_number() {
        let mut eng = Engine::new();
        eng.input_dot();
        eng.input_digit(5);
        // Display echoes the typed "0.5"…
        assert_eq!(eng.display(), "0.5");
        // …and the clipboard sees a clean value too.
        assert_eq!(eng.clipboard_text(), "0.5");

        eng.clear_all();
        eng.input_digit(2);
        eng.input_op(Op::Add);
        eng.input_digit(3);
        eng.equals();
        assert_eq!(eng.clipboard_text(), "5");
    }

    #[test]
    fn paste_replaces_the_display() {
        let mut eng = Engine::new();
        eng.input_digit(9);
        eng.paste("  42.5 ");
        assert_eq!(eng.display(), "42.5");
        // Pasted value works as an operand.
        eng.input_op(Op::Mul);
        eng.input_digit(2);
        eng.equals();
        assert_eq!(eng.display(), "85");
        // Garbage is ignored.
        eng.paste("not a number");
        assert_eq!(eng.display(), "85");
    }
}
