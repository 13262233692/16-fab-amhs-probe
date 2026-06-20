use crate::event::{MoveEventType, OhtMoveEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Idle,
    SawBracket,
    InTimestamp,
    SawS,
    InStreamId,
    SawF,
    InFuncId,
    SecsHeaderFound,
    LookForOht,
    SawO,
    SawOh,
    InOhtId,
    PostOhtId,
    RecognizeVerb,
    InVerb,
    VerbRecognized,
    LookForFrom,
    SawFr,
    SawFro,
    LookForColonFrom,
    InFromNode,
    LookForTo,
    SawT,
    SawTo,
    LookForColonTo,
    InToNode,
    LookForStatus,
    Done,
}

#[derive(Debug, Default)]
struct TokenBuffer {
    year: [u8; 4],
    month: [u8; 2],
    day: [u8; 2],
    hour: [u8; 2],
    minute: [u8; 2],
    second: [u8; 2],
    millis: [u8; 3],
    ts_has_millis: bool,
    stream_id: u32,
    func_id: u32,
    oht_id: String,
    verb: String,
    from_node: String,
    to_node: String,
    status: String,
}

impl TokenBuffer {
    fn reset(&mut self) {
        self.stream_id = 0;
        self.func_id = 0;
        self.ts_has_millis = false;
        self.oht_id.clear();
        self.verb.clear();
        self.from_node.clear();
        self.to_node.clear();
        self.status.clear();
    }

    fn build_timestamp(&self) -> i64 {
        let year = bcd_to_num(&self.year) as i32;
        let month = bcd_to_num(&self.month) as u32;
        let day = bcd_to_num(&self.day) as u32;
        let hour = bcd_to_num(&self.hour) as u32;
        let minute = bcd_to_num(&self.minute) as u32;
        let second = bcd_to_num(&self.second) as u32;
        let millis = if self.ts_has_millis {
            bcd_to_num(&self.millis) as u32
        } else {
            0
        };

        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_milli_opt(hour, minute, second, millis))
            .map(|dt| dt.and_utc().timestamp_millis())
            .unwrap_or(0)
    }

    fn detect_verb_type(&self, line_lower: &str) -> MoveEventType {
        if self.status.eq_ignore_ascii_case("BLOCKED")
            || self.status.eq_ignore_ascii_case("WAITING")
            || self.status.eq_ignore_ascii_case("WAIT")
            || line_lower.contains("blocked")
            || line_lower.contains("waiting")
            || self.verb.eq_ignore_ascii_case("BLOCKED")
        {
            MoveEventType::Blocked
        } else if self.verb.eq_ignore_ascii_case("DEPART")
            || line_lower.contains("depart")
        {
            MoveEventType::Depart
        } else if self.verb.eq_ignore_ascii_case("ARRIVE")
            || line_lower.contains("arrive")
        {
            MoveEventType::Arrive
        } else if self.status.eq_ignore_ascii_case("ESTOP")
            || self.status.eq_ignore_ascii_case("EMERGENCY")
            || line_lower.contains("estop")
            || line_lower.contains("emergency")
        {
            MoveEventType::EmergencyStop
        } else {
            MoveEventType::PassThrough
        }
    }
}

