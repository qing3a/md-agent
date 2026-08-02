#!/usr/bin/env python3
"""mock LLM：OpenAI 兼容 /v1/chat/completions 服务（md-agent 开发/测试用）。

回显收到的 model / messages / stream 字段，用于验证代理转发是否正确。
- stream=false → JSON 响应
- stream=true  → SSE 分块响应
- 用户消息含「记住/沉淀」→ 响应末尾附写回块（测试 Agent 写回链路）
用法: python scripts/mock_llm.py [port]   默认 11434（模拟 Ollama 端口）
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 11434


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw)
        except Exception as e:  # noqa: BLE001
            self.send_error(400, f"bad json: {e}")
            return

        model = body.get("model", "(无)")
        messages = body.get("messages", [])
        user_msgs = [m.get("content", "") for m in messages if m.get("role") == "user"]
        last_user = user_msgs[-1] if user_msgs else "(无 user 消息)"
        stream = body.get("stream") is True

        content = (
            "【mock 回答】消息数=%d，模型=%s，stream=%s。最后一条用户消息：%s"
            % (len(messages), model, stream, last_user[:80])
        )
        # 触发词：最后一条用户消息含「记住/沉淀」时返回写回块（测试 Agent 写回链路）
        if ("记住" in last_user) or ("沉淀" in last_user):
            content += (
                "\n\n<!-- md-agent-save -->\n"
                + json.dumps(
                    {
                        "path": "notes/rag/mock沉淀.md",
                        "mode": "new",
                        "content": "# mock 沉淀笔记\n\n通过 Agent 写回链路自动沉淀的内容（触发词：记住/沉淀）。",
                    },
                    ensure_ascii=False,
                )
            )

        if stream:
            self._send_sse(content, model)
        else:
            self._send_json(content, model)

    def _send_json(self, content, model):
        resp = {
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": content},
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }
        data = json.dumps(resp, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _send_sse(self, content, model):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def emit(delta):
            d = json.dumps(
                {"choices": [{"delta": {"content": delta}}]}, ensure_ascii=False
            )
            self.wfile.write(("data: " + d + "\n\n").encode("utf-8"))
            self.wfile.flush()
            time.sleep(0.03)  # 模拟逐字输出

        step = 4
        for i in range(0, len(content), step):
            emit(content[i : i + step])
        emit("\n")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, fmt, *args):
        sys.stderr.write("[mock-llm] " + (fmt % args) + "\n")


if __name__ == "__main__":
    print(f"mock LLM listening on 127.0.0.1:{PORT} (POST /v1/chat/completions, stream 支持)")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
