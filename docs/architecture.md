# 架构说明

## 目标

把 B 站 MCP 能力从 Windows 桌面应用中拆出来，作为一个可独立构建、部署在 Linux 服务器上的 AI 查询服务。

非目标：

- 不做下载、批量下载、订阅追更、播放器、本地数据库。
- 登录扫码只在管理台完成，不作为 MCP 工具暴露；验证码交互仍不做。
- 不把 B 站 Cookie 暴露给 AI 工具参数或返回值。

## 仓库边界

```
weiyu-mcp/
├─ crates/weiyu-core/    # 从主仓库复制的 B 站 API 实现
├─ crates/weiyu-mcp/     # 服务器运行时：配置、工具、传输、鉴权
├─ deploy/               # Docker / Compose / systemd / Caddy
├─ scripts/              # 打包、smoke、同步 core
└─ .github/workflows/    # CI / Release
```

`weiyu-core` 是复制而不是 git/path 依赖，目的是让服务器工程可以独立构建、独立发版、独立修改。代价是需要同步：

```bash
scripts/sync-core.sh /path/to/bilbili_copy
```

同步后必须跑 `cargo test --workspace`。如果服务器需要 core 里没有的能力，先在主仓库实现，再同步，避免只在新仓库改导致桌面端与服务器端行为分叉。

## 运行时分层

```
AI 客户端 / 浏览器
  │ stdio MCP / Streamable HTTP + Bearer / /admin
  ▼
weiyu-mcp
  ├─ main.rs         CLI 参数、子命令、启动校验
  ├─ auth.rs         MCP/admin Bearer 校验
  ├─ mcp.rs          rmcp ServerHandler，工具列举与调用
  ├─ http.rs         axum 路由、Host/Origin、优雅退出
  ├─ admin.rs        /admin 静态页 + 管理 API（扫码登录、设置）
  ├─ settings.rs     settings.json 读写、运行设置
  ├─ cookie_store.rs 数据目录 Cookie / 只读回退
  ├─ query.rs        工具注册表、参数校验、限速/并发/超时
  └─ input.rs        链接校验、字幕 URL 白名单
        │
        ▼
weiyu-core
  wbi 签名 / 风控 / video / search / comment / subtitle /
  danmaku / article / favorite / history / interaction ...
```

三种入口（JSON CLI、stdio MCP、HTTP MCP）共用 `query::tools()` 和 `QueryService::call()`，不维护第二套工具定义。管理台使用同一份 `QueryService`，扫码登录和设置修改会热更新内存状态。

## 工具注册与写入门控

- 只读工具：39 个，始终注册（含消息通知 `get_at_feed`/`get_reply_feed`/`get_like_feed`/`get_message_unread`）。
- 互动写入工具：5 个，只有运行设置 `allow_write=true` 时才加入目录。
- 评论工具：1 个（`post_comment`，支持回复和 @ 用户），由独立设置 `allow_comment=true` 控制，
  默认关闭；即使开启互动写入也不会自动暴露评论。
- 运行设置来自 `$WEIYU_DATA_DIR/settings.json`；管理台修改后即时生效（并发数需重启）。
- 未开启时，写入工具不会出现在 `tools/list`，直接调用返回 `UNKNOWN_TOOL`。
- 开启后仍要求 Cookie 含 `bili_jct`；缺少 CSRF 字段返回 `AUTH_REQUIRED`。
- 写入工具都通过 `weiyu-core` 的 `interaction_post()`，统一处理 Cookie、csrf、Origin 和风控响应。

这样设计的原因是：MCP 客户端通常会把 `tools/list` 的全部工具交给模型。如果写入工具只是“调用时拒绝”，模型仍会看到并尝试调用；直接从目录中移除，才是真正的默认关闭。

## 安全模型

