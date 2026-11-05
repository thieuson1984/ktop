//! Decode whole control sequences; pasted content never becomes a shortcut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Char(u8),
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
    Clear,
    Quit,
    Suspend,
}
#[derive(Default)]
pub struct Decoder {
    state: State,
}
#[derive(Default)]
enum State {
    #[default]
    Normal,
    Escape,
    Csi {
        data: [u8; 16],
        len: usize,
    },
    Ss3,
    Discard,
    Paste(usize),
}
impl Decoder {
    pub fn pasting(&self) -> bool {
        matches!(self.state, State::Paste(_))
    }
    pub fn waiting_escape(&self) -> bool {
        matches!(self.state, State::Escape)
    }
    pub fn finish_escape(&mut self) -> Option<Key> {
        if self.waiting_escape() {
            self.state = State::Normal;
            Some(Key::Escape)
        } else {
            None
        }
    }
    pub fn feed(&mut self, byte: u8) -> [Option<Key>; 2] {
        if matches!(byte, 3 | 4) {
            self.state = State::Normal;
            return [Some(Key::Quit), None];
        }
        if byte == 27 && matches!(self.state, State::Csi { .. } | State::Discard | State::Ss3) {
            self.state = State::Escape;
            return [None, None];
        }
        let mut second = None;
        let result = match &mut self.state {
            State::Normal => self.normal(byte),
            State::Escape => match byte {
                b'[' => {
                    self.state = State::Csi {
                        data: [0; 16],
                        len: 0,
                    };
                    None
                }
                b'O' => {
                    self.state = State::Ss3;
                    None
                }
                _ => {
                    self.state = State::Normal;
                    second = self.normal(byte);
                    Some(Key::Escape)
                }
            },
            State::Ss3 => {
                self.state = State::Normal;
                match byte {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'H' => Some(Key::Home),
                    b'F' => Some(Key::End),
                    _ => None,
                }
            }
            State::Csi { data, len } => {
                if (0x40..=0x7e).contains(&byte) {
                    let params = &data[..*len];
                    let key = match byte {
                        b'A' => Some(Key::Up),
                        b'B' => Some(Key::Down),
                        b'H' => Some(Key::Home),
                        b'F' => Some(Key::End),
                        b'~' => match params {
                            b"1" | b"7" => Some(Key::Home),
                            b"4" | b"8" => Some(Key::End),
                            b"5" => Some(Key::PageUp),
                            b"6" => Some(Key::PageDown),
                            _ => None,
                        },
                        _ => None,
                    };
                    let pasted = byte == b'~' && params == b"200";
                    self.state = if pasted {
                        State::Paste(0)
                    } else {
                        State::Normal
                    };
                    key
                } else if *len < data.len() {
                    data[*len] = byte;
                    *len += 1;
                    None
                } else {
                    self.state = State::Discard;
                    None
                }
            }
            State::Discard => {
                if (0x40..=0x7e).contains(&byte) {
                    self.state = State::Normal;
                }
                None
            }
            State::Paste(matched) => {
                const END: &[u8] = b"\x1b[201~";
                if byte == END[*matched] {
                    *matched += 1;
                    if *matched == END.len() {
                        self.state = State::Normal;
                    }
                } else {
                    *matched = usize::from(byte == END[0]);
                }
                None
            }
        };
        [result, second]
    }
    fn normal(&mut self, byte: u8) -> Option<Key> {
        match byte {
            27 => {
                self.state = State::Escape;
                None
            }
            3 | 4 => Some(Key::Quit),
            26 => Some(Key::Suspend),
            b'\r' | b'\n' => Some(Key::Enter),
            8 | 127 => Some(Key::Backspace),
            21 => Some(Key::Clear),
            32..=126 => Some(Key::Char(byte)),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn decode(bytes: &[u8]) -> Vec<Key> {
        let mut d = Decoder::default();
        bytes
            .iter()
            .flat_map(|b| d.feed(*b).into_iter().flatten())
            .collect()
    }
    #[test]
    fn csi_parameters_are_not_shortcuts() {
        assert_eq!(decode(b"\x1b[1;2Aq"), [Key::Up, Key::Char(b'q')]);
        assert_eq!(decode(b"\x1b[15~q"), [Key::Char(b'q')]);
        assert_eq!(decode(b"\x1bq"), [Key::Escape, Key::Char(b'q')]);
        assert_eq!(decode(b"\x15"), [Key::Clear]);
        assert_eq!(decode(b"\x1b\x03"), [Key::Quit]);
    }
    #[test]
    fn bracketed_paste_and_oversized_sequences_cannot_confirm_actions() {
        assert_eq!(
            decode(b"\x1b[200~ty\x1b[1;2Ar\x1b[201~q"),
            [Key::Char(b'q')]
        );
        assert_eq!(
            decode(b"\x1b[1111111111111111111111111111~q"),
            [Key::Char(b'q')]
        );
        assert_eq!(
            decode(b"\x1b[\x1b[200~3Kpayload\x1b[201~q"),
            [Key::Char(b'q')]
        );
        let mut d = Decoder::default();
        assert_eq!(d.feed(27), [None, None]);
        assert_eq!(d.finish_escape(), Some(Key::Escape));
        assert_eq!(d.feed(b'q'), [Some(Key::Char(b'q')), None]);
    }
}
