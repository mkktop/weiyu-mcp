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

## 管理台（WebUI）

`serve` 模式在 `/admin` 提供管理台。**首次访问会让设置管理密码**，默认 `admin`，可直接使用或改成自己的密码；密码用 Argon2 哈希后保存到 `$WEIYU_DATA_DIR/admin.json`。

```bash
weiyu-mcp serve \
  --listen 127.0.0.1:8787 \
  --token-file ./mcp-token \
  --data-dir ./data
```

- 浏览器打开 `http://127.0.0.1:8787/admin`，首次进入设置管理密码（默认 `admin`），之后用密码登录。
- 登录会话 12 小时；服务重启后需要重新登录。
- 功能：B 站扫码登录、验证登录态、退出登录、限速/并发/超时/写入开关、修改管理密码。
- 扫码成功后 Cookie 写入 `$WEIYU_DATA_DIR/bilibili.cookie`，并立即热更新内存凭据，不需要重启。
- `$WEIYU_DATA_DIR/settings.json` 保存运行设置；文件存在后以文件为准，CLI/环境变量只作为首次启动的初始值。
- 限速、超时、写入开关即时生效；**并发数修改需要重启容器**（信号量在启动时创建）。
- 忘记管理密码：删除 `$WEIYU_DATA_DIR/admin.json` 后重启，重新进入设置页。
- `WEIYU_ADMIN_TOKEN` / `--admin-token-file` 现在是**可选的 API Bearer**，供脚本/自动化直接调用管理 API；不配置也可以使用管理台密码登录。

> 首次设置密码之前，能访问 `/admin` 的人都可以先设置密码；部署后请尽快完成首次设置，并使用非默认密码。

数据目录建议：

| 场景 | `WEIYU_DATA_DIR` |
| --- | --- |
| systemd | `/var/lib/weiyu`（属主为服务用户） |
| Docker | `/data`（挂载可写卷） |
| 本地试用 | `./data` |

## 互动写入（默认关闭）

写入工具会修改真实 B 站账号。默认不注册，AI 在 `tools/list` 中看不到它们。

开启方式（任选其一）：

