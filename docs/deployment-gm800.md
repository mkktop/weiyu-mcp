# GM800 部署记录

日期：2026-11-18

## 主机

- 地址：`192.168.10.97`
- 系统：Armbian 26.11 / Ubuntu 24.04，aarch64
- 资源：4 核，3.8 GiB 内存，1.9 GiB swap，238 GiB 磁盘
- Docker：29.1.3，Compose 2.40.3
- 限制：无法直连 Docker Hub / GCR / GitHub；crates.io 和 B 站可访问；IPv6 不通，IPv4 正常

## 目录

| 路径 | 用途 |
| --- | --- |
| `/opt/weiyu-mcp/src` | 源码与 compose 文件 |
| `/opt/weiyu-mcp/src/deploy/.env` | Compose 变量（含镜像 tag、对外 Host） |
| `/opt/weiyu-mcp/secrets/mcp-token` | HTTP MCP Bearer 令牌 |
| `/opt/weiyu-mcp/secrets/bilibili.cookie` | B 站 Cookie，当前为空（匿名） |
| `/opt/weiyu-mcp/src/deploy/secrets` | 指向 `/opt/weiyu-mcp/secrets` 的符号链接 |

`/opt/weiyu-mcp` 权限 0700，`secrets` 目录 0700，secret 文件 0644（容器内 nonroot 用户需要读取）。

## 构建

镜像 tag：`weiyu-mcp:0.1.1`。

首次构建使用经典 builder（宿主未安装 buildx，且 `docker compose build` 使用的 Bake 失败）：

```bash
cd /opt/weiyu-mcp/src/deploy
DOCKER_BUILDKIT=0 docker build \
  -f Dockerfile -t weiyu-mcp:0.1.0 \
  --build-arg RUNTIME_IMAGE=gcr.m.daocloud.io/distroless/cc-debian12:nonroot \
  --build-arg CARGO_BUILD_JOBS=2 \
  --build-arg CARGO_PROFILE_RELEASE_LTO=false \
  --build-arg CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 ..
```

首次编译约 35 分钟。`rust:1-bookworm` 和运行时镜像需要提前通过 Docker 镜像加速器拉取：

```bash
docker pull rust:1-bookworm
docker pull gcr.m.daocloud.io/distroless/cc-debian12:nonroot
```

distroless 运行镜像不含 CA 证书，直接运行会无法完成 B 站 HTTPS 握手。当前 `deploy/Dockerfile` 已从 builder 复制 `/etc/ssl/certs`；本次服务器上的 `0.1.1` 是通过追加 CA 层生成的：

```dockerfile
FROM weiyu-mcp:0.1.0
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
```

## 运行

```bash
cd /opt/weiyu-mcp/src/deploy
docker compose up -d --no-build
docker compose ps
docker compose logs --tail=50
```

- 容器：`weiyu-mcp-weiyu-mcp-1`
- 镜像：`weiyu-mcp:0.1.1`
- 端口：`127.0.0.1:8787 -> 8787`
- 用户：`nonroot:nonroot`
- 根文件系统只读，`cap_drop: ALL`，`no-new-privileges`
- 重启策略：`unless-stopped`

宿主 `daemon.json` 配置了 `"iptables": false`，自定义 bridge 的 DNS/NAT 不可用（`wget: bad address`）。因此 compose 使用：

```yaml
network_mode: bridge
```

默认 bridge 可以正常访问 B 站，端口映射仍然有效。

## 验证结果

```text
health {'service': 'weiyu-mcp', 'status': 'ok', 'version': '0.1.0'}
tools 35 write_tools_present False
parse_url aid=170001
get_hot_search 返回 3 条
get_ranking 返回 100 条
search_videos 无 Cookie -> AUTH_REQUIRED
```

## 更新 B 站 Cookie

```bash
sudo nano /opt/weiyu-mcp/secrets/bilibili.cookie
# 粘贴单行 Cookie 请求头，例如 SESSDATA=...; bili_jct=...; buvid3=...; DedeUserID=...
cd /opt/weiyu-mcp/src/deploy
docker compose restart
```

重启后 `get_login_status` 的 `configured` 应为 true；`get_current_user` 才会联网验证。

## 访问方式

服务只监听 GM800 的 `127.0.0.1:8787`。从本机访问建议用 SSH 隧道：

```bash
ssh -L 8787:127.0.0.1:8787 root@192.168.10.97
```

客户端配置：

```json
{
  "mcpServers": {
    "weiyu": {
      "type": "http",
      "url": "http://127.0.0.1:8787/mcp",
      "headers": { "Authorization": "Bearer <mcp-token 文件中的值>" }
    }
  }
}
```

令牌读取：

```bash
cat /opt/weiyu-mcp/secrets/mcp-token
```

如需 LAN 直接访问，把 compose 的端口改为 `0.0.0.0:8787:8787`，并确保令牌保密、防火墙只放行可信网段；更推荐在前面加 Caddy/Nginx 终止 TLS。
