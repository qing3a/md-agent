#!/usr/bin/env python3
"""md-agent 项目制（多项目硬隔离）端到端验证：
项目 CRUD / 模板落盘 / 跨项目隔离（检索+文件+会话）/ 全局不泄漏 / 删除保护。
用法: python scripts/e2e-projects.py  （需先 cargo build；MD_AGENT_BIN 可覆盖二进制）
不污染主知识库：临时 kb_root（MD_AGENT_CONFIG 指向临时 config）。
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.request
import urllib.error

BIN = os.environ.get("MD_AGENT_BIN", str(pathlib.Path(__file__).resolve().parent.parent / "target" / "debug" / "md-agent.exe"))
PORT = 8898
BASE = f"http://127.0.0.1:{PORT}"

passed = failed = 0


def ok(name, cond, detail=""):
    global passed, failed
    if cond:
        passed += 1
        print(f"  PASS {name}")
    else:
        failed += 1
        print(f"  FAIL {name}  {detail}")


def api(path, method="GET", body=None, project=None):
    data = json.dumps(body).encode("utf-8") if body else None
    headers = {"Content-Type": "application/json"}
    if project:
        headers["X-Project"] = project
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=headers)
    r = urllib.request.urlopen(req, timeout=15)
    return json.loads(r.read().decode("utf-8"))


def api_status(path, method="GET", body=None, project=None):
    """返回 (status, json)；用于期望 4xx/5xx 的断言"""
    data = json.dumps(body).encode("utf-8") if body else None
    headers = {"Content-Type": "application/json"}
    if project:
        headers["X-Project"] = project
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=headers)
    try:
        r = urllib.request.urlopen(req, timeout=15)
        return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode("utf-8"))
        except Exception:
            return e.code, {}


def wait_health(retries=20):
    for _ in range(retries):
        try:
            urllib.request.urlopen(BASE + "/api/health", timeout=2)
            return True
        except Exception:
            time.sleep(0.5)
    return False


def main():
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="md-agent-e2e-proj-"))
    kb = tmp / "kb"
    kb.mkdir()
    cfg = tmp / "config.json"
    cfg.write_text(json.dumps({"kb_root": str(kb), "llm": {"endpoint": "", "model": "", "api_key": ""},
                               "heartbeat": {"enabled": False, "interval_secs": 5}}), encoding="utf-8")

    proc = subprocess.Popen([BIN, "--no-tray"], env={**os.environ, "MD_AGENT_CONFIG": str(cfg), "MD_AGENT_PORT": str(PORT)},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_health():
            print("FATAL: 服务未启动")
            return 1

        print("== 项目 CRUD ==")
        r = api("/api/projects")
        ok("初始无项目", len(r["projects"]) == 0)

        ra = api("/api/projects", "POST", {"name": "张先生劳动仲裁", "template": "lawyer"})
        ok("创建律师项目 201", "project" in ra and ra["project"]["template"] == "lawyer", str(ra))
        aid = ra["project"]["id"]
        rb = api("/api/projects", "POST", {"name": "研发总监招聘", "template": "headhunter"})
        bid = rb["project"]["id"]
        ok("创建猎头项目", rb["project"]["template"] == "headhunter")

        r = api("/api/projects")
        ok("列表 2 项目", len(r["projects"]) == 2)

        s, _ = api_status("/api/projects", "POST", {"name": "x", "template": "nope"})
        ok("未知模板拒绝", s == 400)
        s, _ = api_status("/api/projects", "POST", {"name": "  ", "template": "blank"})
        ok("空名拒绝", s == 400)

        print("== 模板落盘（lawyer 有证据清单/时间线） ==")
        rd = api("/api/file?path=notes/%E8%AF%81%E6%8D%AE%E6%B8%85%E5%8D%95.md", project=aid)
        ok("律师模板证据清单", "证据" in rd.get("content", ""))
        s, _ = api_status("/api/file?path=notes/%E8%81%8C%E4%BD%8D%E9%9C%80%E6%B1%82.md", project=aid)
        ok("律师项目无猎头模板", s != 200)
        rd = api("/api/file?path=notes/%E8%81%8C%E4%BD%8D%E9%9C%80%E6%B1%82.md", project=bid)
        ok("猎头模板职位需求", "职位" in rd.get("content", ""))

        print("== 跨项目硬隔离 ==")
        api("/api/file", "POST", {"path": "notes/机密.md", "content": "# 机密\n案件A的绝密资料 绝密token"}, project=aid)
        # 全局检索不得命中项目内容
        r = api("/api/search?q=%E7%BB%9D%E5%AF%86&layer=all")
        ok("全局检索不泄漏项目", r["hit_count"] == 0, f"hits={r['hit_count']}")
        # 项目内检索命中
        r = api("/api/search?q=%E7%BB%9D%E5%AF%86&layer=all", project=aid)
        ok("项目A检索命中", r["hit_count"] > 0)
        # 项目B检索不命中 A 的内容
        r = api("/api/search?q=%E7%BB%9D%E5%AF%86&layer=all", project=bid)
        ok("项目B检索隔离", r["hit_count"] == 0)
        # 项目B 读不到 A 的文件
        s, _ = api_status("/api/file?path=notes/%E6%9C%BA%E5%AF%86.md", project=bid)
        ok("项目B读A文件失败", s != 200)
        rd = api("/api/file?path=notes/%E6%9C%BA%E5%AF%86.md", project=aid)
        ok("项目A读A文件成功", rd.get("content", "").find("绝密") != -1)

        print("== 会话项目化 ==")
        api("/api/file", "POST", {"path": "sessions/20260807-120000.md", "content": "---\ntype: session\ntitle: A案会谈\nstatus: active\n---\n# 会话\nA 案内容\n"}, project=aid)
        r = api("/api/sessions", project=aid)
        ok("项目A会话列表", len(r["sessions"]) == 1 and r["sessions"][0]["id"] == "20260807-120000")
        r = api("/api/sessions", project=bid)
        ok("项目B会话为空", len(r["sessions"]) == 0)
        r = api("/api/sessions")
        ok("全局会话为空", len(r["sessions"]) == 0)

        print("== 重命名 / 删除保护 / 删除 ==")
        r = api(f"/api/projects/{aid}", "PATCH", {"name": "张先生劳动仲裁（改）"})
        ok("重命名", r["project"]["name"].find("改") != -1)
        s, _ = api_status("/api/projects/default", "DELETE")
        ok("default 不可删", s != 200)
        s, _ = api_status("/api/projects/%20%20", "DELETE")
        ok("非法 id 不可删", s != 200)
        s, _ = api_status(f"/api/projects/{bid}", "DELETE")
        ok("删除项目B", s == 200)
        r = api("/api/projects")
        ok("删除后剩1项目", len(r["projects"]) == 1)
        s, _ = api_status("/api/search?q=x", project=bid)
        ok("删除后访问报错", s != 200)

        print(f"\n结果: {passed} 通过, {failed} 失败")
        return 0 if failed == 0 else 1
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
