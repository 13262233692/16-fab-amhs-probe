use crate::cli::{Args, MergeMode};
use crate::digraph::{IntersectionCongestion, NodeDownsampler, TrackGraph};
use crate::event::OhtMoveEvent;
use crate::parser::StreamParser;
use anyhow::Result;
use colored::*;
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Cell, Color, Table};
use indicatif::{ProgressBar, ProgressStyle};
use log::info;
use std::io::Write;
use std::time::Instant;

pub fn run_probe(args: Args) -> Result<()> {
    print_banner();

    println!(
        "  {} {}",
        "输入文件:".cyan(),
        args.input.display()
    );
    println!(
        "  {} {}{}",
        "并行线程:".cyan(),
        args.threads.to_string().yellow(),
        " 线程".dimmed()
    );
    println!(
        "  {} {}{}",
        "读取块大小:".cyan(),
        args.chunk_mb.to_string().yellow(),
        " MB".dimmed()
    );
    println!(
        "  {} {}",
        "降采样策略:".cyan(),
        match args.merge {
            MergeMode::Semantic => format!("语义桶 (间隔:{})", args.bucket_interval).green().to_string(),
            MergeMode::MaxNodes => format!("上限节点 (max:{})", args.max_nodes).magenta().to_string(),
            MergeMode::None => "禁用".dimmed().to_string(),
        }
    );
    println!(
        "  {} {}",
        "解析引擎:".cyan(),
        "DFA 确定性状态机 (无 regex, 零回溯)".green().to_string()
    );
    println!();

    let start = Instant::now();

    info!("正在启动流式解析器...");
    let parser = StreamParser::new(args.input.clone(), args.threads, args.chunk_mb);

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg} [{elapsed_precise}] {per_sec}",
        )
        .unwrap(),
    );
    pb.set_message("解析 SECS/GEM 转储数据...");

    let events = parser.parse()?;
    pb.finish_with_message("解析完成");

    let parse_time = start.elapsed();
    println!();

    print_parse_summary(&events, parse_time);

    println!();
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );
    println!(
        "  {}  {}",
        "▶".red(),
        "构建有向权重图模型...".bold().white()
    );
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );

    let graph_start = Instant::now();

    let downsampler = match args.merge {
        MergeMode::Semantic => NodeDownsampler::with_semantic_buckets(args.bucket_interval),
        MergeMode::MaxNodes => NodeDownsampler::with_max_nodes(args.max_nodes),
        MergeMode::None => NodeDownsampler::no_merge(),
    };

    let mut graph = TrackGraph::with_downsampler(downsampler);
    graph.build_from_events(&events);
    let graph_time = graph_start.elapsed();

    let stats = graph.graph_stats();

    print_graph_stats(&stats, graph_time);

    println!();
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );
    println!(
        "  {}  {}",
        "▶".red(),
        format!("拥堵排名 Top {} 轨道交叉路口", args.top)
            .bold()
            .white()
    );
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );
    println!();

    let congested = graph.top_congested_intersections(args.top);
    print_congestion_table(&congested);

    if let Some(ref export_path) = args.export_graph {
        let json = graph.export_json();
        let mut file = std::fs::File::create(export_path)?;
        file.write_all(serde_json::to_string_pretty(&json)?.as_bytes())?;
        println!(
            "\n  {} 图数据已导出至: {}",
            "✔".green(),
            export_path.display()
        );
    }

    println!();
    let total_time = start.elapsed();
    println!(
        "  {} 总耗时: {:.2}s (解析: {:.2}s, 建图: {:.2}s)",
        "⏱".yellow(),
        total_time.as_secs_f64(),
        parse_time.as_secs_f64(),
        graph_time.as_secs_f64(),
    );
    println!(
        "  {} 处理吞吐: {:.2} 事件/s",
        "⚡".yellow(),
        events.len() as f64 / parse_time.as_secs_f64().max(0.001),
    );

    Ok(())
}

fn print_banner() {
    println!();
    println!(
        "  {}",
        r#"
  ╔═══════════════════════════════════════════════════════════════╗
  ║                                                               ║
  ║   █████╗ ███╗   ██╗██╗  ██╗                                 ║
  ║  ██╔══██╗████╗  ██║╚██╗██╔╝   ██████╗ ███████╗███████╗     ║
  ║  ███████║██╔██╗ ██║ ╚███╔╝    ██╔════╝ ██╔════╝██╔════╝     ║
  ║  ██╔══██║██║╚██╗██║ ██╔██╗    ██║  ███╗█████╗  ███████╗     ║
  ║  ██║  ██║██║ ╚████║██╔╝ ██╗   ██║   ██║██╔══╝  ╚════██║     ║
  ║  ╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝   ╚██████╔╝███████╗███████║     ║
  ║       PROBE v0.1.0           ╚══════╝ ╚════════╝╚══════╝     ║
  ║                                                               ║
  ║   SECS/GEM AMHS OHT 轨道拥堵探针                              ║
  ║   半导体晶圆厂天车系统分析工具                                  ║
  ║                                                               ║
  ╚═══════════════════════════════════════════════════════════════╝
"#
        .bright_cyan()
    );
}

