# GM800 部署记录

日期：2026-11-18

## 主机

- 地址：`192.168.10.97`
- 系统：Armbian 26.11 / Ubuntu 24.04，aarch64
- 资源：4 核，3.8 GiB 内存，1.9 GiB swap，238 GiB 磁盘
- Docker：29.1.3，Compose 2.40.3
- 限制：无法直连 Docker Hub / GCR / GitHub；GHCR、crates.io、B 站可访问；IPv6 不通，IPv4 正常

## 当前状态

- 镜像：`ghcr.io/mkktop/weiyu-mcp:0.2.1`（公开，可匿名拉取）
- 容器：`weiyu-mcp-weiyu-mcp-1`
- 端口：`127.0.0.1:8787`
- 管理台：`/admin`，扫码登录已可用
- 版本：`0.2.1`
- 重启策略：`unless-stopped`
- 运行用户：`nonroot:nonroot`，只读根文件系统，`cap_drop: ALL`

## 目录

| 路径 | 用途 |
| --- | --- |
| `/opt/weiyu-mcp/src` | 源码与 compose 文件 |
| `/opt/weiyu-mcp/src/deploy/.env` | Compose 变量（`WEIYU_IMAGE`、对外 Host） |
| `/opt/weiyu-mcp/src/deploy/secrets` | 指向 `/opt/weiyu-mcp/secrets` 的符号链接 |
| `/opt/weiyu-mcp/src/deploy/data` | 指向 `/opt/weiyu-mcp/data` 的符号链接 |
| `/opt/weiyu-mcp/data` | 可写数据目录：`settings.json`、扫码登录后的 `bilibili.cookie` |
| `/opt/weiyu-mcp/secrets/mcp-token` | MCP Bearer 令牌 |
| `/opt/weiyu-mcp/secrets/admin-token` | 管理台 Bearer 令牌（与 MCP 分离） |
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
- Release：<https://github.com/mkktop/weiyu-mcp/releases/tag/v0.2.1>

`v0.2.0` 的镜像二进制缺少可执行位（artifact 上传丢失 mode），运行时报
`/usr/local/bin/weiyu-mcp: permission denied`。`v0.2.1` 在 `Dockerfile.ci` 中使用
`COPY --chmod=0755` 修复。

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
docker pull ghcr.io/mkktop/weiyu-mcp:0.2.1
cd /opt/weiyu-mcp/src/deploy
sed -i 's#^WEIYU_IMAGE=.*#WEIYU_IMAGE=ghcr.io/mkktop/weiyu-mcp:0.2.1#' .env
docker compose up -d --no-build
docker compose logs --tail=50
```

## 运行配置

`deploy/.env` 关键项：

```dotenv
COMPOSE_PROJECT_NAME=weiyu-mcp
WEIYU_IMAGE=ghcr.io/mkktop/weiyu-mcp:0.2.1
WEIYU_PUBLIC_HOST=192.168.10.97,localhost,127.0.0.1
WEIYU_RATE_LIMIT=2
WEIYU_MAX_CONCURRENCY=4
WEIYU_TIMEOUT_SECS=45
WEIYU_ALLOW_WRITE=false
```

compose 额外设置：

- `WEIYU_DATA_DIR=/data`
- `WEIYU_ADMIN_TOKEN_FILE=/run/secrets/admin_token`
- `WEIYU_COOKIE_FILE=/run/secrets/bilibili_cookie`（只读回退）
- `network_mode: bridge`：宿主 `daemon.json` 配置了 `"iptables": false`，自定义 bridge 的 DNS/NAT 不可用。

## 验证结果

```text
health {'service': 'weiyu-mcp', 'status': 'ok', 'version': '0.2.1'}
/admin 200，包含管理台和扫码界面
/admin/api/status 无令牌 -> 401；MCP 令牌 -> 401；admin 令牌 -> 200
status: 35 只读 / 40 含写入，credential_configured=false，persistence=true
/mcp tools/list -> 35
/admin/api/login/qrcode -> 200（qrcode_key 32 位，SVG）
/admin/api/login/poll -> pending
```

## 访问管理台

服务只监听 GM800 的 `127.0.0.1:8787`。从 Windows 建 SSH 隧道：

```bash
ssh -L 8787:127.0.0.1:8787 root@192.168.10.97
```

浏览器打开 <http://127.0.0.1:8787/admin>。管理员令牌：

```bash
ssh root@192.168.10.97 'cat /opt/weiyu-mcp/secrets/admin-token'
```

进入后：

1. 点击“生成登录二维码”，用哔哩哔哩 App 扫码。
2. 扫码成功后 Cookie 写入 `/opt/weiyu-mcp/data/bilibili.cookie`，内存凭据立即生效。
3. 点“验证登录态”确认账号。
4. 在“运行设置”里调整限速/超时/写入开关；并发数修改后需重启容器。

MCP 客户端仍使用 `mcp-token`，连接 `http://127.0.0.1:8787/mcp`。

## 手动更新 Cookie

如果不用扫码，也可以直接写数据目录 Cookie（会覆盖扫码结果）：

```bash
sudo nano /opt/weiyu-mcp/data/bilibili.cookie
# 粘贴单行 Cookie 请求头
cd /opt/weiyu-mcp/src/deploy && docker compose restart
```

数据目录 Cookie 优先于 `/opt/weiyu-mcp/secrets/bilibili.cookie` 回退文件。