| 面 | 措施 |
| --- | --- |
| MCP 访问 | 至少 32 字符 Bearer，常量时间比较 |
| 管理台 | 首次访问设置密码（默认 `admin`，Argon2 哈希 + 12 小时会话）；可选的 admin API Bearer 供脚本使用 |
| Host | 默认只允许本机；对外必须显式 `--allowed-host` |
| Origin | 默认拒绝所有带 Origin 的浏览器请求 |
| 请求体 | MCP 1 MiB；管理 API 64 KiB |
| 查询并发 | 默认 4，满时返回 `BUSY` |
| 查询超时 | 默认 45 秒 |
| 结果大小 | 2 MiB 上限 |
| 限速 | 默认每秒 2 次工具查询 |
| Cookie | 只从数据目录或管理员文件读取，不通过工具参数传入，不写日志 |
| 数据目录 | 容器以 nonroot 运行，`/data` 是唯一可写挂载；systemd 用 `ReadWritePaths` |
| URL | 链接和字幕 URL 做域名/协议/端口白名单，防 SSRF |
| 写入 | 默认不注册；需在管理台开启且 Cookie 含 `bili_jct` |
| 评论 | 独立开关 `allow_comment`，默认关闭；公开可见并会通知被回复/@的用户 |

## B 站风控

服务器出口 IP 比家庭宽带更容易触发验证。服务不会尝试绕过验证码；触发时返回可读错误，由管理员更新 Cookie 或在桌面应用完成验证后重试。专栏/opus 正文已改为走 `x/article/view` 与 `web-dynamic/v1/opus/detail` JSON 接口，避开网页验证码，页面解析只作兜底。建议：

- 使用专用账号和可快速更新的 Cookie。
- 保持较低 `WEIYU_RATE_LIMIT`。
- 必要时为服务器配置固定的出口代理，避免 IP 频繁变化。

## 部署形态

- **静态二进制**：`cargo zigbuild` 产出 `aarch64-unknown-linux-musl` / `x86_64-unknown-linux-musl`。
- **systemd**：只监听 `127.0.0.1:8787`，由 Caddy/Nginx 终止 TLS。
- **Docker**：distroless + nonroot + `read_only` + `cap_drop: ALL`；Cookie/令牌通过 secrets 挂载，`/data` 是可写卷。
- **CI 镜像**：GitHub Actions 用预编译静态二进制构建多架构镜像并推送到 GHCR，服务器只需 `docker pull`，不在本机编译。

## 管理台与扫码登录

- `GET /admin` 返回嵌入二进制的单页管理界面；首次访问进入设置页，默认密码 `admin`，密码哈希保存在 `$WEIYU_DATA_DIR/admin.json`。
- 登录成功后服务端签发 12 小时会话 token；UI 只在 sessionStorage 保存会话，不保存明文密码。
- `WEIYU_ADMIN_TOKEN` / `--admin-token-file` 是可选的 API Bearer，供脚本直接调用管理 API；管理台密码登录不依赖它。
- 扫码登录：`POST /admin/api/login/qrcode` 调 B 站生成二维码并返回 SVG；`GET /admin/api/login/poll?key=...` 轮询状态。
- 登录成功后把 Cookie 写入 `$WEIYU_DATA_DIR/bilibili.cookie`（原子写、0600），同时替换 `QueryService` 内存凭据，MCP 下一次调用立即使用新 Cookie。
- 退出登录写入空 Cookie 文件作为标记，避免只读回退 Cookie 在重启后重新生效。
- 设置保存到 `settings.json`；限速、超时、互动写入、评论写入热更新，并发数变更提示重启。

## 当前未实现（后续可加）

1. **响应裁剪层**：现在部分工具返回完整结构体，token 消耗偏大；后续可按工具做字段精简和长文本截断。
2. **多令牌/配额**：现在 MCP 和 admin 各一个静态令牌；多 AI 客户端场景可加命名令牌和每令牌限速。
3. **指标**：`/healthz` 已提供基础信息，尚未暴露 Prometheus 指标。
4. **OAuth**：远程 MCP 目前只支持静态 Bearer；需要动态授权时再评估 OAuth 2.1。
5. **admin API 令牌轮换**：管理密码可以在管理台修改；可选的 admin API Bearer 修改后需要重启。
