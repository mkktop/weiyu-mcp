#!/usr/bin/env python3
"""Run the packaged Linux executable, optionally under qemu-aarch64. No account needed."""
import argparse
import json
import os
import re
import secrets
import selectors
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def read_line(stream, timeout=20):
    with selectors.DefaultSelector() as selector:
        selector.register(stream, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise TimeoutError("Timed out waiting for CLI output")
    value = stream.readline()
    if not value:
        raise RuntimeError("CLI closed its output unexpectedly")
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("--runner")
    parser.add_argument("--live", action="store_true", help="Also query the public Bilibili hot list")
    args = parser.parse_args()
    prefix = ([args.runner] if args.runner else []) + [str(args.binary.resolve())]
    env = os.environ.copy()
    for name in [
        "WEIYU_COOKIE_FILE",
        "WEIYU_MCP_TOKEN_FILE",
        "WEIYU_ADMIN_TOKEN",
        "WEIYU_ADMIN_TOKEN_FILE",
        "WEIYU_DATA_DIR",
        "WEIYU_ALLOWED_HOSTS",
        "WEIYU_ALLOWED_ORIGINS",
        "WEIYU_ALLOW_WRITE",
    ]:
        env.pop(name, None)
    token = secrets.token_hex(32)
    admin_token = secrets.token_hex(32)
    env["WEIYU_MCP_TOKEN"] = token
    env["WEIYU_ADMIN_TOKEN"] = admin_token
    env["WEIYU_RATE_LIMIT"] = "0"
    data_dir = tempfile.TemporaryDirectory(prefix="weiyu-smoke-")
    server_env = env.copy()
    server_env["WEIYU_DATA_DIR"] = data_dir.name

    version = subprocess.check_output(prefix + ["--version"], env=env, timeout=20).decode().strip()
    catalog = json.loads(subprocess.check_output(prefix + ["tools"], env=env, timeout=20))
    assert len(catalog["tools"]) == 39
    assert all(tool["name"] not in ("like_video", "coin_video") for tool in catalog["tools"])
    writable = json.loads(subprocess.check_output(prefix + ["tools", "--allow-write"], env=env, timeout=20))
    assert len(writable["tools"]) == 44
    assert any(tool["name"] == "like_video" for tool in writable["tools"])
    commented = json.loads(subprocess.check_output(prefix + ["tools", "--allow-comment"], env=env, timeout=20))
    assert len(commented["tools"]) == 40
    assert any(tool["name"] == "post_comment" for tool in commented["tools"])

    parsed = json.loads(
        subprocess.check_output(
            prefix + ["call", "parse_url", "--args", '{"url":"BV17x411w7KC"}'],
            env=env,
            timeout=20,
        )
    )
    assert parsed["aid"] == 170001

    def message(identifier, method, params=None):
        value = {"jsonrpc": "2.0", "id": identifier, "method": method}
        if params is not None:
            value["params"] = params
        return value

    init = message(
        1,
        "initialize",
        {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "artifact-smoke", "version": "1"},
        },
    )
    stdio = subprocess.Popen(
        prefix + ["mcp"],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        def stdio_rpc(value):
            stdio.stdin.write(json.dumps(value).encode() + b"\n")
            stdio.stdin.flush()
            return json.loads(read_line(stdio.stdout))

        assert stdio_rpc(init)["result"]["serverInfo"]["name"] == "weiyu-mcp"
        stdio.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        listing = stdio_rpc(message(2, "tools/list"))
        assert len(listing["result"]["tools"]) == 39
        result = stdio_rpc(
            message(3, "tools/call", {"name": "parse_url", "arguments": {"url": "BV17x411w7KC"}})
        )
        assert result["result"]["structuredContent"]["aid"] == 170001
        stdio.stdin.close()
        assert stdio.wait(timeout=10) == 0
        assert stdio.stdout.read() == b""
    finally:
        if stdio.poll() is None:
            stdio.kill()
            stdio.wait()

    server = subprocess.Popen(
        prefix + ["serve", "--listen", "127.0.0.1:0"],
        env=server_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 20
        while True:
            line = read_line(server.stderr).decode()
            match = re.search(r"http://(127\.0\.0\.1:\d+)/mcp", line)
            if match:
                base = "http://" + match.group(1)
                break
            if time.monotonic() > deadline:
                raise TimeoutError("HTTP server did not start")
        # This opener is only used for the loopback server under test.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        health = json.loads(opener.open(base + "/healthz", timeout=10).read())
        assert health["status"] == "ok"
        assert health["service"] == "weiyu-mcp"
        admin_page = opener.open(base + "/admin", timeout=10).read().decode()
        assert "未雨" in admin_page and "扫码" in admin_page
        try:
            opener.open(base + "/admin/api/status", timeout=10)
            raise AssertionError("Unauthenticated admin request was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 401
        state = json.loads(opener.open(base + "/admin/api/auth/state", timeout=10).read())
        assert state["needs_setup"] is True
        setup = json.loads(
            opener.open(
                urllib.request.Request(
                    base + "/admin/api/auth/setup",
                    data=b"{}",
                    headers={"Content-Type": "application/json"},
                    method="POST",
                ),
                timeout=10,
            ).read()
        )
        assert setup["using_default_password"] is True
        session = setup["session"]
        admin_status = json.loads(
            opener.open(
                urllib.request.Request(
                    base + "/admin/api/status",
                    headers={"Authorization": "Bearer " + session},
                ),
                timeout=10,
            ).read()
        )
        assert admin_status["read_tools"] == 39
        assert admin_status["write_tools"] == 45
        assert admin_status["persistence"] is True
        assert admin_status["using_default_password"] is True
        legacy_status = json.loads(
            opener.open(
                urllib.request.Request(
                    base + "/admin/api/status",
                    headers={"Authorization": "Bearer " + admin_token},
                ),
                timeout=10,
            ).read()
        )
        assert legacy_status["read_tools"] == 39
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "MCP-Protocol-Version": "2025-03-26",
        }
        try:
            opener.open(
                urllib.request.Request(base + "/mcp", data=json.dumps(init).encode(), headers=headers),
                timeout=10,
            )
            raise AssertionError("Unauthenticated MCP request was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 401
        headers["Authorization"] = "Bearer " + token

        def http_rpc(value):
            req = urllib.request.Request(base + "/mcp", data=json.dumps(value).encode(), headers=headers)
            return json.loads(opener.open(req, timeout=60).read())

        assert http_rpc(init)["result"]["serverInfo"]["name"] == "weiyu-mcp"
        assert len(http_rpc(message(2, "tools/list"))["result"]["tools"]) == 39
        value = http_rpc(message(3, "tools/call", {"name": "parse_url", "arguments": {"url": "BV17x411w7KC"}}))
        assert value["result"]["structuredContent"]["aid"] == 170001
        value = http_rpc(message(4, "tools/call", {"name": "search_videos", "arguments": {"keyword": "test"}}))
        assert value["result"]["isError"] is True
        assert value["result"]["structuredContent"]["error"]["code"] == "AUTH_REQUIRED"
        if args.live:
            value = http_rpc(message(5, "tools/call", {"name": "get_hot_search", "arguments": {"limit": 3}}))
            assert value["result"]["isError"] is False, value["result"]
            assert len(value["result"]["structuredContent"]["data"]) == 3
        server.terminate()
        assert server.wait(timeout=15) == 0
        print(
            json.dumps(
                {
                    "binary": str(args.binary),
                    "version": version,
                    "runner": args.runner,
                    "tools": 39,
                    "write_tools": 45,
                    "stdio": "passed",
                    "http": "passed",
                    "admin": "passed",
                    "sigterm": "passed",
                    "live_bilibili": args.live,
                }
            )
        )
    finally:
        if server.poll() is None:
            server.kill()
            server.wait()


if __name__ == "__main__":
    main()
