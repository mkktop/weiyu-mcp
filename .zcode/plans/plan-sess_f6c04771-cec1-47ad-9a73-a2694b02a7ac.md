# 抖音 MCP 服务器（weiyu-douyin）实施计划

## Context

现有 `weiyu-mcp` 是 B 站单平台 Rust MCP 服务，写入工具默认不注册。用户要在 `D:\work\weiyu-douyin` 新建一个平级项目，个人使用、低并发，需要评论/点赞/收藏/关注，并要求 **Rust + 运行时纯 HTTP**（不依赖 Playwright），登录通过 **WebUI 扫码**。

调研结论：运行期写请求可以纯 HTTP 实现，但需要 `a_bogus` + TicketGuard v2 + CSRF，评论还需要 `x-tt-session-dtrait`；扫码登录可能触发 2046 短信二次验证，dtrait 内层材料可能需要一次性从浏览器导出。计划按此设计，并在第一阶段用 live probe 做 go/no-go 决策门。

## 交付物

- 新仓库 `D:\work\weiyu-douyin`，单 crate `weiyu-douyin`，MIT；不修改现有 `weiyu-mcp`。
- CLI：`tools`、`call`、`mcp`、`serve`、`login`、`logout`、`probe`。
- 10 个只读工具 + 3 个互动写工具 + 1 个评论写工具，写工具默认不注册。
- 内嵌 WebUI：扫码登录、短信 2046、Cookie 粘贴、浏览器凭据包导入、运行设置。
- README / AGENTS.md / NOTICE / 测试夹具。

## 工具清单

| 类型 | 工具 | 说明 |
|---|---|---|
| 只读 | `douyin_get_login_status` | 是否已配置、write_ready、comment_ready |
| 只读 | `douyin_get_current_user` | 联网校验登录态 |
| 只读 | `douyin_parse_video` | 链接/aweme_id → 视频详情 |
| 只读 | `douyin_search_videos` | 关键词搜索（标注实验性） |
| 只读 | `douyin_get_video_comments` | 评论列表，cursor 分页 |
| 只读 | `douyin_get_comment_replies` | 楼中楼回复 |
| 只读 | `douyin_get_user_profile` | 用户资料 |
| 只读 | `douyin_get_user_videos` | 用户作品，max_cursor 分页 |
| 只读 | `douyin_get_hot_search` | 热榜 |
| 只读 | `douyin_get_own_favorites` | 自己的收藏列表 |
| 互动写（allow_write） | `douyin_like_video` | like=true/false |
| 互动写（allow_write） | `douyin_favorite_video` | favorite=true/false |
| 互动写（allow_write） | `douyin_follow_user` | follow=true/false，需 sec_user_id |
| 评论写（allow_comment） | `douyin_post_comment` | 评论/回复，必须 `confirm=true` |

## 项目结构

```
weiyu-douyin/
├─ Cargo.toml
├─ README.md / AGENTS.md / NOTICE
├─ src/
│  ├─ main.rs / lib.rs / query.rs / mcp.rs
│  ├─ settings.rs / credential.rs / cookie_store.rs
│  ├─ client.rs / profile.rs / error.rs / input.rs
│  ├─ sign/{abogus,secsdk,ticket_guard,dtrait,passport_sign,csrf}.rs
│  ├─ api/{video,comment,search,user,interaction,hot,auth}.rs
│  ├─ login.rs / admin.rs / admin_auth.rs / auth.rs / http.rs
│  └─ static/admin.html
├─ tests/{signing,client,fixtures}
└─ scripts/probe.ps1
```

数据目录（`WEIYU_DOUYIN_DATA_DIR` 或 `--data-dir`，默认平台数据目录）：

- `settings.json`：`rate_limit`、`write_rate_limit`、`max_concurrency`、`timeout_secs`、`allow_write`、`allow_comment`。
- `douyin.cookie`：Cookie 请求头，0600。
- `douyin.session.json`：uid/nickname、TicketGuard 材料、dtrait blob、设备 profile、更新时间，0600。
- `admin.json`：Argon2 管理密码。
- 只读回退：`WEIYU_DOUYIN_COOKIE_FILE` / `--cookie-file`。

## 关键实现

### 1. HTTP 客户端与身份