- 管理台 → 勾选“启用互动写入工具” → 保存（即时生效）。
- 首次启动时 `weiyu-mcp --allow-write serve ...` 或 `WEIYU_ALLOW_WRITE=true`；如果 `settings.json` 已存在，以文件值为准。

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
sudo install -d -o weiyu -g weiyu -m 0700 /var/lib/weiyu
sudo install -o weiyu -g weiyu -m 0600 mcp-token /etc/weiyu/mcp-token
sudo sh -c 'openssl rand -hex 32 > /etc/weiyu/admin-token'
sudo chown weiyu:weiyu /etc/weiyu/admin-token
sudo chmod 0600 /etc/weiyu/admin-token
sudo install -o weiyu -g weiyu -m 0600 /dev/null /etc/weiyu/bilibili.cookie
sudo install -m 0644 deploy/weiyu-mcp.service /etc/systemd/system/weiyu-mcp.service
sudo install -o weiyu -g weiyu -m 0600 deploy/weiyu.env.example /etc/weiyu/weiyu.env
# 编辑 /etc/weiyu/weiyu.env，填写对外 Host 和 Cookie 路径
sudo systemctl daemon-reload
sudo systemctl enable --now weiyu-mcp
sudo journalctl -u weiyu-mcp -f
```

`/var/lib/weiyu` 保存 `settings.json` 和扫码登录后的 `bilibili.cookie`；service 已通过 `ReadWritePaths` 允许写入该目录。

Caddy 反向代理示例见 `deploy/Caddyfile.example`。服务只需监听 `127.0.0.1:8787`，由 Caddy/Nginx 终止 TLS。

## Docker / Compose

```bash
cd deploy
mkdir -m 0700 secrets data
openssl rand -hex 32 > secrets/mcp-token
openssl rand -hex 32 > secrets/admin-token
touch secrets/bilibili.cookie
chmod 0644 secrets/mcp-token secrets/admin-token secrets/bilibili.cookie
# 容器以 UID 65532 运行，需要可写 data 目录
sudo chown -R 65532:65532 data
cp .env.example .env   # 修改 WEIYU_PUBLIC_HOST / WEIYU_IMAGE / WEIYU_ALLOW_WRITE
docker compose up -d --build
docker compose logs -f
```

服务默认绑定宿主机 `127.0.0.1:8787`；需要局域网直接访问时在 `.env` 设置 `WEIYU_BIND_ADDR=0.0.0.0`。容器使用 distroless + nonroot + 只读根文件系统；`./data` 挂载为唯一可写目录。Cookie 留空即匿名运行，之后可在 `/admin` 扫码登录。

### 使用 GitHub Actions 构建的镜像

仓库的 `Release` workflow 会用 `cargo zigbuild` 产出 arm64/x86_64 静态二进制，并构建多架构镜像推送到 GHCR：

```bash
docker pull ghcr.io/<owner>/weiyu-mcp:latest
# 在 deploy/.env 中设置：
# WEIYU_IMAGE=ghcr.io/<owner>/weiyu-mcp:latest
docker compose up -d --no-build
```

- 公开仓库的 GHCR 包默认可能仍是私有；在 GitHub → Packages → Package settings 中设为 Public，或先 `docker login ghcr.io`（PAT 需要 `read:packages`）。
- 版本 tag 会同时生成 `:x.y.z` 和 `:latest`，分支构建生成 `:edge`。

构建注意事项：

- 本地直接 `docker compose build` 使用 `deploy/Dockerfile`；CI 镜像是 `deploy/Dockerfile.ci`，只复制预编译的静态二进制，不在 Docker 里编译。
- 本地构建镜像默认是 `gcr.io/distroless/cc-debian12:nonroot`。无法直连 GCR 时，用 `RUNTIME_IMAGE` 指定镜像加速器地址，例如 `gcr.m.daocloud.io/distroless/cc-debian12:nonroot`。
- distroless 不含 CA 证书，Dockerfile 会从 builder 复制 `/etc/ssl/certs`；缺少时 B 站 HTTPS 请求会失败。
- 宿主关闭了 Docker iptables（`daemon.json` 里 `"iptables": false`）时，自定义 bridge 的 DNS/NAT 不可用；compose 已设置 `network_mode: bridge`。
- 没有安装 buildx 的宿主可用经典 builder：`DOCKER_BUILDKIT=0 docker build ...`。首次在 4 核 ARM 上编译约 30–40 分钟；小内存机器保持 `CARGO_BUILD_JOBS=2`，内存充足可调到 4。
- 实际部署记录见 [docs/deployment-gm800.md](docs/deployment-gm800.md)。

## 配置项

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `WEIYU_DATA_DIR` | 无 | 可写数据目录；存放 `settings.json`、`admin.json` 和扫码登录后的 `bilibili.cookie` |
| `WEIYU_COOKIE_FILE` | 无 | 只读 Cookie 回退文件；数据目录 Cookie 优先 |
| `WEIYU_ADMIN_TOKEN` / `WEIYU_ADMIN_TOKEN_FILE` | 无 | 可选的 admin API Bearer；管理台密码在首次访问时设置，不依赖此项 |
| `WEIYU_ALLOW_WRITE` | `false` | 首次启动的写入开关；`settings.json` 存在后以文件为准 |
| `WEIYU_MCP_TOKEN` / `WEIYU_MCP_TOKEN_FILE` | 无 | HTTP MCP Bearer 令牌，至少 32 字符 |
| `WEIYU_LISTEN` | `127.0.0.1:8787` | HTTP 监听地址 |
| `WEIYU_ALLOWED_HOSTS` | 本机 | 逗号分隔的允许 Host |
| `WEIYU_ALLOWED_ORIGINS` | 空 | 逗号分隔的允许 Origin |
| `WEIYU_RATE_LIMIT` | `2` | 首次启动的每秒查询数，0 表示不限 |
| `WEIYU_MAX_CONCURRENCY` | `4` | 首次启动的最大并发 |
| `WEIYU_TIMEOUT_SECS` | `45` | 首次启动的单次查询超时 |

Compose 额外变量：`WEIYU_IMAGE`（镜像地址）、`WEIYU_PUBLIC_HOST`（允许的对外 Host）、`WEIYU_BIND_ADDR`（默认 `127.0.0.1`，局域网访问设 `0.0.0.0`）、`RUNTIME_IMAGE` / `CARGO_*`（构建参数）。

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
| 本地数据库、播放进度、设置 | 有 | 无（运行设置存 `settings.json`） |
| 登录扫码、验证码 | 有 | 管理台支持扫码登录；无验证码交互 |
| Web 管理台 | 无 | 有（`/admin`） |
| 远程 HTTP MCP | 无 | 有 |
| 部署环境 | Windows 桌面 | Linux systemd / Docker |
