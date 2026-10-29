//! Bounded plain lines for matchers, independent of transport chunk boundaries.
//! The terminal owns full VT rendering; this decoder strips controls and progress-line rewrites.

const MAX_LINE: usize = 64 * 1024;
const MAX_CONTROL: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    LongLine,
    LongControl,
    InvalidUtf8,
    IncompleteControl,
    Gap,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    Line(&'a str),
    Fault(Fault),
}
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    Plain,
    Start,
    Csi,
    Osc,
    OscEnd,
}

#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    escape: Escape,
    control: usize,
    discard: bool,
    carriage: bool,
}

impl Decoder {
    fn emit_line(&mut self, emit: &mut impl FnMut(Event<'_>)) {
        if !self.discard {
            match std::str::from_utf8(&self.line) {
                Ok(line) => emit(Event::Line(line)),
                Err(_) => emit(Event::Fault(Fault::InvalidUtf8)),
            }
        }
        self.line.clear();
        self.discard = false;
        self.carriage = false;
    }
    fn fail(&mut self, fault: Fault, emit: &mut impl FnMut(Event<'_>)) {
        self.line.clear();
        self.discard = true;
        self.carriage = false;
        self.escape = Escape::Plain;
        self.control = 0;
        emit(Event::Fault(fault));
    }

    /// A lost output chunk invalidates the partial line. Never interpret its remaining suffix as a new diagnostic.
    pub fn gap(&mut self, mut emit: impl FnMut(Event<'_>)) {
        self.fail(Fault::Gap, &mut emit);
    }

    pub fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(Event<'_>)) {
        for &byte in bytes {
            if self.discard {
                if byte == b'\n' {
                    self.emit_line(&mut emit);
                }
                continue;
            }
            if self.escape != Escape::Plain {
                self.control += 1;
                if self.control > MAX_CONTROL {
                    self.fail(Fault::LongControl, &mut emit);
                    continue;
                }
                match self.escape {
                    Escape::Start => {
                        self.escape = match byte {
                            b'[' => Escape::Csi,
                            b']' => Escape::Osc,
                            _ => Escape::Plain,
                        }
                    }
                    Escape::Csi if (0x40..=0x7e).contains(&byte) => {
                        // Erasure/cursor-to-column controls must not glue a stale prefix to a new problem.
                        if matches!(byte, b'K' | b'J' | b'G' | b'H' | b'f') {
                            self.line.clear();
                            self.carriage = false;
                        }
                        self.escape = Escape::Plain;
                    }
                    Escape::Osc if byte == 7 => self.escape = Escape::Plain,
                    Escape::Osc if byte == 27 => self.escape = Escape::OscEnd,
                    Escape::OscEnd => {
                        self.escape = match byte {
                            b'\\' | 7 => Escape::Plain,
                            27 => Escape::OscEnd,
                            _ => Escape::Osc,
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match byte {
                b'\n' => self.emit_line(&mut emit),
                b'\r' => self.carriage = true,
                27 => {
                    self.escape = Escape::Start;
                    self.control = 0;
                }
                8 => {
                    while let Some(last) = self.line.pop() {
                        if last & 0xc0 != 0x80 {
                            break;
                        }
                    }
                }
                b'\t' | 0x20..=0x7e | 0x80..=0xff => {
                    if self.carriage {
                        self.line.clear();
                        self.carriage = false;
                    }
                    if self.line.len() == MAX_LINE {
                        self.fail(Fault::LongLine, &mut emit);
                    } else {
                        self.line.push(byte);
                    }
                }
                _ => {}
            }
        }
    }

    /// Flush before delivering process exit, so a final newline is optional.
    pub fn finish(&mut self, mut emit: impl FnMut(Event<'_>)) {
        if self.escape != Escape::Plain {
            self.fail(Fault::IncompleteControl, &mut emit);
        } else if !self.line.is_empty() {
            self.emit_line(&mut emit);
        }
        *self = Self::default();
    }
}

#[cfg(test)]
#[path = "text_stream_tests.rs"]
mod tests;
