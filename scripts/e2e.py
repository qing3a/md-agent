#!/usr/bin/env python3
"""md-agent 端到端验证：隔离 kb 起服务，一键跑待审四型审批链路（note/memory/skill/consolidate）+ 巩固器。
用法: python scripts/e2e.py  （需先 cargo build；默认用 target/debug/md-agent.exe，可用 MD_AGENT_BIN 覆盖）
不污染主知识库：用临时目录作 kb_root（MD_AGENT_CONFIG 指向临时 config，config.kb_root 优先于 env）。
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

BIN = os.environ.get("MD_AGENT_BIN", str(pathlib.Path(__file__).resolve().parent.parent / "target" / "debug" / "md-agent.exe"))
PORT = 8899
BASE = f"http://127.0.0.1:{PORT}"

passed = failed = 0


def ok(name, cond):
    global passed, failed
    if cond:
        passed += 1
        print(f"  PASS {name}")
    else:
        failed += 1
        print(f"  FAIL {name}")


def api(path, method="GET", body=None):
    data = json.dumps(body).encode("utf-8") if body else None
    req = urllib.request.Request(BASE + path, data=data, method=method, headers={"Content-Type": "application/json"})
    r = urllib.request.urlopen(req, timeout=15)
    return json.loads(r.read().decode("utf-8"))


def wait_health(retries=20):
    for _ in range(retries):
        try:
            urllib.request.urlopen(BASE + "/api/health", timeout=2)
            return True
        except Exception:
            time.sleep(0.5)
    return False


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="md-agent-e2e-"))
    kb = tmp / "kb"
    kb.mkdir()
    cfg = tmp / "config.json"
    cfg.write_text(json.dumps({"kb_root": str(kb), "llm": {"endpoint": "", "model": "", "api_key": ""},
                               "heartbeat": {"enabled": False, "interval_secs": 5}}), encoding="utf-8")

    proc = subprocess.Popen([BIN, "--no-tray"], env={**os.environ, "MD_AGENT_CONFIG": str(cfg), "MD_AGENT_PORT": str(PORT)},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_health():
            print("FATAL: 服务未启动"); return 1

        def wf(rel, content):
            p = kb / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(content, encoding="utf-8")

        print("== note/memory 回归 ==")
        wf("pending/MEMORY.t.md", "## 2026-08-03\n- 记忆条目\n")
        r = api("/api/kb/pending/approve", "POST", {"path": "pending/MEMORY.t.md"})
        ok("memory 合并", r.get("ok") and r["ok"][0]["target"] == "MEMORY.md")
        ok("MEMORY 落盘", "记忆条目" in (kb / "MEMORY.md").read_text(encoding="utf-8"))
        wf("pending/notes/新.md", "# 新笔记\n正文\n")
        r = api("/api/kb/pending/approve", "POST", {"path": "pending/notes/新.md"})
        ok("note 落地", r.get("ok") and (kb / "notes" / "新.md").exists())

        print("== skill 提案 ==")
        wf("pending/SKILL.整理网页.md",
           "---\ntype: skill\ntitle: 整理网页笔记\ntrigger: 整理网页\ndesc: 抓网页转笔记\n---\n# 整理网页笔记\n步骤\n")
        r = api("/api/kb/pending/approve", "POST", {"path": "pending/SKILL.整理网页.md"})
        ok("skill 安装", r.get("ok") and (kb / "skills" / "整理网页.md").exists())
        idx = (kb / "skills" / "INDEX.md").read_text(encoding="utf-8")
        ok("技能注册表", "整理网页" in idx)
        sk = api("/api/skills")
        ok("/api/skills 列表", any(s["trigger"] == "整理网页" for s in sk["skills"]))

        print("== consolidate 提案 ==")
        wf("pending/CONSOLIDATE.c.md",
           "---\ntype: consolidate\ntarget: MEMORY.md\n---\n# MEMORY\n- 巩固后内容\n")
        r = api("/api/kb/pending/approve", "POST", {"path": "pending/CONSOLIDATE.c.md"})
        ok("consolidate 替换", r.get("ok") and "巩固后内容" in (kb / "MEMORY.md").read_text(encoding="utf-8"))

        print("== 巩固器 ==")
        (kb / "MEMORY.md").write_text("# MEMORY\n- A\n- A\n- B\n", encoding="utf-8")
        r = api("/api/consolidate", "POST")
        created = [c for c in r.get("created", []) if "CONSOLIDATE.MEMORY" in c]
        ok("巩固器生成去重提案", len(created) >= 1)
        if created:
            prop = (kb / created[0]).read_text(encoding="utf-8")
            ok("去重内容正确", prop.count("- A") == 1 and "- B" in prop)

        print("== 巩固器 v2 守卫 ==")
        try:
            api("/api/consolidate?llm=1", "POST")
            ok("v2 未配置 LLM 应报错", False)
        except urllib.error.HTTPError as e:
            ok("v2 未配置 LLM 应报错", e.code == 400)

        print(f"\n结果: {passed} 通过, {failed} 失败")
        return 1 if failed else 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
