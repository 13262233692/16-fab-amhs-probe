use crate::event::{MoveEventType, OhtMoveEvent};
use crate::scanner::{has_secs_header, parse_timestamp_from_str, SecsScanner};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use log::{info, warn};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);
static PARSED_BYTES: AtomicU64 = AtomicU64::new(0);

pub struct StreamParser {
    file_path: std::path::PathBuf,
    num_threads: usize,
    chunk_size: usize,
}

impl StreamParser {
    pub fn new(file_path: std::path::PathBuf, num_threads: usize, chunk_mb: usize) -> Self {
        let chunk_size = chunk_mb * 1024 * 1024;
        Self {
            file_path,
            num_threads,
            chunk_size,
        }
    }

    pub fn parse(&self) -> Result<Vec<OhtMoveEvent>> {
        let file_size = std::fs::metadata(&self.file_path)?.len();
        TOTAL_BYTES.store(file_size, Ordering::Relaxed);
        info!("文件大小: {:.2} GB", file_size as f64 / (1024.0 * 1024.0 * 1024.0));

        let is_xml = self.detect_xml()?;

        if is_xml {
            self.parse_xml_stream()
        } else {
            self.parse_text_stream()
        }
    }

    fn detect_xml(&self) -> Result<bool> {
        let mut file = File::open(&self.file_path)
            .with_context(|| format!("无法打开文件: {:?}", self.file_path))?;
        let mut buf = [0u8; 512];
        let n = file.read(&mut buf)?;
        let head = String::from_utf8_lossy(&buf[..n]);
        Ok(head.trim_start().starts_with('<'))
    }

    fn parse_text_stream(&self) -> Result<Vec<OhtMoveEvent>> {
        let file = File::open(&self.file_path)?;
        let file_size = file.metadata()?.len();
        let num_chunks = ((file_size as usize) / self.chunk_size + 1).max(self.num_threads);
        let chunk_size = (file_size as usize) / num_chunks;

        let (tx, rx): (Sender<Vec<OhtMoveEvent>>, Receiver<Vec<OhtMoveEvent>>) =
            crossbeam_channel::bounded(self.num_threads * 2);

        let file_path = self.file_path.clone();
        let num_threads = self.num_threads;

        thread::scope(|s| {
            for i in 0..num_threads {
                let tx = tx.clone();
                let fp = file_path.clone();
                s.spawn(move || {
                    let start = i * chunk_size;
                    let end = if i == num_threads - 1 {
                        file_size as usize
                    } else {
                        (i + 1) * chunk_size
                    };
                    if let Ok(events) = parse_text_chunk(&fp, start, end) {
                        let _ = tx.send(events);
                    }
                });
            }
            drop(tx);

            for _received in rx.iter() {
                PARSED_BYTES.fetch_add(chunk_size as u64, Ordering::Relaxed);
            }
        });

        let mut all_events = Vec::new();
        let file = File::open(&self.file_path)?;
        let reader = BufReader::new(file);

        let mut scanner = SecsScanner::new();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if !has_secs_header(&line) {
                continue;
            }
            if let Some(evt) = scanner.scan_line(&line) {
                all_events.push(evt);
            }
        }

