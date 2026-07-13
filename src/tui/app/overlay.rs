/// Single-variant overlay — at most one active at a time. Push = assign a
/// non-`None` variant, pop = assign `None`. No stack because business flows
/// never nest (log, config form, cancel confirmation, help cheatsheet are
/// mutually exclusive entry points).
pub enum Overlay {
    None,
    Log(LogOverlay),
    Confirm(ConfirmOverlay),
    Form(ConfigFormState),
    Help,
}

impl Overlay {
    pub fn is_active(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

pub struct LogOverlay {
    pub lines: Vec<String>,
    pub scroll: usize,
}

impl LogOverlay {
    pub fn new(lines: Vec<String>) -> Self {
        let scroll = lines.len().saturating_sub(1);
        Self { lines, scroll }
    }
}

pub enum ConfirmAction {
    CancelJob(usize),
}

pub struct ConfirmOverlay {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

pub struct ConfigFormState {
    pub field_idx: usize,
    pub key: String,
    pub hint: &'static str,
    pub value: TextInput,
}

/// Byte-safe cursor over a UTF-8 string. Borrowed pattern from cc-switch
/// form.rs — `cursor` is a byte index, `move_left/right` walk char boundaries.
#[derive(Default, Clone)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(s: &str) -> Self {
        Self {
            value: s.to_string(),
            cursor: s.len(),
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = self.value[..self.cursor].chars().next_back();
        if let Some(ch) = before {
            let new_cursor = self.cursor - ch.len_utf8();
            self.value.drain(new_cursor..self.cursor);
            self.cursor = new_cursor;
        }
    }

    pub fn move_left(&mut self) {
        if let Some(ch) = self.value[..self.cursor].chars().next_back() {
            self.cursor -= ch.len_utf8();
        }
    }

    pub fn move_right(&mut self) {
        if let Some(ch) = self.value[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_ascii_roundtrip() {
        let mut ti = TextInput::default();
        ti.insert_char('a');
        ti.insert_char('b');
        ti.insert_char('c');
        assert_eq!(ti.as_str(), "abc");
        assert_eq!(ti.cursor, 3);
        ti.backspace();
        assert_eq!(ti.as_str(), "ab");
    }

    #[test]
    fn text_input_utf8_safe() {
        let mut ti = TextInput::new("中文");
        assert_eq!(ti.cursor, 6);
        ti.move_left();
        assert_eq!(ti.cursor, 3);
        ti.backspace();
        assert_eq!(ti.as_str(), "文");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn text_input_home_end() {
        let mut ti = TextInput::new("hello");
        ti.home();
        assert_eq!(ti.cursor, 0);
        ti.end();
        assert_eq!(ti.cursor, 5);
    }
}
