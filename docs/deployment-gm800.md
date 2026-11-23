# GM800 部署记录

日期：2026-11-18

## 主机

- 地址：`192.168.10.97`
- 系统：Armbian 26.11 / Ubuntu 24.04，aarch64
- 资源：4 核，3.8 GiB 内存，1.9 GiB swap，238 GiB 磁盘
- Docker：29.1.3，Compose 2.40.3
- 限制：无法直连 Docker Hub / GCR / GitHub；GHCR、crates.io、B 站可访问；IPv6 不通，IPv4 正常

## 当前状态

- 镜像：`ghcr.io/mkktop/weiyu-mcp:0.4.1`（公开，可匿名拉取）
- 容器：`weiyu-mcp-weiyu-mcp-1`
- 端口：`0.0.0.0:8787`（局域网可访问，`WEIYU_BIND_ADDR=0.0.0.0`）
- 管理台：`/admin`，首次访问设置管理密码（默认 `admin`），扫码登录已可用
- 版本：`0.4.1`
- 重启策略：`unless-stopped`
- 运行用户：`nonroot:nonroot`，只读根文件系统，`cap_drop: ALL`

## 目录

| 路径 | 用途 |
| --- | --- |
| `/opt/weiyu-mcp/src` | 源码与 compose 文件 |
| `/opt/weiyu-mcp/src/deploy/.env` | Compose 变量（`WEIYU_IMAGE`、对外 Host） |
| `/opt/weiyu-mcp/src/deploy/secrets` | 指向 `/opt/weiyu-mcp/secrets` 的符号链接 |
| `/opt/weiyu-mcp/src/deploy/data` | 指向 `/opt/weiyu-mcp/data` 的符号链接 |
| `/opt/weiyu-mcp/data` | 可写数据目录：`settings.json`、`admin.json`、扫码登录后的 `bilibili.cookie` |
| `/opt/weiyu-mcp/secrets/mcp-token` | MCP Bearer 令牌 |
| `/opt/weiyu-mcp/secrets/admin-token` | 可选的 admin API Bearer（脚本用；管理台密码不依赖它） |
| `/opt/weiyu-mcp/secrets/bilibili.cookie` | 只读 Cookie 回退（当前为空） |

权限：

- `/opt/weiyu-mcp` 和 `secrets` 目录 0700。
- `data` 属主 `65532:65532`（distroless nonroot），0700。
- secret 文件 0644，目录 0700 限制访问。

## 发布与构建

镜像由 GitHub Actions 的 `Release` workflow 构建：

- `cargo zigbuild` 产出 `aarch64-unknown-linux-musl` / `x86_64-unknown-linux-musl` 静态二进制。
- `deploy/Dockerfile.ci` 把预编译二进制复制进 `distroless/static-debian12`，不在 Docker 内编译。
- `docker/build-push-action` 推送多架构镜像到 GHCR。
- 仓库：<https://github.com/mkktop/weiyu-mcp>
- Release：<https://github.com/mkktop/weiyu-mcp/releases/tag/v0.4.1>

`v0.2.0` 的镜像二进制缺少可执行位（artifact 上传丢失 mode），运行时报
`/usr/local/bin/weiyu-mcp: permission denied`。`v0.2.2` 在 `Dockerfile.ci` 中使用
`COPY --chmod=0755` 修复。

`v0.2.3` 修复 `get_article_content`：B 站对服务器出口 IP 抓取 `/read`、`/opus` 页面会返回
风控验证码页，页面里没有 `__INITIAL_STATE__`。专栏正文改为优先调用
`x/article/view?id=` 与 `x/polymer/web-dynamic/v1/opus/detail`，页面解析只作兜底。

`v0.2.4` 补齐错误信息：接口和页面同时失败时同时返回两边原因（如接口 `-404`/风控），
不再只显示「页面解析也失败」。

`v0.3.0` 新增 `post_comment`：可发表评论、回复评论和 @ 用户（`at_name_to_mid`）。
由独立设置 `allow_comment` 控制，默认关闭；即使开启互动写入也不会自动开放评论。

