# 未雨 MCP（weiyu-mcp）

独立的 B 站查询 MCP 服务，面向 Linux 服务器部署，供 Claude Desktop、ZCode、Cursor、Codex 等 AI 客户端使用。

- 35 个**只读**查询工具：搜索、解析、评论、字幕、弹幕、专栏、UP 主、榜单、个人列表。
- 5 个**可选互动写入**工具：点赞、投币、收藏、加入/移出稍后再看。默认不注册，必须显式开启。
- 三种入口共用同一份工具定义：JSON CLI、stdio MCP、Streamable HTTP MCP。
- 单个静态 musl 二进制，无 Tauri / WebView / FFmpeg / Node / Python / SQLite 依赖。
- 部署方式：systemd 或 Docker（distroless、非 root、只读根文件系统）。

`crates/weiyu-core` 从主仓库 [mkktop/bilbili_copy](https://github.com/mkktop/bilbili_copy) 的 `crates/weiyu-core` 复制而来，原因和同步方式见 [UPSTREAM.md](UPSTREAM.md)。

## 快速开始

```bash
cargo build --release -p weiyu-mcp

# 查看工具
./target/release/weiyu-mcp tools

# 本地 stdio MCP
./target/release/weiyu-mcp --cookie-file /etc/weiyu/bilibili.cookie mcp

# 远程 HTTP MCP
openssl rand -hex 32 > mcp-token
./target/release/weiyu-mcp serve \
  --listen 127.0.0.1:8787 \
  --token-file ./mcp-token \
  --allowed-host mcp.example.com
```

JSON 命令：

```bash
weiyu-mcp call parse_url --args '{"url":"BV17x411w7KC"}'
weiyu-mcp call search_videos --args '{"keyword":"Rust 入门","page":1}'
weiyu-mcp video 'https://www.bilibili.com/video/BV17x411w7KC'
weiyu-mcp search 'Rust 入门' --page 1
```

## Cookie

部分公开查询可匿名使用；搜索、字幕、官方 AI 总结、个人列表需要 Cookie。

服务器管理员在自己已登录 B 站的浏览器开发者工具中复制 **Cookie 请求头的值**，保存为单行 UTF-8 文件：

```bash
umask 077
printf '%s' 'SESSDATA=...; bili_jct=...; buvid3=...; DedeUserID=...; ac_time_value=...' > /etc/weiyu/bilibili.cookie
```

```bash
export WEIYU_COOKIE_FILE=/etc/weiyu/bilibili.cookie
weiyu-mcp call get_login_status
weiyu-mcp call get_current_user
```

- 文件只在启动时读取；更新 Cookie 后重启服务。
- 空文件表示匿名运行。
- 不要使用桌面版的 Windows DPAPI `credentials.json`，它不能跨系统迁移。
- 服务不会把 Cookie 返回给 AI。

## 远程 HTTP MCP

HTTP 服务始终需要至少 32 字符的 Bearer 令牌：

```bash
umask 077
openssl rand -hex 32 > mcp-token
weiyu-mcp serve \
  --listen 127.0.0.1:8787 \
  --token-file ./mcp-token \
  --allowed-host mcp.example.com \
  --allowed-host 127.0.0.1
```

- MCP 路径：`/mcp`
- 健康检查：`/healthz`（不需要令牌）
- Host 必须通过 `--allowed-host` 或 `WEIYU_ALLOWED_HOSTS` 指定；未指定时只允许本机 Host。
- 带 `Origin` 的浏览器请求默认拒绝，可用 `--allowed-origin` 放行。
- 默认每秒 2 次工具查询、最多 4 个并发、45 秒超时；可用 `WEIYU_RATE_LIMIT`、`WEIYU_MAX_CONCURRENCY`、`WEIYU_TIMEOUT_SECS` 调整。

客户端配置（Streamable HTTP）：

```json
{
  "mcpServers": {
    "weiyu": {
      "type": "http",
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer <访问令牌>" }
    }
  }
}
```

stdio 客户端：

```json
{
  "mcpServers": {
    "weiyu": {
      "command": "/usr/local/bin/weiyu-mcp",
      "args": ["--cookie-file", "/etc/weiyu/bilibili.cookie", "mcp"]
    }
  }
}
```

## 互动写入（默认关闭）

写入工具会修改真实 B 站账号。默认不注册，AI 在 `tools/list` 中看不到它们。

开启方式（二选一）：

```bash
weiyu-mcp --allow-write serve ...
WEIYU_ALLOW_WRITE=true weiyu-mcp serve ...
```

开启后增加 5 个工具：

| 工具 | 作用 | 注意事项 |
| --- | --- | --- |
| `like_video` | 点赞 / 取消点赞 | 点赞列表他人可见 |
| `coin_video` | 投币，可选同时点赞 | 消耗账号真实硬币，`multiply` 为 1 或 2 |
| `favorite_video` | 加入指定收藏夹 | `folder_id` 来自 `get_favorite_folders` |
| `add_watch_later` | 加入稍后再看 | 修改个人列表 |
| `remove_watch_later` | 移出稍后再看 | 修改个人列表 |

写入还要求 Cookie 包含 `bili_jct`（CSRF 字段）。只有 `SESSDATA` 的 Cookie 只能读。

建议：

- 使用专用账号，不要用主账号。
- 只在必要时开启；多个 AI 客户端共用一个令牌时尤其谨慎。
- 投币等消耗性操作应在 AI 客户端侧要求人工确认。

## 工具清单

只读 35 个：

| 领域 | 工具 |
| --- | --- |
| 账号 | `get_login_status`、`get_current_user` |
| 视频 | `parse_url`、`parse_video`、`get_related_videos`、`get_ai_summary` |
| 搜索/发现 | `search_videos`、`get_hot_search`、`get_search_suggest`、`get_ranking`、`get_region_videos`、`get_pgc_rank`、`get_recommend`、`get_weekly_series`、`get_weekly_detail`、`get_precious_list` |
| 内容 | `get_video_comments`、`get_comment_replies`、`get_video_subtitles`、`get_subtitle_content`、`get_video_danmaku`、`get_article_content` |
| UP/合集 | `get_upper_info`、`get_upper_videos`、`get_upper_collections`、`get_collection_videos`、`get_followings`、`get_followers` |
| 个人列表 | `get_watch_history`、`get_favorite_folders`、`get_favorite_videos`、`get_watch_later`、`get_bangumi_follow`、`get_subscribed_collections`、`get_dynamic_feed` |

工具参数和 JSON Schema 以 `weiyu-mcp tools` 输出为准。

## systemd 部署

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin weiyu
sudo install -d -o weiyu -g weiyu -m 0700 /etc/weiyu
sudo install -o weiyu -g weiyu -m 0600 mcp-token /etc/weiyu/mcp-token
sudo install -o weiyu -g weiyu -m 0600 /dev/null /etc/weiyu/bilibili.cookie
sudo install -m 0644 deploy/weiyu-mcp.service /etc/systemd/system/weiyu-mcp.service
sudo install -o weiyu -g weiyu -m 0600 deploy/weiyu.env.example /etc/weiyu/weiyu.env
# 编辑 /etc/weiyu/weiyu.env，填写对外 Host 和 Cookie 路径
sudo systemctl daemon-reload
sudo systemctl enable --now weiyu-mcp
sudo journalctl -u weiyu-mcp -f
```

Caddy 反向代理示例见 `deploy/Caddyfile.example`。服务只需监听 `127.0.0.1:8787`，由 Caddy/Nginx 终止 TLS。

## Docker / Compose

```bash
cd deploy
mkdir -m 0700 secrets
openssl rand -hex 32 > secrets/mcp-token
touch secrets/bilibili.cookie
chmod 0644 secrets/mcp-token secrets/bilibili.cookie
cp .env.example .env   # 修改 WEIYU_PUBLIC_HOST / WEIYU_ALLOW_WRITE
docker compose up -d --build
docker compose logs -f
```

服务绑定宿主机 `127.0.0.1:8787`，容器使用 distroless + nonroot + 只读根文件系统。Cookie 留空即匿名运行。

构建注意事项：

- 运行镜像默认是 `gcr.io/distroless/cc-debian12:nonroot`。无法直连 GCR 时，用 `RUNTIME_IMAGE` 指定镜像加速器地址，例如 `gcr.m.daocloud.io/distroless/cc-debian12:nonroot`。
- distroless 不含 CA 证书，Dockerfile 会从 builder 复制 `/etc/ssl/certs`；缺少时 B 站 HTTPS 请求会失败。
- 宿主关闭了 Docker iptables（`daemon.json` 里 `"iptables": false`）时，自定义 bridge 的 DNS/NAT 不可用；compose 已设置 `network_mode: bridge`。
- 没有安装 buildx 的宿主可用经典 builder：`DOCKER_BUILDKIT=0 docker build ...`。首次在 4 核 ARM 上编译约 30–40 分钟；小内存机器保持 `CARGO_BUILD_JOBS=2`，内存充足可调到 4。
- 实际部署记录见 [docs/deployment-gm800.md](docs/deployment-gm800.md)。

## 配置项

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `WEIYU_COOKIE_FILE` | 无 | 单行 Cookie 文件；为空表示匿名 |
| `WEIYU_ALLOW_WRITE` | `false` | 是否注册互动写入工具 |
| `WEIYU_MCP_TOKEN` / `WEIYU_MCP_TOKEN_FILE` | 无 | HTTP Bearer 令牌，至少 32 字符 |
| `WEIYU_LISTEN` | `127.0.0.1:8787` | HTTP 监听地址 |
| `WEIYU_ALLOWED_HOSTS` | 本机 | 逗号分隔的允许 Host |
| `WEIYU_ALLOWED_ORIGINS` | 空 | 逗号分隔的允许 Origin |
| `WEIYU_RATE_LIMIT` | `2` | 每秒工具查询数，0 表示不限 |
| `WEIYU_MAX_CONCURRENCY` | `4` | 最大并发 |
| `WEIYU_TIMEOUT_SECS` | `45` | 单次查询超时 |

## 开发与测试

```bash
cargo fmt -p weiyu-mcp --check
cargo clippy -p weiyu-mcp --all-targets --no-deps -- -D warnings
cargo test --workspace --no-default-features
```

离线测试覆盖：工具目录与写入门控、参数校验、stdio/HTTP MCP 握手、Bearer 鉴权、Host/Origin、请求大小、错误码。真实账号权限、Cookie 有效性和服务器出口 IP 是否触发 B 站验证，需要在目标服务器上用实际账号验证。

跨架构构建：

```bash
rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
pip install cargo-zigbuild==0.24.0 ziglang==0.14.1
cargo zigbuild --locked --release -p weiyu-mcp --target aarch64-unknown-linux-musl
cargo zigbuild --locked --release -p weiyu-mcp --target x86_64-unknown-linux-musl
bash scripts/package.sh aarch64-unknown-linux-musl
bash scripts/package.sh x86_64-unknown-linux-musl
```

## 同步上游 core

```bash
scripts/sync-core.sh /path/to/bilbili_copy
git diff -- crates/weiyu-core UPSTREAM.md
```

同步后跑一遍 `cargo test --workspace --no-default-features`，确认 B 站接口调整没有破坏工具层。

## 与桌面版 MCP 的区别

| 能力 | 桌面版 `Weiyu.exe --mcp` | 本服务 |
| --- | --- | --- |
| 查询 | 有 | 有（35 个只读） |
| 点赞/投币/收藏/稍后再看 | 有 | 可选（默认关闭） |
| 下载、批量下载、订阅追更 | 有 | 无 |
| 本地数据库、播放进度、设置 | 有 | 无 |
| 登录扫码、验证码 | 有 | 无（Cookie 由管理员提供） |
| 远程 HTTP MCP | 无 | 有 |
| 部署环境 | Windows 桌面 | Linux systemd / Docker |
