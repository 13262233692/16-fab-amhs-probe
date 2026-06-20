use crate::event::{MoveEventType, OhtMoveEvent};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use log::{info, warn};
use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::fs::File;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

const SECS_MESSAGE_PATTERN: &str =
    r"(?i)S(?:\d+)F(?:\d+)";
const OHT_MOVE_PATTERN: &str =
    r"(?i)OHT[_\-]?(\w+)\s+(?:MOVE|TRANSIT|PASSING|DEPART|ARRIVE)\s+FROM\s+(?:NODE\s*)?[:=]\s*([A-Za-z0-9_\-]+)\s+TO\s+(?:NODE\s*)?[:=]\s*([A-Za-z0-9_\-]+)";
const TIMESTAMP_PATTERN: &str =
    r"(\d{4}[-/]\d{2}[-/]\d{2}\s+\d{2}:\d{2}:\d{2}(?:\.\d+)?)";

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

        let oht_re = Regex::new(OHT_MOVE_PATTERN)?;
        let ts_re = Regex::new(TIMESTAMP_PATTERN)?;
        let secs_re = Regex::new(SECS_MESSAGE_PATTERN)?;

        for line in reader.lines() {
            let line = line?;
            if !secs_re.is_match(&line) {
                continue;
            }
            if let Some(evt) = extract_move_event(&line, &oht_re, &ts_re) {
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

            let oht_re = Regex::new(OHT_MOVE_PATTERN)?;
            let ts_re = Regex::new(TIMESTAMP_PATTERN)?;

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
                            let text = e.unescape().unwrap_or_default().to_string();
                            if let Some(ref mut ed) = current_event_data {
                                if let Some(caps) = oht_re.captures(&text) {
                                    ed.oht_id = Some(caps[1].to_string());
                                    ed.from_node = Some(caps[2].to_string());
                                    ed.to_node = Some(caps[3].to_string());
                                }
                                if ed.timestamp.is_none() {
                                    if let Some(caps) = ts_re.captures(&text) {
                                        ed.timestamp = Some(caps[1].to_string());
                                    }
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

        let timestamp = parse_timestamp(&ts);

        let event_type = match self.status.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("blocked") || s.eq_ignore_ascii_case("waiting") => {
                MoveEventType::Blocked
            }
            Some(s) if s.eq_ignore_ascii_case("depart") => MoveEventType::Depart,
            Some(s) if s.eq_ignore_ascii_case("arrive") => MoveEventType::Arrive,
            Some(s) if s.eq_ignore_ascii_case("estop") || s.eq_ignore_ascii_case("emergency") => {
                MoveEventType::EmergencyStop
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

fn parse_timestamp(ts: &str) -> i64 {
    let ts_clean = ts.replace('T', " ");
    let formats = [
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y/%m/%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
    ];

    for fmt in &formats {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&ts_clean, fmt) {
            return dt.and_utc().timestamp_millis();
        }
    }

    ts.parse::<i64>().unwrap_or(0)
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

    let oht_re = Regex::new(OHT_MOVE_PATTERN)?;
    let ts_re = Regex::new(TIMESTAMP_PATTERN)?;
    let secs_re = Regex::new(SECS_MESSAGE_PATTERN)?;

    let mut events = Vec::new();

    for line in BufReader::new(reader).lines() {
        let line = line?;
        if !secs_re.is_match(&line) {
            continue;
        }
        if let Some(evt) = extract_move_event(&line, &oht_re, &ts_re) {
            events.push(evt);
        }
    }

    Ok(events)
}

fn extract_move_event(line: &str, oht_re: &Regex, ts_re: &Regex) -> Option<OhtMoveEvent> {
    let caps = oht_re.captures(line)?;
    let oht_id = caps[1].to_string();
    let from_node = caps[2].to_string();
    let to_node = caps[3].to_string();

    let timestamp = if let Some(ts_caps) = ts_re.captures(line) {
        parse_timestamp(&ts_caps[1])
    } else {
        0
    };

    let event_type = if line.to_lowercase().contains("blocked") || line.to_lowercase().contains("wait") {
        MoveEventType::Blocked
    } else if line.to_lowercase().contains("depart") {
        MoveEventType::Depart
    } else if line.to_lowercase().contains("arrive") {
        MoveEventType::Arrive
    } else if line.to_lowercase().contains("estop") || line.to_lowercase().contains("emergency") {
        MoveEventType::EmergencyStop
    } else {
        MoveEventType::PassThrough
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