`v0.3.1` 处理评论索引延迟：刚发表的一级评论立即回复可能返回「没有该评论」，现在会等待 3 秒
自动重试一次；实测确认 @ 提及会写入 `content.members`，但 @ 自己不会生成提及。

`v0.3.2` 重设计管理台界面：新品牌色与渐变标题、卡片阴影与入场动画、状态值按健康度着色、
互动写入/评论开关改为滑动开关、二维码加渐变边框并带等待动画，提示条 8 秒自动消失。

`v0.4.0` 新增 4 个消息通知只读工具（只读 35 → 39）：`get_at_feed`（@我的）、`get_reply_feed`
（回复我的）、`get_like_feed`（收到的赞）、`get_message_unread`（未读数，可轮询是否被 @/回复）。
对应 B 站 `x/msgfeed/at|reply|like|unread`，只需 SESSDATA，服务器 IP 不触发风控；
游标分页（`last_id`+`last_time`），接口无状态不去重，调用方按通知 id 去重。

`v0.4.1` 修复 at 通知条目时间戳取错字段（此前显示 `time: 0`，现正确取 `at_time`）。

### 初始 0.1.1 本地构建（历史）

首次部署时在 GM800 本地用经典 builder 构建，耗时约 35 分钟：

```bash
cd /opt/weiyu-mcp/src/deploy
DOCKER_BUILDKIT=0 docker build \
  -f Dockerfile -t weiyu-mcp:0.1.0 \
  --build-arg RUNTIME_IMAGE=gcr.m.daocloud.io/distroless/cc-debian12:nonroot \
  --build-arg CARGO_BUILD_JOBS=2 \
  --build-arg CARGO_PROFILE_RELEASE_LTO=false \
  --build-arg CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 ..
```

distroless 不含 CA 证书，第一次运行时手工追加 CA 层生成了 `0.1.1`。现在 CI 镜像已包含 CA 证书和可执行位，不再需要本地构建。

## 更新镜像

```bash
docker pull ghcr.io/mkktop/weiyu-mcp:0.4.1
cd /opt/weiyu-mcp/src/deploy
sed -i 's#^WEIYU_IMAGE=.*#WEIYU_IMAGE=ghcr.io/mkktop/weiyu-mcp:0.4.1#' .env
docker compose up -d --no-build
docker compose logs --tail=50
```

## 运行配置

`deploy/.env` 关键项：

```dotenv
COMPOSE_PROJECT_NAME=weiyu-mcp
WEIYU_IMAGE=ghcr.io/mkktop/weiyu-mcp:0.4.1
WEIYU_PUBLIC_HOST=192.168.10.97,localhost,127.0.0.1
WEIYU_BIND_ADDR=0.0.0.0
WEIYU_RATE_LIMIT=2
WEIYU_MAX_CONCURRENCY=4
WEIYU_TIMEOUT_SECS=45
WEIYU_ALLOW_WRITE=false
WEIYU_ALLOW_COMMENT=false
```

compose 额外设置：

- `WEIYU_DATA_DIR=/data`
- `WEIYU_ADMIN_TOKEN_FILE=/run/secrets/admin_token`
- `WEIYU_COOKIE_FILE=/run/secrets/bilibili_cookie`（只读回退）
- `WEIYU_BIND_ADDR=0.0.0.0`：compose 端口映射绑定所有网卡，局域网可直接访问管理台和 MCP。
- `network_mode: bridge`：宿主 `daemon.json` 配置了 `"iptables": false`，自定义 bridge 的 DNS/NAT 不可用。

## 验证结果

