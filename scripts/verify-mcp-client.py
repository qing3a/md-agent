#!/usr/bin/env python3
"""MCP 客户端实机验证：md-agent 同时连接 mock_mcp 与真实 headhunter-erp MCP 壳。
覆盖：servers 列表 / tools 注册表动态合并 / 远程工具执行（只读端点，不污染 ERP 数据）/ 停用移除。
用法: python scripts/verify-mcp-client.py
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

PORT = 18759
BASE = f"http://127.0.0.1:{PORT}"
BIN = os.environ.get("MD_AGENT_BIN", str(pathlib.Path(__file__).resolve().parent.parent / "target" / "debug" / "md-agent.exe"))
ERPS = r"C:\Users\Administrator\Desktop\headhunter-erp\bff\mcp-server.js"
NODE = shutil.which("node") or r"C:\Program Files\nodejs\node.exe"
FAILS = []


def ok(name, cond, extra=""):
    print(("  PASS " if cond else "  FAIL ") + name + (f" — {extra}" if extra else ""))
    if not cond:
        FAILS.append(name)


def api(path, method="GET", body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="md-agent-mcp-"))
    kb = tmp / "kb"
    kb.mkdir()
    cfg = tmp / "config.json"
    scripts = pathlib.Path(__file__).resolve().parent
    mock_mcp = str(scripts / "mock_mcp.py")
    cfg.write_text(json.dumps({
        "kb_root": str(kb),
        "llm": {"endpoint": "", "model": "", "api_key": ""},
        "heartbeat": {"enabled": False, "interval_secs": 5},
        "mcp_servers": [
            {"id": "mock", "name": "Mock MCP", "transport": "stdio",
             "command": sys.executable, "args": [mock_mcp], "enabled": True},
            {"id": "erp", "name": "猎头 ERP", "transport": "stdio",
             "command": NODE, "args": [ERPS], "enabled": True},
        ],
    }), encoding="utf-8")

    proc = subprocess.Popen([BIN, "--no-tray"],
                            env={**os.environ, "MD_AGENT_CONFIG": str(cfg), "MD_AGENT_PORT": str(PORT)},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(30):
            try:
                urllib.request.urlopen(BASE + "/api/health", timeout=2)
                break
            except Exception:
                time.sleep(0.5)
        else:
            print("FATAL: 服务未启动")
            return 1

        print("== 双服务连接 ==")
        r = api("/api/mcp/servers")
        svcs = {s["id"]: s for s in r.get("servers", [])}
        ok("servers 列表含 mock+erp", "mock" in svcs and "erp" in svcs, str(list(svcs.keys())))
        # 懒启动设计：首次列表不触发连接，状态在 tools 合并触发后再断言

        print("== tools 注册表动态合并 ==")
        r = api("/api/tools")
        names = [t["name"] for t in r]
        ok("注册表含 mcp__mock.echo", "mcp__mock.echo" in names)
        ok("注册表含 mcp__erp.candidates_list", "mcp__erp.candidates_list" in names)
        ok("注册表含 mcp__erp.dashboard_stats", "mcp__erp.dashboard_stats" in names)
        ok("注册表含 mcp__erp.ai_match_candidate", "mcp__erp.ai_match_candidate" in names)

        # tools 合并已触发懒启动 → 连接状态此时应就绪
        r = api("/api/mcp/servers")
        svcs = {s["id"]: s for s in r.get("servers", [])}
        ok("mock 已连接", svcs["mock"].get("connected") is True, str(svcs["mock"]))
        ok("erp 已连接", svcs["erp"].get("connected") is True, str(svcs["erp"]))
        ok("erp 工具数=13", svcs["erp"].get("tools") == 13, str(svcs["erp"].get("tools")))

        print("== 远程工具执行 ==")
        r = api("/api/mcp/call", "POST", {"name": "mcp__mock.add", "args": {"a": 40, "b": 2}})
        ok("mock.add = 42", r.get("ok") is True and r.get("result") == "42", str(r)[:100])
        r = api("/api/mcp/call", "POST", {"name": "mcp__erp.dashboard_stats", "args": {}})
        ok("erp.dashboard_stats 真实数据", r.get("ok") is True and "candidates" in r.get("result", ""), str(r)[:120])
        r = api("/api/mcp/call", "POST", {"name": "mcp__erp.candidates_list", "args": {"limit": 3}})
        ok("erp.candidates_list 返回候选", r.get("ok") is True and "candidates" in r.get("result", ""), str(r)[:120])
        r = api("/api/mcp/call", "POST", {"name": "mcp__erp.ai_match_candidate", "args": {"candidate_id": 1, "limit": 3}})
        ok("erp.ai_match_candidate 返回匹配", r.get("ok") is True and "matches" in r.get("result", ""), str(r)[:120])

        print("== 测试连接端点 ==")
        r = api("/api/mcp/servers/erp/test", "POST")
        ok("erp test 返回 13 工具", r.get("ok") is True and len(r.get("tools", [])) == 13)

        print("== 停用/删除 ==")
        api("/api/mcp/servers/erp", "PATCH", {"enabled": False})
        r = api("/api/tools")
        ok("停用后 mcp__erp 移除", not any(t["name"].startswith("mcp__erp") for t in r))
        r = api("/api/mcp/servers")
        s = next((x for x in r.get("servers", []) if x["id"] == "erp"), None)
        ok("停用状态生效", s is not None and s["enabled"] is False)
        r = api("/api/mcp/servers/mock", "DELETE")
        ok("删除 mock", r.get("ok") is True)
    finally:
        proc.terminate()

    print()
    if FAILS:
        print(f"结果: {len(FAILS)} 项失败 — {FAILS}")
        return 1
    print("结果: MCP 客户端实机验证全部通过 ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