        all_events.sort_by_key(|e| e.timestamp);
        info!("从文本格式中提取到 {} 条 OHT 移动事件", all_events.len());
        Ok(all_events)
    }

    fn parse_xml_stream(&self) -> Result<Vec<OhtMoveEvent>> {
        info!("检测到 XML 格式，启动流式 XML 解析器...");

        let (tx, rx): (Sender<Vec<OhtMoveEvent>>, Receiver<Vec<OhtMoveEvent>>) =
            crossbeam_channel::bounded(self.num_threads * 2);

        let chunk_size = self.chunk_size;
        let file_path = self.file_path.clone();

        let producer = thread::spawn(move || -> Result<()> {
            let mut reader = Reader::from_file(&file_path)?;
            reader.config_mut().trim_text(true);

            let mut buf = Vec::new();
            let mut batch = Vec::new();
            let mut in_oht_event = false;
            let mut current_event_data: Option<EventData> = None;
            let mut parent_timestamp: Option<String> = None;

            let mut scanner = SecsScanner::new();

            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(e)) => {
                        let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        let tag_lower = tag.to_lowercase();

                        let is_oht_tag = tag_lower.contains("oht")
                            || tag_lower.contains("transfer")
                            || tag_lower.contains("transit")
                            || tag_lower.contains("vehicle");

                        if is_oht_tag {
                            in_oht_event = true;
                            current_event_data = Some(EventData::default());
                        }

                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref()).to_lowercase();
                            let val = String::from_utf8_lossy(&attr.value).to_string();

                            if key.contains("time") || key.contains("timestamp") || key.contains("ts") {
                                parent_timestamp = Some(val.clone());
                            }

                            if let Some(ref mut ed) = current_event_data {
                                if key.contains("oht") || key.contains("vehicle") || key.contains("id") {
                                    ed.oht_id = Some(val.clone());
                                }
                                if key.contains("from") || key.contains("source") || key.contains("src") {
                                    ed.from_node = Some(val.clone());
                                }
                                if key.contains("to") || key.contains("dest") || key.contains("target") {
                                    ed.to_node = Some(val.clone());
                                }
                                if key.contains("time") || key.contains("timestamp") || key.contains("ts") {
                                    ed.timestamp = Some(val);
                                }
                            }
                        }

                        let pos = reader.buffer_position() as u64;
                        if pos - PARSED_BYTES.load(Ordering::Relaxed) > chunk_size as u64 {
                            PARSED_BYTES.store(pos, Ordering::Relaxed);
                        }
                    }
                    Ok(Event::Empty(e)) => {
                        let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        let tag_lower = tag.to_lowercase();

                        let is_oht_tag = tag_lower.contains("oht")
                            || tag_lower.contains("transfer")
                            || tag_lower.contains("transit")
                            || tag_lower.contains("vehicle");

                        if is_oht_tag {
                            let mut ed = EventData::default();

                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.as_ref()).to_lowercase();
                                let val = String::from_utf8_lossy(&attr.value).to_string();

                                if key.contains("oht") || key.contains("vehicle") || key.contains("id") {
                                    ed.oht_id = Some(val.clone());
                                }
                                if key.contains("from") || key.contains("source") || key.contains("src") {
                                    ed.from_node = Some(val.clone());
                                }
                                if key.contains("to") || key.contains("dest") || key.contains("target") {
                                    ed.to_node = Some(val.clone());
                                }
                                if key.contains("time") || key.contains("timestamp") || key.contains("ts") {
                                    ed.timestamp = Some(val.clone());
                                }
                                if key.contains("status") || key.contains("type") || key.contains("event") {
                                    ed.status = Some(val);
                                }
                            }

                            if ed.timestamp.is_none() {
                                ed.timestamp = parent_timestamp.clone();
                            }

                            if let Some(evt) = ed.into_event() {
                                batch.push(evt);
                            }

                            if batch.len() >= 1000 {
                                let _ = tx.send(batch.clone());
                                batch.clear();
                            }
                        }

                        let pos = reader.buffer_position() as u64;
                        if pos - PARSED_BYTES.load(Ordering::Relaxed) > chunk_size as u64 {
                            PARSED_BYTES.store(pos, Ordering::Relaxed);
                        }
                    }
                    Ok(Event::Text(e)) => {
                        if in_oht_event {
                            let text = match e.unescape() {
                                Ok(s) => s.to_string(),
                                Err(_) => continue,
                            };
                            if let Some(ref mut ed) = current_event_data {
                                if ed.oht_id.is_none()
                                    || ed.from_node.is_none()
                                    || ed.to_node.is_none()
                                {
                                    if let Some(scanned) = scanner.scan_line(&text) {
                                        ed.oht_id = Some(scanned.oht_id);
                                        ed.from_node = Some(scanned.from_node);
                                        ed.to_node = Some(scanned.to_node);
                                        if ed.status.is_none() {
                                            ed.status = Some(match scanned.event_type {
                                                MoveEventType::Blocked => "BLOCKED".into(),
                                                MoveEventType::Depart => "DEPART".into(),
                                                MoveEventType::Arrive => "ARRIVE".into(),
                                                MoveEventType::EmergencyStop => "ESTOP".into(),
                                                MoveEventType::PassThrough => "TRANSIT".into(),
                                            });
                                        }
                                    }
                                }
                                if ed.timestamp.is_none() {
                                    ed.timestamp = extract_timestamp_inline(&text);
                                }
                            }
                        }
                    }
                    Ok(Event::End(e)) => {
                        let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                        if in_oht_event
                            && (tag.contains("oht")
                                || tag.contains("transfer")
                                || tag.contains("transit")
                                || tag.contains("vehicle"))
                        {
                            if let Some(ed) = current_event_data.take() {
                                if let Some(evt) = ed.into_event() {
                                    batch.push(evt);
                                }
                            }
                            in_oht_event = false;
                        }

                        if batch.len() >= 1000 {
                            let _ = tx.send(batch.clone());
                            batch.clear();
                        }
                    }
                    Ok(Event::Eof) => {
                        if !batch.is_empty() {
                            let _ = tx.send(batch);
                        }
                        break;
                    }
                    Err(e) => {
                        warn!("XML 解析错误: {:?}", e);
                        break;
                    }
                    _ => {}
                }
            }
            Ok(())
        });

        let mut all_events = Vec::new();
        while let Ok(batch) = rx.recv() {
            all_events.extend(batch);
        }

        producer.join().unwrap()?;

        all_events.sort_by_key(|e| e.timestamp);
        info!("从 XML 格式中提取到 {} 条 OHT 移动事件", all_events.len());
        Ok(all_events)
    }
}