- `reqwest 0.12` + rustls，启用 gzip/brotli/http2；封装 `DouyinClient`，统一构造公共 query、浏览器 headers、Cookie 顺序。
- 公共 query：`device_platform/aid/channel/pc_client_type/version_code/version_name/platform/browser_*/screen_*/engine_*/os_*/cpu_core_num/device_memory/webid/uifid/verifyFp/fp/msToken`。
- 匿名身份 bootstrap：`ttwid` 注册（`ttwid.bytedance.com`）、首页获取 `UIFID`/`msToken`/`webid`，本地生成 `s_v_web_id`/`verifyFp`。
- 固定一套 Chrome UA + sec-ch-ua + 屏幕参数 profile，保证 a_bogus 的 browser_info 与实际请求一致。
- 读请求可重试一次；写请求**绝不自动重试**。
- 风险信号统一映射：HTTP 200 空 body、HTML、`__ac_nonce`/`_$jsvmprt`、`Uifid Not Found`、`X-Vc-Bdturing-Parameters`、`X-Tt-Verify-Passport-Decision`、`status_code` 8/2483/20003；错误里带 `X-Tt-Logid`，不带 Cookie。

### 2. 签名模块

- `abogus.rs`：移植 2026 新变体（`salt=dhzx`、API `pageId=6241`、passport `pageId=7571`、GET options `[0,1,8]`、POST `[0,1,14]`），SM3 + 改造 RC4 + 自定义 base64 + 校验和。时钟和 RNG 可注入，便于 golden 测试。
- `secsdk.rs`：`md5("{uifid}_{ts}_A96D855A08C0A9707F8BEF0D9A527E4E_{query}")`，只对 14 条受保护 GET 路径生效。
- `passport_sign.rs`：登录用 `sign`/`qs`（排序前 10 个 query + body + app_key 的 SHA-256）。
- `ticket_guard.rs`：P-256 密钥对、ECDH + HKDF-SHA256，`ticket/path/timestamp` 的 HMAC 或 ECDSA 签名，生成 `bd-ticket-guard-*` 头。
- `dtrait.rs`：外层 RSA PKCS#1 v1.5 + AES-128-CBC 信封；内层 blob 优先使用浏览器导出值，不在 v1 重建 19 个渲染特征。
- 参考实现只使用 Apache-2.0/MIT 来源（Evil0ctal、rust-srec、synctv），**不复制 GPL/AGPL 代码**；在 NOTICE 中保留归属。

### 3. 登录（WebUI）

- `login` 子命令启动本地管理页（默认 `127.0.0.1:8789`）并打开浏览器；`serve` 同时挂载 `/admin` 和 `/mcp`。
- 主流程：`login.douyin.com/passport/web/get_qrcode/`（带 passport a_bogus + sign/qs）→ WebUI 展示二维码 → 服务端轮询 `check_qrconnect` → 成功跟随 `redirect_url`（≤5 跳）抓取 Set-Cookie → `profile/self` 校验 → 持久化。
- 内部状态机：`waiting_scan / scanned / verification_required / success / expired / failed`；二维码默认 60 秒有效，按 `expire_time` 刷新，最多 5 次。
- 2046：解析 `verify_ways`；有 `mobile_sms_verify` 时 WebUI 发码/输码，验证码按 XOR 5 转 hex；滑块/人脸/密码则提示降级。
- 兜底入口：Cookie 粘贴、DevTools “Copy as cURL” 解析、浏览器凭据包导入（Cookie + TicketGuard + dtrait blob + UA/profile）。
- 管理密码：首次访问设置（Argon2），12 小时内存会话，5 次失败锁定 30 秒；默认只监听本机。

### 4. 写入

- 点赞：`POST /aweme/v1/web/commit/item/digg/`，form `aweme_id/item_type=0/type=1|0`，a_bogus 只签 query。
- 收藏：`POST /aweme/v1/web/aweme/collect/`，form `action=1|0/aweme_id/aweme_type=0`，a_bogus 签 query+form。
- 关注：`POST /aweme/v1/web/commit/follow/user/`，form `user_id/sec_user_id/type=1|0/channel_id=3/from=0/from_pre=0`。
- 评论：`POST /aweme/v1/web/comment/publish`，最小 form 含 `text/text_extra=[]/comment_send_celltime=0/comment_video_celltime=0/one_level_comment_rank=-1/paste_edit_method=non_paste`；回复追加 `reply_id`/`reply_to_reply_id`。
- 写前检查 `write_ready`/`comment_ready`；写后做一次读校验（detail/comments），不自动重试；写操作串行 + 每分钟限速。
- 错误信息明确区分：未登录、未开启开关、材料未就绪、风控验证、业务错误。

