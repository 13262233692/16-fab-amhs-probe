use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum MergeMode {
    #[value(name = "semantic")]
    Semantic,
    #[value(name = "max")]
    MaxNodes,
    #[value(name = "none")]
    None,
}

#[derive(Parser, Debug)]
#[command(
    name = "amhs-probe",
    version,
    about = "SECS/GEM AMHS OHT 轨道拥堵探针 — 半导体晶圆厂天车系统分析工具",
    long_about = "amhs-probe 是一款专供半导体晶圆厂使用的硬核思考过程探针 CLI 工具。\n\
                  通过确定性状态机流式读取器极速解析包含数十 GB 历史通信记录的 SECS/GEM 协议\n\
                  XML/文本混合转储文件（彻底废弃正则匹配，杜绝灾难性回溯），精确提取 OHT 天车\n\
                  在轨道节点间的移动事件序列，通过节点降采样策略构建有向权重图并分析车间\n\
                  最拥堵的 Top 10 轨道交叉路口。"
)]
pub struct Args {
    #[arg(
        short,
        long,
        help = "SECS/GEM 转储文件路径（支持 XML/文本混合格式）",
        value_name = "FILE"
    )]
    pub input: PathBuf,

    #[arg(
        short = 'n',
        long,
        default_value = "10",
        help = "显示拥堵排名前 N 的轨道交叉路口"
    )]
    pub top: usize,

    #[arg(
        short = 't',
        long,
        default_value_t = num_cpus::get(),
        help = "并行解析线程数"
    )]
    pub threads: usize,

    #[arg(
        short = 'c',
        long,
        default_value = "64",
        help = "流式读取块大小（MB）"
    )]
    pub chunk_mb: usize,

    #[arg(
        short = 'm',
        long,
        value_enum,
        default_value_t = MergeMode::Semantic,
        help = "节点降采样合并策略"
    )]
    pub merge: MergeMode,

    #[arg(
        short = 'b',
        long,
        default_value = "10",
        help = "语义桶区间大小（编号 0-9 → IX-000-009）"
    )]
    pub bucket_interval: u32,

    #[arg(
        short = 'x',
        long,
        default_value = "10000",
        help = "最大节点数（超过则合并低频节点到 OVERFLOW-MERGED）"
    )]
    pub max_nodes: usize,

    #[arg(long, help = "导出有向图数据为 JSON 格式")]
    pub export_graph: Option<PathBuf>,

    #[arg(long, help = "显示详细解析过程日志")]
    pub verbose: bool,
}