fn extract_timestamp_inline(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(19) {
        if bytes[i] >= b'0' && bytes[i] <= b'9' {
            let mut j = i;
            while j < bytes.len() && j < i + 24 && j < i.saturating_add(24) {
                let b = bytes[j];
                if !matches!(b, b'0'..=b'9' | b'-' | b'/' | b'T' | b't' | b' ' | b':' | b'.') {
                    break;
                }
                j += 1;
            }
            let candidate = &text[i..j];
            if candidate.len() >= 16 {
                let mut digit_count = 0u8;
                for c in candidate.bytes() {
                    if c.is_ascii_digit() {
                        digit_count += 1;
                    }
                }
                if digit_count >= 12 {
                    return Some(candidate.to_string());
                }
            }
        }
        i += 1;
    }
    None
}

#[derive(Default)]
struct EventData {
    oht_id: Option<String>,
    from_node: Option<String>,
    to_node: Option<String>,
    timestamp: Option<String>,
    status: Option<String>,
}

impl EventData {
    fn into_event(self) -> Option<OhtMoveEvent> {
        let oht_id = self.oht_id?;
        let from_node = self.from_node?;
        let to_node = self.to_node?;
        let ts = self.timestamp.unwrap_or_default();

        let timestamp = parse_timestamp_from_str(&ts);

        let event_type = match self.status.as_deref() {
            Some(s) => {
                let sl = s.to_ascii_lowercase();
                if sl == "blocked" || sl == "waiting" || sl == "wait" {
                    MoveEventType::Blocked
                } else if sl == "depart" {
                    MoveEventType::Depart
                } else if sl == "arrive" {
                    MoveEventType::Arrive
                } else if sl == "estop" || sl == "emergency" {
                    MoveEventType::EmergencyStop
                } else {
                    MoveEventType::PassThrough
                }
            }
            _ => MoveEventType::PassThrough,
        };

        Some(OhtMoveEvent {
            timestamp,
            oht_id,
            from_node,
            to_node,
            event_type,
            duration_ms: 0,
        })
    }
}

fn parse_text_chunk(
    file_path: &std::path::Path,
    start: usize,
    end: usize,
) -> Result<Vec<OhtMoveEvent>> {
    let mut file = File::open(file_path)?;
    file.seek(SeekFrom::Start(start as u64))?;

    let bytes_to_read = end.saturating_sub(start);
    let reader = BufReader::new(file.take(bytes_to_read as u64));

    let mut scanner = SecsScanner::new();
    let mut events = Vec::new();

    for line in BufReader::new(reader).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if !has_secs_header(&line) {
            continue;
        }
        if let Some(evt) = scanner.scan_line(&line) {
            events.push(evt);
        }
    }

    Ok(events)
}