fn bcd_to_num(buf: &[u8]) -> u32 {
    let mut result: u32 = 0;
    for &b in buf {
        if b >= b'0' && b <= b'9' {
            result = result * 10 + (b - b'0') as u32;
        }
    }
    result
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_whitespace(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r' || b == b'\n'
}

fn to_lower_ascii(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

pub struct SecsScanner {
    buf: TokenBuffer,
}

impl SecsScanner {
    pub fn new() -> Self {
        Self {
            buf: TokenBuffer::default(),
        }
    }

    pub fn scan_line(&mut self, line: &str) -> Option<OhtMoveEvent> {
        self.buf.reset();
        let bytes = line.as_bytes();
        let mut state = ScanState::Idle;

        let mut ts_idx: usize = 0;
        let mut ts_part: u8 = 0;
        let mut digit_acc: u32 = 0;
        let mut oht_prefix_skipped = false;
        let mut line_lower_buf = String::with_capacity(line.len());

        let mut i: usize = 0;
        while i < bytes.len() {
            let b = bytes[i];
            line_lower_buf.push(to_lower_ascii(b) as char);

            match state {
                ScanState::Idle => {
                    if b == b'[' {
                        state = ScanState::SawBracket;
                        ts_idx = 0;
                        ts_part = 0;
                    } else if b == b'S' || b == b's' {
                        state = ScanState::SawS;
                        digit_acc = 0;
                    } else if is_whitespace(b) {
                    } else {
                        let word = &line_lower_buf[line_lower_buf.len().saturating_sub(4)..];
                        if word.contains("oht") && b != b'-' && b != b'_' {
                            state = ScanState::InOhtId;
                            oht_prefix_skipped = true;
                            self.buf.oht_id.clear();
                            if is_word_char(b) {
                                self.buf.oht_id.push(b as char);
                            }
                        }
                    }
                }

                ScanState::SawBracket => {
                    if b >= b'0' && b <= b'9' && ts_idx < 4 {
                        self.buf.year[ts_idx] = b;
                        ts_idx += 1;
                        if ts_idx == 4 {
                            state = ScanState::InTimestamp;
                            ts_part = 1;
                            ts_idx = 0;
                        }
                    } else {
                        state = ScanState::Idle;
                    }
                }

                ScanState::InTimestamp => {
                    match ts_part {
                        1 => {
                            if b == b'-' || b == b'/' {
                                ts_part = 2;
                                ts_idx = 0;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.month[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        2 => {
                            if b == b'-' || b == b'/' {
                                ts_part = 3;
                                ts_idx = 0;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.day[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        3 => {
                            if is_whitespace(b) || b == b'T' || b == b't' {
                                ts_part = 4;
                                ts_idx = 0;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.hour[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        4 => {
                            if b == b':' {
                                ts_part = 5;
                                ts_idx = 0;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.hour[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        5 => {
                            if b == b':' {
                                ts_part = 6;
                                ts_idx = 0;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.minute[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        6 => {
                            if b == b'.' {
                                ts_part = 7;
                                ts_idx = 0;
                                self.buf.ts_has_millis = true;
                            } else if is_whitespace(b) || b == b']' {
                                state = ScanState::Idle;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 2 {
                                self.buf.second[ts_idx] = b;
                                ts_idx += 1;
                            }
                        }
                        7 => {
                            if is_whitespace(b) || b == b']' {
                                state = ScanState::Idle;
                            } else if b >= b'0' && b <= b'9' && ts_idx < 3 {
                                self.buf.millis[ts_idx] = b;
                                ts_idx += 1;
                            } else if b == b']' {
                                state = ScanState::Idle;
                            }
                        }
                        _ => {
                            state = ScanState::Idle;
                        }
                    }
                }

                ScanState::SawS => {
                    if b >= b'0' && b <= b'9' {
                        digit_acc = digit_acc * 10 + (b - b'0') as u32;
                        state = ScanState::InStreamId;
                    } else {
                        state = ScanState::Idle;
                    }
                }

                ScanState::InStreamId => {
                    if b >= b'0' && b <= b'9' {
                        digit_acc = digit_acc * 10 + (b - b'0') as u32;
                    } else if b == b'F' || b == b'f' {
                        self.buf.stream_id = digit_acc;
                        digit_acc = 0;
                        state = ScanState::SawF;
                    } else {
                        state = ScanState::Idle;
                    }
                }

                ScanState::SawF => {
                    if b >= b'0' && b <= b'9' {
                        digit_acc = digit_acc * 10 + (b - b'0') as u32;
                        state = ScanState::InFuncId;
                    } else {
                        state = ScanState::Idle;
                    }
                }

                ScanState::InFuncId => {
                    if b >= b'0' && b <= b'9' {
                        digit_acc = digit_acc * 10 + (b - b'0') as u32;
                    } else {
                        self.buf.func_id = digit_acc;
                        state = ScanState::SecsHeaderFound;
                    }
                }

                ScanState::SecsHeaderFound | ScanState::LookForOht => {
                    if b == b'O' || b == b'o' {
                        state = ScanState::SawO;
                    } else if is_whitespace(b) {
                        state = ScanState::SecsHeaderFound;
                    }
                }

                ScanState::SawO => {
                    if b == b'H' || b == b'h' {
                        state = ScanState::SawOh;
                    } else {
                        state = ScanState::LookForOht;
                    }
                }

                ScanState::SawOh => {
                    if b == b'T' || b == b't' {
                        state = ScanState::InOhtId;
                        oht_prefix_skipped = false;
                        self.buf.oht_id.clear();
                    } else {
                        state = ScanState::LookForOht;
                    }
                }

                ScanState::InOhtId => {
                    if !oht_prefix_skipped && (b == b'-' || b == b'_') {
                        oht_prefix_skipped = true;
                    } else if is_word_char(b) {
                        self.buf.oht_id.push(b as char);
                        state = ScanState::InOhtId;
                    } else if is_whitespace(b) && !self.buf.oht_id.is_empty() {
                        state = ScanState::PostOhtId;
                    } else if !is_word_char(b) && !self.buf.oht_id.is_empty() {
                        state = ScanState::PostOhtId;
                    }
                }

                ScanState::PostOhtId => {
                    if is_whitespace(b) {
                    } else if is_word_char(b) {
                        state = ScanState::RecognizeVerb;
                        self.buf.verb.clear();
                        self.buf.verb.push(b as char);
                    }
                }

                ScanState::RecognizeVerb => {
                    if is_word_char(b) {
                        self.buf.verb.push(b as char);
                    } else if is_whitespace(b) {
                        state = ScanState::VerbRecognized;
                    } else {
                        state = ScanState::VerbRecognized;
                    }
                }

                ScanState::VerbRecognized => {
                    if is_whitespace(b) {
                    } else if b == b'F' || b == b'f' {
                        state = ScanState::SawFr;
                    } else if b == b'T' || b == b't' {
                        state = ScanState::SawT;
                    } else if is_word_char(b) {
                        let tail = &line_lower_buf[line_lower_buf.len().saturating_sub(4)..];
                        if tail.contains("from") {
                            state = ScanState::LookForColonFrom;
                        }
                    }
                }

                ScanState::SawFr => {
                    if b == b'R' || b == b'r' {
                        state = ScanState::SawFro;
                    } else {
                        state = ScanState::VerbRecognized;
                    }
                }

                ScanState::SawFro => {
                    if b == b'O' || b == b'o' {
                        state = ScanState::LookForColonFrom;
                    } else if b == b'M' || b == b'm' {
                    } else {
                        state = ScanState::VerbRecognized;
                    }
                }

                ScanState::LookForColonFrom => {
                    if is_whitespace(b) || (b >= b'A' && b <= b'Z') || (b >= b'a' && b <= b'z') {
                    } else if b == b':' || b == b'=' {
                        state = ScanState::InFromNode;
                        self.buf.from_node.clear();
                    } else if is_word_char(b) {
                        state = ScanState::InFromNode;
                        self.buf.from_node.clear();
                        self.buf.from_node.push(b as char);
                    }
                }

                ScanState::InFromNode => {
                    if is_word_char(b) {
                        self.buf.from_node.push(b as char);
                    } else if is_whitespace(b) && !self.buf.from_node.is_empty() {
                        state = ScanState::LookForTo;
                    } else if !is_word_char(b) && !self.buf.from_node.is_empty() {
                        state = ScanState::LookForTo;
                    }
                }

                ScanState::LookForTo => {
                    if is_whitespace(b) {
                    } else if b == b'T' || b == b't' {
                        state = ScanState::SawTo;
                    } else if is_word_char(b) {
                        let tail = &line_lower_buf[line_lower_buf.len().saturating_sub(2)..];
                        if tail == "to" {
                            state = ScanState::LookForColonTo;
                        }
                    }
                }

                ScanState::SawT => {
                    if b == b'O' || b == b'o' {
                        state = ScanState::SawTo;
                    } else {
                        state = ScanState::LookForTo;
                    }
                }

                ScanState::SawTo => {
                    if is_whitespace(b) {
                        state = ScanState::LookForColonTo;
                    } else if is_word_char(b) {
                        let v = self.buf.verb.to_ascii_lowercase();
                        if v == "to" {
                            if b == b':' || b == b'=' {
                                state = ScanState::InToNode;
                                self.buf.to_node.clear();
                            } else if is_word_char(b) {
                                state = ScanState::InToNode;
                                self.buf.to_node.clear();
                                self.buf.to_node.push(b as char);
                            }
                        } else {
                            state = ScanState::LookForColonTo;
                            if b == b':' || b == b'=' {
                                state = ScanState::InToNode;
                                self.buf.to_node.clear();
                            }
                        }
                    }
                }

                ScanState::LookForColonTo => {
                    if is_whitespace(b) || (b >= b'A' && b <= b'Z') || (b >= b'a' && b <= b'z') {
                    } else if b == b':' || b == b'=' {
                        state = ScanState::InToNode;
                        self.buf.to_node.clear();
                    } else if is_word_char(b) {
                        state = ScanState::InToNode;
                        self.buf.to_node.clear();
                        self.buf.to_node.push(b as char);
                    }
                }

                ScanState::InToNode => {
                    if is_word_char(b) {
                        self.buf.to_node.push(b as char);
                    } else if !self.buf.to_node.is_empty() {
                        state = ScanState::LookForStatus;
                    }
                }

                ScanState::LookForStatus => {
                    if is_word_char(b) {
                        self.buf.status.push(b as char);
                    } else if is_whitespace(b) && !self.buf.status.is_empty() {
                        break;
                    } else if !is_word_char(b) && !self.buf.status.is_empty() {
                        break;
                    }
                }

                ScanState::Done => {
                    break;
                }

                ScanState::InVerb => {
                    state = ScanState::VerbRecognized;
                }

                ScanState::LookForFrom => {
                    state = ScanState::SawFr;
                }
            }
            i += 1;
        }

        let has_secs = self.buf.stream_id > 0 && self.buf.func_id > 0;
        let has_nodes = !self.buf.from_node.is_empty() && !self.buf.to_node.is_empty();
        let has_oht = !self.buf.oht_id.is_empty();

        if has_oht && has_nodes && has_secs {
            let timestamp = self.buf.build_timestamp();
            let line_lower = line_lower_buf;
            let event_type = self.buf.detect_verb_type(&line_lower);

            Some(OhtMoveEvent {
                timestamp,
                oht_id: std::mem::take(&mut self.buf.oht_id),
                from_node: std::mem::take(&mut self.buf.from_node),
                to_node: std::mem::take(&mut self.buf.to_node),
                event_type,
                duration_ms: 0,
            })
        } else if has_oht && has_nodes {
            let timestamp = self.buf.build_timestamp();
            let line_lower = line_lower_buf;
            let event_type = self.buf.detect_verb_type(&line_lower);

            Some(OhtMoveEvent {
                timestamp,
                oht_id: std::mem::take(&mut self.buf.oht_id),
                from_node: std::mem::take(&mut self.buf.from_node),
                to_node: std::mem::take(&mut self.buf.to_node),
                event_type,
                duration_ms: 0,
            })
        } else {
            None
        }
    }
}

impl Default for SecsScanner {
    fn default() -> Self {
        Self::new()
    }
}

pub fn has_secs_header(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(3) {
        if (bytes[i] == b'S' || bytes[i] == b's')
            && bytes[i + 1].is_ascii_digit()
        {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'F' || bytes[j] == b'f')
                && j + 1 < bytes.len()
                && bytes[j + 1].is_ascii_digit()
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

pub fn parse_timestamp_from_str(ts: &str) -> i64 {
    let bytes = ts.as_bytes();
    let mut buf = TokenBuffer::default();
    let mut state = 0;
    let mut idx = 0;
    let mut part = 0;

    for &b in bytes {
        match state {
            0 => {
                if b >= b'0' && b <= b'9' && idx < 4 {
                    buf.year[idx] = b;
                    idx += 1;
                    if idx == 4 {
                        state = 1;
                        idx = 0;
                        part = 1;
                    }
                }
            }
            1 => {
                match part {
                    1 => {
                        if b == b'-' || b == b'/' || b == b'T' || b == b't' {
                            part = 2;
                            idx = 0;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.month[idx] = b;
                            idx += 1;
                        }
                    }
                    2 => {
                        if b == b'-' || b == b'/' || b == b'T' || b == b't' {
                            part = 3;
                            idx = 0;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.day[idx] = b;
                            idx += 1;
                        }
                    }
                    3 => {
                        if is_whitespace(b) || b == b'T' || b == b't' {
                            part = 4;
                            idx = 0;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.hour[idx] = b;
                            idx += 1;
                        }
                    }
                    4 => {
                        if b == b':' {
                            part = 5;
                            idx = 0;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.hour[idx] = b;
                            idx += 1;
                        }
                    }
                    5 => {
                        if b == b':' {
                            part = 6;
                            idx = 0;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.minute[idx] = b;
                            idx += 1;
                        }
                    }
                    6 => {
                        if b == b'.' {
                            part = 7;
                            idx = 0;
                            buf.ts_has_millis = true;
                        } else if b >= b'0' && b <= b'9' && idx < 2 {
                            buf.second[idx] = b;
                            idx += 1;
                        } else {
                            break;
                        }
                    }
                    7 => {
                        if b >= b'0' && b <= b'9' && idx < 3 {
                            buf.millis[idx] = b;
                            idx += 1;
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ => break,
        }
    }
    buf.build_timestamp()
}
