# 部署指南

systemd 与 Docker 两种部署方式的完整步骤、GHCR 镜像说明和构建疑难排查。
真实服务器的完整实战记录（含验证输出）见 [deployment-gm800.md](deployment-gm800.md)。

## systemd

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
cp .env.example .env   # 修改 WEIYU_PUBLIC_HOST / WEIYU_IMAGE / WEIYU_ALLOW_WRITE / WEIYU_ALLOW_COMMENT
docker compose up -d --build
docker compose logs -f
```

服务默认绑定宿主机 `127.0.0.1:8787`；需要局域网直接访问时在 `.env` 设置 `WEIYU_BIND_ADDR=0.0.0.0`。容器使用 distroless + nonroot + 只读根文件系统；`./data` 挂载为唯一可写目录。Cookie 留空即匿名运行，之后可在 `/admin` 扫码登录。

### 使用 GitHub Actions 构建的镜像

仓库的 `Release` workflow 会用 `cargo zigbuild` 产出 arm64/x86_64 静态二进制，并构建多架构镜像推送到 GHCR：

```bash
docker pull ghcr.io/mkktop/weiyu-mcp:latest
# 在 deploy/.env 中设置：
# WEIYU_IMAGE=ghcr.io/mkktop/weiyu-mcp:latest
docker compose up -d --no-build
```

- 公开仓库的 GHCR 包默认可能仍是私有；在 GitHub → Packages → Package settings 中设为 Public，或先 `docker login ghcr.io`（PAT 需要 `read:packages`）。
- 版本 tag 会同时生成 `:x.y.z` 和 `:latest`，分支构建生成 `:edge`。

### 构建注意事项

- 本地直接 `docker compose build` 使用 `deploy/Dockerfile`；CI 镜像是 `deploy/Dockerfile.ci`，只复制预编译的静态二进制，不在 Docker 里编译。
- 本地构建镜像默认是 `gcr.io/distroless/cc-debian12:nonroot`。无法直连 GCR 时，用 `RUNTIME_IMAGE` 指定镜像加速器地址，例如 `gcr.m.daocloud.io/distroless/cc-debian12:nonroot`。
- distroless 不含 CA 证书，Dockerfile 会从 builder 复制 `/etc/ssl/certs`；缺少时 B 站 HTTPS 请求会失败。
- 宿主关闭了 Docker iptables（`daemon.json` 里 `"iptables": false`）时，自定义 bridge 的 DNS/NAT 不可用；compose 已设置 `network_mode: bridge`。
- 没有安装 buildx 的宿主可用经典 builder：`DOCKER_BUILDKIT=0 docker build ...`。首次在 4 核 ARM 上编译约 30–40 分钟；小内存机器保持 `CARGO_BUILD_JOBS=2`，内存充足可调到 4。

## 运行配置要点

- `WEIYU_DATA_DIR` 指向的可写目录保存 `settings.json`、`admin.json` 和扫码登录后的 `bilibili.cookie`。
- `settings.json` 存在后以文件为准，CLI/环境变量只作为首次启动的初始值。
- 限速、超时、写入开关、评论开关在管理台修改后即时生效；**并发数修改需要重启**（信号量在启动时创建）。
- 忘记管理密码：删除 `$WEIYU_DATA_DIR/admin.json` 后重启，重新进入设置页。

完整环境变量列表见 [README 配置项](../README.md#配置项)。
