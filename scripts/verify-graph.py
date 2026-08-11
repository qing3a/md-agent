#!/usr/bin/env python3
"""知识图谱全端点实机验证（隔离 kb + [[双链]] 文档）：
sync/stats/graph 数据/paths BFS/orphans/激活扩散检索/补链建议。
用法: python scripts/verify-graph.py
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

PORT = 18758
BASE = f"http://127.0.0.1:{PORT}"
BIN = os.environ.get("MD_AGENT_BIN", str(pathlib.Path(__file__).resolve().parent.parent / "target" / "debug" / "md-agent.exe"))
FAILS = []


def ok(name, cond, extra=""):
    print(("  PASS " if cond else "  FAIL ") + name + (f" — {extra}" if extra else ""))
    if not cond:
        FAILS.append(name)


def api(path, method="GET", body=None, timeout=30):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="md-agent-graph-"))
    kb = tmp / "kb"
    kb.mkdir()
    cfg = tmp / "config.json"
    cfg.write_text(json.dumps({"kb_root": str(kb), "llm": {"endpoint": "", "model": "", "api_key": ""},
                               "heartbeat": {"enabled": False, "interval_secs": 5}}), encoding="utf-8")

    def wf(rel, content):
        p = kb / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")

    # 三个互链文档 + 一个孤立文档（类型模板 frontmatter）
    wf("notes/架构/托盘应用.md", "---\ntype: note\n---\n# 托盘应用\n\n## 设计\n桌面常驻，参考 [[会话管理]] 与 [[记忆分层]]。\n")
    wf("notes/会话管理.md", "---\ntype: note\n---\n# 会话管理\n\n## 方案\n与 [[托盘应用]] 联动，见 [[记忆分层]]。\n")
    wf("notes/记忆分层.md", "---\ntype: note\n---\n# 记忆分层\nL1 规范，L2 正文，[[托盘应用]] 已采纳。\n")
    wf("notes/孤立文档.md", "---\ntype: note\n---\n# 孤立文档\n没有任何链接。\n")

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

        print("== 图谱同步与统计 ==")
        r = api("/api/graph/sync", "POST")
        ok("graph sync ok", r.get("ok") is True, str(r)[:120])
        r = api("/api/graph/stats")
        s = r if isinstance(r, dict) else {}
        ok("stats 文档数>=4", s.get("docs", 0) >= 4, str(s)[:150])
        ok("stats 链接数>=4", s.get("links", 0) >= 4, str(s)[:150])

        print("== 图谱数据 ==")
        r = api("/api/graph/graph")
        nodes = {n.get("id") or n.get("title"): n for n in r.get("nodes", [])}
        edges = r.get("edges", [])
        ok("graph 节点含四文档", all(t in nodes for t in ["托盘应用", "会话管理", "记忆分层", "孤立文档"]),
           str(list(nodes.keys())))
        ok("graph 边含 托盘→会话管理", any("托盘应用" in str(e) and "会话管理" in str(e) for e in edges))

        print("== 关系路径 BFS ==")
        from_p = urllib.parse.quote("notes/会话管理.md")
        to_p = urllib.parse.quote("notes/记忆分层.md")
        r = api(f"/api/graph/paths?from={from_p}&to={to_p}")
        ok("paths 直达 记忆分层", any(n.get("title") == "记忆分层" for n in r.get("path", [])), str(r)[:150])

        print("== 悬空/孤立 ==")
        r = api("/api/graph/orphans")
        ok("孤立检测含 孤立文档", any("孤立文档" in str(o) for o in r.get("orphans", [])), str(r)[:150])

        print("== 检索激活扩散（图谱联动）==")
        r = api("/api/search?q=" + urllib.parse.quote("桌面常驻") + "&layer=notes&ctx=1&expand=1")
        hit_files = [h.get("file") for h in r.get("hits", [])]
        related = [x.get("title") for x in r.get("related", [])]
        ok("expand 命中相关文档", "notes/架构/托盘应用.md" in hit_files, str(hit_files))
        ok("expand related 含邻居", len(related) > 0, str(related))

        print("== 图谱自动补链建议 ==")
        content = (kb / "notes/孤立文档.md").read_text(encoding="utf-8")
        r = api("/api/link/suggest", "POST", {"content": content})
        ok("link suggest 返回候选", r.get("ok") is True and len(r.get("links", [])) > 0, str(r)[:150])
    finally:
        proc.terminate()

    print()
    if FAILS:
        print(f"结果: {len(FAILS)} 项失败 — {FAILS}")
        return 1
    print("结果: 图谱全部通过 ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