```text
health {'service': 'weiyu-mcp', 'status': 'ok', 'version': '0.4.1'}
/admin 200，管理台密码已设置（needs_setup=false，非默认密码）
/admin/api/status 无令牌 -> 401；MCP 令牌 -> 401；admin API 令牌 -> 200
status: 39 只读 / 45 全开，comment_enabled=true，credential_configured=true（data）
/mcp tools/list -> 45（互动写入 + 评论写入均已开启；默认关闭时 39）
扫码登录后 get_current_user -> 新雨bot（Lv2，mid 3707006871407373）
get_article_content cvid=53698541 -> 约 1.5 万字正文
get_article_content opus_id=1265361021143351318 -> 约 1.5 万字正文
get_article_content cvid=999999999 -> 同时返回接口 -404 与页面解析失败原因
post_comment 一级评论 -> OK，返回 rpid
post_comment parent=主评论 -> OK，root=parent=主评论（索引延迟时 3 秒后自动重试）
@ 用户 -> content.members 包含被@用户；@ 自己不生成提及
测试评论已全部通过 x/v2/reply/del 删除，评论区计数恢复为 0
get_message_unread -> 计数字段可用但不可靠（有 2 条 @ 时仍全 0，判断被@要看 feed 本身）
get_at_feed -> 端到端验证通过：主账号（脉冲疯子）在两个视频评论区 @ 新雨bot，
  通知均到达，含 at_time、评论内容、视频标题和跳转 URI；发出后约 1 分钟内可查到
get_reply_feed / get_like_feed -> 空列表结构正常
楼中楼回复 @ 被回复人本人 -> 只产生「回复我的」通知（条目内带 at_details，@ 被折叠），
  不产生「@我的」；原始 reply API 核实 content.members 均已正确写入，@ 链接渲染正常
get_at_feed 全部条目 root_id=0 -> 「@我的」通知只来自一级评论，楼中楼 @ 被回复人不进此列表
```

## 访问管理台

浏览器直接打开：

**<http://192.168.10.97:8787/admin>**

首次访问会显示“设置管理密码”页面，密码默认 `admin`，可以直接保存或改成自己的密码。密码用 Argon2 哈希后写入
`/opt/weiyu-mcp/data/admin.json`；之后用密码登录，会话有效期 12 小时。建议不要长期使用默认密码。

`/opt/weiyu-mcp/secrets/admin-token` 现在是**可选的 API Bearer**，只供脚本直接调用管理 API；管理台密码登录不依赖它：

```bash
ssh root@192.168.10.97 'cat /opt/weiyu-mcp/secrets/admin-token'
```

如果只想走本机访问，也可以建 SSH 隧道：

```bash
ssh -L 8787:127.0.0.1:8787 root@192.168.10.97
# 然后打开 http://127.0.0.1:8787/admin
```

> 当前是 HTTP 直连，管理密码和 MCP 令牌在局域网内以明文传输。只在可信网络使用；
> 如果不信任局域网，建议改回 `WEIYU_BIND_ADDR=127.0.0.1` 走 SSH 隧道，或在前面加 Caddy/Nginx 终止 TLS。
>
> 首次设置完成之前，能访问 `/admin` 的人都可以先设置密码；部署后请立即完成首次设置。

进入后：

1. 首次进入设置管理密码（默认 `admin`，建议修改）。
2. 点击“生成登录二维码”，用哔哩哔哩 App 扫码。
3. 扫码成功后 Cookie 写入 `/opt/weiyu-mcp/data/bilibili.cookie`，内存凭据立即生效。
4. 点“验证登录态”确认账号。
5. 在“运行设置”里调整限速/超时/写入开关；并发数修改后需重启容器。
6. 在“管理密码”里修改密码；忘记密码时删除 `/opt/weiyu-mcp/data/admin.json` 并重启。

MCP 客户端仍使用 `mcp-token`，连接 `http://192.168.10.97:8787/mcp`（或隧道地址 `http://127.0.0.1:8787/mcp`）。

## 手动更新 Cookie

如果不用扫码，也可以直接写数据目录 Cookie（会覆盖扫码结果）：

```bash
sudo nano /opt/weiyu-mcp/data/bilibili.cookie
# 粘贴单行 Cookie 请求头
cd /opt/weiyu-mcp/src/deploy && docker compose restart
```

数据目录 Cookie 优先于 `/opt/weiyu-mcp/secrets/bilibili.cookie` 回退文件。
