#!/usr/bin/env python3
"""Phase 4 bug 修复实机验证（隔离 kb + mock LLM）：
1. /api/agent stream=true SSE：不再 blocking_send panic，事件完整（ToolStart/ToolResult/Answer/__done）
2. /api/agent 非流式：无 system 时注入软工具协议提示词（LLM 可见 system 消息）
3. /api/memory/extract 自动落地：MEMORY.md 不再混入 frontmatter/提示行
4. /api/memory/dream 自动落地：MEMORY.md 不再混入「> 后台巩固」提示行
用法: python scripts/verify-phase4-fixes.py
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.request

PORT = 18757
BASE = f"http://127.0.0.1:{PORT}"
BIN = os.environ.get("MD_AGENT_BIN", str(pathlib.Path(__file__).resolve().parent.parent / "target" / "debug" / "md-agent.exe"))
FAILS = []


def ok(name, cond, extra=""):
    print(("  PASS " if cond else "  FAIL ") + name + (f" — {extra}" if extra else ""))
    if not cond:
        FAILS.append(name)


def api(path, method="GET", body=None, timeout=90):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="md-agent-fix-"))
    kb = tmp / "kb"
    kb.mkdir()
    (kb / "MEMORY.md").write_text("# 记忆\n\n## 2026-08-01\n- 初始记忆\n", encoding="utf-8")
    cfg = tmp / "config.json"
    cfg.write_text(json.dumps({
        "kb_root": str(kb),
        "llm": {"endpoint": "http://127.0.0.1:11434/v1", "model": "mock", "api_key": "",
                "embedding": {"endpoint": "", "model": "", "api_key": ""}},
        "heartbeat": {"enabled": False, "interval_secs": 5},
    }), encoding="utf-8")

    mock = subprocess.Popen([sys.executable, "scripts/mock_llm.py", "11434"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(1.2)
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

            print("== 1/4 /api/agent stream=true（SSE）==")
            req = urllib.request.Request(
                BASE + "/api/agent",
                data=json.dumps({"prompt": "调用工具 查一下 托盘 架构", "stream": True}).encode(),
                headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=90) as r:
                raw = r.read().decode()
            events = [l[6:] for l in raw.splitlines() if l.startswith("data: ")]
            parsed = [json.loads(e) for e in events]
            types = [p["type"] for p in parsed]
            ok("SSE 事件完整（ToolStart/ToolResult/Answer）",
               "ToolStart" in types and "ToolResult" in types and "Answer" in types, str(types))
            ok("SSE 收尾 __done 事件", any(p.get("name") == "__done" for p in parsed),
               str([p for p in parsed if p.get("name") in ("__done", "__error")]))
            ok("SSE 无 __error", not any(p.get("name") == "__error" for p in parsed))

            print("== 2/4 /api/agent 非流式（无 system 注入协议提示词）==")
            r = api("/api/agent", "POST", {"prompt": "调用工具 查一下 记忆 分片"})
            ok("agent 非流式 ok", r.get("ok") is True, str(r)[:200])
            ok("agent 调工具", r.get("tool_calls", 0) >= 1 and "search" in (r.get("tools_used") or []),
               str(r.get("tools_used")))
            ok("agent 回答非空", bool(r.get("answer")))

            print("== 3/4 /api/memory/extract（auto_land 无 frontmatter 污染）==")
            r = api("/api/memory/extract", "POST",
                    {"qa": "Q: 图谱用哪个方案？\nA: 双骨架：目录放射树 + case-rel 定向边\n", "source": "sessions/test.md"})
            ok("extract ok", r.get("ok") is True, str(r)[:150])
            mem = (kb / "MEMORY.md").read_text(encoding="utf-8")
            ok("MEMORY.md 无 frontmatter 污染", "type: memory" not in mem and "target: MEMORY.md" not in mem, mem[-200:])
            ok("MEMORY.md 无提示行", "人工核对" not in mem and "> 会话" not in mem)
            ok("MEMORY.md 有新内容", len(mem) > len("# 记忆\n\n## 2026-08-01\n- 初始记忆\n"))

            print("== 4/4 /api/memory/dream（auto_land 无提示行）==")
            r = api("/api/memory/dream", "POST")
            ok("dream ok", r.get("ok") is True, str(r)[:150])
            mem2 = (kb / "MEMORY.md").read_text(encoding="utf-8")
            ok("dream 后无「后台巩固」提示行", "后台巩固" not in mem2)
            ok("dream 后 MEMORY.md 仍是记忆结构", mem2.strip().startswith("# 记忆") or "## 2026-08-01" in mem2)

            # 待审目录应为空（全部自动落地）
            pending_files = list((kb / "pending").glob("**/*")) if (kb / "pending").exists() else []
            ok("pending 已清空（自动落地）", not pending_files, str([str(p) for p in pending_files][:5]))

        finally:
            proc.terminate()
    finally:
        mock.terminate()

    print()
    if FAILS:
        print(f"结果: {len(FAILS)} 项失败 — {FAILS}")
        return 1
    print("结果: 全部通过 ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