fn print_parse_summary(events: &[OhtMoveEvent], elapsed: std::time::Duration) {
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );
    println!(
        "  {}  {}",
        "▶".green(),
        "SECS/GEM 数据解析摘要".bold().white()
    );
    println!(
        "  {}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            .cyan()
            .dimmed()
    );
    println!();

    let depart_count = events
        .iter()
        .filter(|e| e.event_type == crate::event::MoveEventType::Depart)
        .count();
    let arrive_count = events
        .iter()
        .filter(|e| e.event_type == crate::event::MoveEventType::Arrive)
        .count();
    let pass_count = events
        .iter()
        .filter(|e| e.event_type == crate::event::MoveEventType::PassThrough)
        .count();
    let blocked_count = events
        .iter()
        .filter(|e| e.event_type == crate::event::MoveEventType::Blocked)
        .count();

    let unique_ohts: std::collections::HashSet<_> = events.iter().map(|e| &e.oht_id).collect();
    let unique_nodes: std::collections::HashSet<_> = events
        .iter()
        .flat_map(|e| [&e.from_node, &e.to_node])
        .collect();

    println!(
        "  {} {} 条 OHT 移动事件",
        "✔".green(),
        events.len().to_string().yellow()
    );
    println!(
        "  {} {} 台天车 (OHT)",
        "✔".green(),
        unique_ohts.len().to_string().yellow()
    );
    println!(
        "  {} {} 个轨道节点",
        "✔".green(),
        unique_nodes.len().to_string().yellow()
    );
    println!(
        "  {} 离站:{} 到站:{} 通过:{} 阻塞:{}",
        "✔".green(),
        depart_count.to_string().cyan(),
        arrive_count.to_string().cyan(),
        pass_count.to_string().cyan(),
        blocked_count.to_string().red(),
    );
    println!(
        "  {} 解析耗时: {:.2}s",
        "✔".green(),
        elapsed.as_secs_f64()
    );
}

fn print_graph_stats(stats: &crate::digraph::GraphStats, elapsed: std::time::Duration) {
    println!();
    println!(
        "  {} {} 个节点, {} 条有向边",
        "✔".green(),
        stats.node_count.to_string().yellow(),
        stats.edge_count.to_string().yellow()
    );
    println!(
        "  {} 总流量: {} 次, 阻塞: {} 次 ({:.1}%)",
        "✔".green(),
        stats.total_flow.to_string().yellow(),
        stats.total_blocked.to_string().red(),
        if stats.total_flow > 0 {
            stats.total_blocked as f64 / stats.total_flow as f64 * 100.0
        } else {
            0.0
        }
    );
    println!(
        "  {} {} 台天车在线",
        "✔".green(),
        stats.unique_ohts.to_string().yellow()
    );
    println!(
        "  {} 建图耗时: {:.2}s",
        "✔".green(),
        elapsed.as_secs_f64()
    );
}

fn print_congestion_table(congested: &[IntersectionCongestion]) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);

    table.set_header(vec![
        Cell::new("排名").fg(Color::Cyan),
        Cell::new("交叉路口编号").fg(Color::Cyan),
        Cell::new("通过流量").fg(Color::Cyan),
        Cell::new("阻塞次数").fg(Color::Cyan),
        Cell::new("阻塞率").fg(Color::Cyan),
        Cell::new("连接边数").fg(Color::Cyan),
        Cell::new("活跃天车").fg(Color::Cyan),
    ]);

    for (i, c) in congested.iter().enumerate() {
        let rank = i + 1;
        let rank_cell = match rank {
            1 => Cell::new(&rank).fg(Color::Red),
            2 => Cell::new(&rank).fg(Color::Rgb {
                r: 255,
                g: 165,
                b: 0,
            }),
            3 => Cell::new(&rank).fg(Color::Yellow),
            _ => Cell::new(&rank),
        };

        let blocking_color = if c.blocking_ratio > 0.3 {
            Color::Red
        } else if c.blocking_ratio > 0.1 {
            Color::Yellow
        } else {
            Color::Green
        };

        table.add_row(vec![
            rank_cell,
            Cell::new(&c.intersection_id).fg(Color::White),
            Cell::new(&c.total_flow).fg(Color::Yellow),
            Cell::new(&c.blocked_count).fg(if c.blocked_count > 0 {
                Color::Red
            } else {
                Color::Green
            }),
            Cell::new(format!("{:.1}%", c.blocking_ratio * 100.0)).fg(blocking_color),
            Cell::new(&c.connected_edges).fg(Color::Cyan),
            Cell::new(&c.active_ohts).fg(Color::Magenta),
        ]);
    }

    println!("  {}", table);

    if !congested.is_empty() {
        println!();
        println!(
            "  {} 最拥堵交叉路口: {} (流量: {}, 阻塞率: {:.1}%)",
            "⚠".red().bold(),
            congested[0].intersection_id.yellow().bold(),
            congested[0].total_flow.to_string().yellow().bold(),
            congested[0].blocking_ratio * 100.0,
        );
    }
}
