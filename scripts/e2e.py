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
        # 2026-08-11 起巩固提案自动落地（git 自动提交即回滚通道）：pending 文件已被 auto_land 移除并替换 MEMORY.md——
        # 断言改为验证落地后的 MEMORY.md 已去重（比读提案文件更强：验证生效而非仅生成）
        mem = (kb / "MEMORY.md").read_text(encoding="utf-8")
        ok("去重内容正确（自动落地）", mem.count("- A") == 1 and "- B" in mem)

        print("== 巩固器 v2 守卫 ==")
        try:
            api("/api/consolidate?llm=1", "POST")
            ok("v2 未配置 LLM 应报错", False)
        except urllib.error.HTTPError as e:
            ok("v2 未配置 LLM 应报错", e.code == 400)

        print("== Agent 回路（mock LLM）==")
        # 起 mock LLM（scripts/mock_llm.py，OpenAI 兼容）→ 配置后端 → 验证 /api/agent 主回路 + 子回路
        mock_port = 11435
        mock_llm = pathlib.Path(__file__).resolve().parent / "mock_llm.py"
        mock_proc = subprocess.Popen(
            [sys.executable, str(mock_llm), str(mock_port)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            time.sleep(1.5)  # mock 服务就绪
            cfg_new = {"kb_root": str(kb),
                       "llm": {"endpoint": f"http://127.0.0.1:{mock_port}", "model": "mock", "api_key": "", "embedding": {"endpoint": "", "model": "", "api_key": ""}},
                       "heartbeat": {"enabled": False, "interval_secs": 5}}
            api("/api/config", "POST", cfg_new)

            # 主回路：prompt 含「调用工具」→ mock 首轮返回 search 工具 JSON → 宿主执行（空库无命中）→ 回填 → 第二轮收敛回答
            r = api("/api/agent", "POST", {"prompt": "调用工具 查一下 托盘 架构"})
            ok("agent 主回路 ok", r.get("ok") is True)
            ok("agent 主回路工具调用", r.get("tool_calls", 0) >= 1 and "search" in (r.get("tools_used") or []))
            ok("agent 主回路回答", bool(r.get("answer")))

            # 子 agent（spawn）：独立上下文 + 受限策略——同一条 mock 触发链应在子回路内走通
            r2 = api("/api/agent", "POST", {"prompt": "调用工具 查一下 记忆 分片", "spawn": True})
            ok("agent spawn 子回路 ok", r2.get("ok") is True)
            ok("agent spawn 回答", bool(r2.get("answer")))

            # 子 agent 越权守卫：非白名单工具（fetch=网络侧效应）→ 拒绝回填、不执行、收敛为回答
            # mock 只模拟 search 工具，越权由 Rust 单测覆盖；此处验证未配置 LLM 时主回路守卫
            api("/api/config", "POST", {"kb_root": str(kb),
                                        "llm": {"endpoint": "", "model": "", "api_key": "", "embedding": {"endpoint": "", "model": "", "api_key": ""}},
                                        "heartbeat": {"enabled": False, "interval_secs": 5}})
            try:
                api("/api/agent", "POST", {"prompt": "hi"})
                ok("agent 未配置 LLM 应 400", False)
            except urllib.error.HTTPError as e:
                ok("agent 未配置 LLM 应 400", e.code == 400)
        finally:
            mock_proc.terminate()
            try:
                mock_proc.wait(timeout=5)
            except Exception:
                mock_proc.kill()

        print("== 跨会话记忆 recall ==")
        # M3：提问前记忆召回（只读）——MEMORY.md 已在巩固器段落写入内容
        r = api("/api/memory/recall", "POST", {"q": "记忆", "k": 3})
        ok("recall 返回 hits", "hits" in r and isinstance(r["hits"], list))

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