### 5. 安全门控

- `allow_write=false`、`allow_comment=false` 默认；写工具不出现在 `tools/list`，而不是运行时拒绝。
- 评论与互动写独立开关；`douyin_post_comment` 额外要求 `confirm=true`。
- Cookie/Authorization/TicketGuard/dtrait/验证码不写日志、不作为工具参数、不返回给模型。
- `serve` 的 `/mcp` 要求 ≥32 字符 Bearer；Host/Origin 校验；管理台复用密码会话。
- README 显著标注：非官方、个人使用、可能违反抖音用户协议、账号可能被限流/封禁，不做批量和高频。

## 实施顺序

| 阶段 | 内容 | 验收 |
|---|---|---|
| 0 | 建仓库、Cargo、CLI、settings、credential、错误模型、MCP 骨架、admin 空页 | `cargo run -- tools` 输出 10 个只读工具 |
| 1 | 身份 bootstrap、a_bogus、secsdk、公共 query/headers、视频详情/评论/用户/热榜、`probe` | live probe 返回真实视频详情；若 403/空包，进入 fallback 分支，不继续堆写入 |
| 2 | WebUI 扫码 + 2046 短信 + Cookie/cURL/凭据包导入 + 设置 | 扫码后 `get_login_status` 显示已登录、write_ready |
| 3 | TicketGuard、CSRF、点赞/收藏/关注 | 低频率真实写入后，读接口确认状态变化 |
| 4 | 评论/回复 + dtrait 信封 + 导出兜底 | 测试账号发一条评论并可读回；失败时返回可操作的导出提示 |
| 5 | 单测/夹具/mock 集成测试、README/AGENTS、release build | `fmt`、`clippy -D warnings`、`test` 全绿 |

**Go/no-go 门**：阶段 1 的 `probe` 必须确认 a_bogus 新变体和 UIFID/msToken 能被接受。若失败，按顺序尝试：旧变体 → 嵌入 QuickJS 执行 bdms → 浏览器凭据包导入；不伪造“已可用”。

## 验证

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- tools
cargo run -- probe --aweme-id <公开视频ID>
cargo run -- login          # 打开 WebUI 扫码
cargo run -- call douyin_parse_video --args '{"aweme_id":"..."}'
```

- 单测：SM3/RC4/自定义 base64 KAT、a_bogus 固定 RNG/时钟 golden、secsdk MD5、TicketGuard 签名/验签、dtrait AES/RSA 往返、passport sign、Cookie/URL/状态码解析。
- 夹具：Apache-2.0 的 a_bogus 浏览器 oracle，只做结构与解码字段比对，不硬编码随机输出。
- mock 集成：本地 axum mock 断言 query/header/body，覆盖读、点赞、收藏、关注、评论。
- 手动写入 smoke：仅在用户确认的测试账号/测试视频上，低频执行，写后读校验；不批量。

## 风险与预案

- **签名漂移**：抖音改 bdms/字段会导致 200 空包；`probe` + `logid` + 版本日志用于快速定位，必要时切旧变体/QuickJS。
- **TLS 指纹**：`reqwest` 的 ClientHello 可能被识别；若 probe 显示异常，HTTP 客户端抽象可切换到 `rquest`/`impit`，不影响上层。
- **2046/滑块**：短信可纯 HTTP；滑块/人脸不破解，直接降级 Cookie/凭据包导入。
- **dtrait**：评论最脆弱；v1 使用导出 blob，不重建渲染特征；失效则重新导出。
- **关注**：纯 HTTP 成功证据弱于点赞/收藏，阶段 3 单独验证；失败时保留工具但返回明确错误。
- **合规/账号**：个人使用、低频率、不发布公开镜像；README 明确风险，不提供批量/循环能力。