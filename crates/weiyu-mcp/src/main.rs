use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::{io::Read, net::SocketAddr, path::PathBuf};
use tokio_util::sync::CancellationToken;
use weiyu_mcp::{
    admin::AdminState,
    auth,
    cookie_store::{CookieSource, CookieStore},
    error::QueryError,
    http,
    mcp::QueryServer,
    query::{self, QueryService},
    settings::{RuntimeSettings, SettingsStore},
};

#[derive(Parser)]
#[command(
    name = "weiyu-mcp",
    version,
    about = "未雨：B站查询 CLI、MCP 服务与管理台"
)]
struct Cli {
    /// 单行浏览器Cookie文件；作为可写数据目录 Cookie 的回退
    #[arg(long, global = true, env = "WEIYU_COOKIE_FILE")]
    cookie_file: Option<PathBuf>,
    /// 可写数据目录：settings.json 和扫码登录后的 Cookie 放这里
    #[arg(long, global = true, env = "WEIYU_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// 管理台访问令牌文件；也可用 WEIYU_ADMIN_TOKEN 环境变量
    #[arg(long, global = true, env = "WEIYU_ADMIN_TOKEN_FILE")]
    admin_token_file: Option<PathBuf>,
    /// 初始互动写入开关；settings.json 存在时以文件为准
    #[arg(
        long,
        global = true,
        env = "WEIYU_ALLOW_WRITE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        default_value = "false"
    )]
    allow_write: String,
    /// 初始每秒工具查询数，0表示不限制
    #[arg(long,global=true,env="WEIYU_RATE_LIMIT",default_value_t=2,value_parser=clap::value_parser!(u32).range(0..=100))]
    rate_limit: u32,
    /// 初始最大并发
    #[arg(long,global=true,env="WEIYU_MAX_CONCURRENCY",default_value_t=4,value_parser=clap::value_parser!(u32).range(1..=64))]
    max_concurrency: u32,
    /// 初始查询超时（秒）
    #[arg(long,global=true,env="WEIYU_TIMEOUT_SECS",default_value_t=45,value_parser=clap::value_parser!(u64).range(1..=600))]
    timeout_secs: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 输出工具JSON Schema；启用--allow-write时包含互动写入工具
    Tools,
    /// 调用工具，JSON结果写stdout，失败返回非零退出码
    Call {
        tool: String,
        /// JSON参数；传-从stdin读取，最多1MiB
        #[arg(long, default_value = "{}")]
        args: String,
    },
    /// 搜索视频（call search_videos的快捷方式）
    Search {
        keyword: String,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value = "totalrank")]
        order: String,
    },
    /// 查询视频详情（call parse_video的快捷方式）
    Video { url: String },
    /// 通过stdin/stdout运行MCP，日志只写stderr
    Mcp,
    /// 运行远程Streamable HTTP MCP与管理台：/mcp、/admin、/healthz
    Serve {
        #[arg(long, env = "WEIYU_LISTEN", default_value = "127.0.0.1:8787")]
        listen: SocketAddr,
        /// MCP访问令牌文件；也可用WEIYU_MCP_TOKEN环境变量，至少32字符
        #[arg(long, env = "WEIYU_MCP_TOKEN_FILE")]
        token_file: Option<PathBuf>,
        /// MCP对外域名或IP，不含协议；可重复。未指定时只允许本机Host
        #[arg(long, env = "WEIYU_ALLOWED_HOSTS", value_delimiter = ',')]
        allowed_host: Vec<String>,
        /// 允许的Origin；未指定时拒绝所有带Origin的浏览器请求
        #[arg(long, env = "WEIYU_ALLOWED_ORIGINS", value_delimiter = ',')]
        allowed_origin: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let cli = Cli::parse();
    let stdio = matches!(cli.command, Command::Mcp);
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            if !stdio {
                println!("{}", json!({"error":error}));
            }
            std::process::ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<(), QueryError> {
    let allow_write = parse_switch(&cli.allow_write, "WEIYU_ALLOW_WRITE")?;
    let settings_store = SettingsStore::new(cli.data_dir.clone());
    let defaults = RuntimeSettings {
        rate_limit: cli.rate_limit,
        max_concurrency: cli.max_concurrency,
        timeout_secs: cli.timeout_secs,
        allow_write,
    };
    defaults.validate()?;
    let settings = settings_store.load(defaults.clone())?;
    settings_store.save_if_missing(&settings);

    if matches!(cli.command, Command::Tools) {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"tools":query::tools(settings.allow_write)}))?
        );
        return Ok(());
    }

    let cookie_store = CookieStore::new(cli.data_dir.clone(), cli.cookie_file.clone());
    let (credential, cookie_source) = cookie_store.load()?;
    if cookie_source != CookieSource::None {
        eprintln!("B站 Cookie 来源: {}", cookie_source.as_str());
    }
    let service = QueryService::new(credential, settings.clone());
    let admin_token_file = cli.admin_token_file.clone();

    let (tool, args) = match cli.command {
        Command::Tools => unreachable!(),
        Command::Call { tool, args } => {
            let content = if args == "-" {
                let mut content = String::new();
                std::io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut content)
                    .map_err(|_| QueryError::invalid("无法读取stdin JSON"))?;
                content
            } else {
                args
            };
            if content.len() > 1024 * 1024 {
                return Err(QueryError::invalid("JSON参数超过1MiB"));
            }
            (tool, serde_json::from_str(&content)?)
        }
        Command::Search {
            keyword,
            page,
            order,
        } => (
            "search_videos".into(),
            json!({"keyword":keyword,"page":page,"order":order}),
        ),
        Command::Video { url } => ("parse_video".into(), json!({"url":url})),
        Command::Mcp => {
            if service.allow_write() {
                eprintln!("警告：互动写入工具已启用，连接的AI可以点赞/投币/收藏/修改稍后再看。");
            }
            let running = QueryServer(service)
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|_| QueryError::new("MCP_ERROR", "MCP stdio握手失败"))?;
            running
                .waiting()
                .await
                .map_err(|_| QueryError::new("MCP_ERROR", "MCP stdio会话异常结束"))?;
            return Ok(());
        }
        Command::Serve {
            listen,
            token_file,
            allowed_host,
            allowed_origin,
        } => {
            let token = auth::load_required_token(token_file.as_deref(), "WEIYU_MCP_TOKEN")?;
            let admin_token =
                auth::load_token_file(admin_token_file.as_deref(), "WEIYU_ADMIN_TOKEN")?;
            let admin = AdminState::new(
                service.clone(),
                settings_store.clone(),
                cookie_store.clone(),
                cli.data_dir.clone(),
                admin_token,
            );
            if cli.data_dir.is_none() {
                eprintln!("提示：未配置 WEIYU_DATA_DIR，管理台密码无法持久化。");
            }
            if service.allow_write() {
                eprintln!(
                    "警告：互动写入工具已启用，持有访问令牌的AI可以点赞/投币/收藏/修改稍后再看。"
                );
            }
            let cancel = CancellationToken::new();
            let app = http::router(
                service,
                token,
                Some(admin),
                allowed_host,
                allowed_origin,
                cancel.clone(),
            );
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .map_err(|e| QueryError::new("LISTEN_ERROR", e.to_string()))?;
            let address = listener.local_addr().unwrap_or(listen);
            eprintln!("未雨 HTTP MCP: http://{address}/mcp");
            eprintln!("未雨管理台: http://{address}/admin（首次访问设置管理密码，默认 admin）");
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    http::shutdown_signal().await;
                    cancel.cancel();
                })
                .await
                .map_err(|e| QueryError::new("SERVER_ERROR", e.to_string()))?;
            return Ok(());
        }
    };
    let result: Value = service.call(&tool, args).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn parse_switch(value: &str, name: &str) -> Result<bool, QueryError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        _ => Err(QueryError::new(
            "CONFIG_ERROR",
            format!("{name} 需为 true/false、1/0、yes/no 或 on/off"),
        )),
    }
}
