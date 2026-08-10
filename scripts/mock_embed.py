#!/usr/bin/env python3
"""mock embedding：OpenAI 兼容 /v1/embeddings 服务（md-agent 语义召回链路测试用）。

无真实模型——基于预设关键词词袋生成确定性向量（同一文本恒同向量）：
- 文本含某关键词 → 该维 +1（计数），再 L2 归一化；
- 查询与文本共享关键词越多 → cosine 越高 → 语义召回链路（embed→建索引→检索→RRF 融合）真实走通。
用法: python scripts/mock_embed.py [port]   默认 11436
"""
import json
import math
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 11436

# 预设关键词词袋（维度 = 词表长度；覆盖 md-agent 领域词，足够验证召回链路）
WORDS = [
    "架构", "记忆", "检索", "图谱", "托盘", "会话", "待审", "任务",
    "应用", "技能", "风险", "案件", "证据", "决策", "经验", "事实",
    "搜索", "向量", "代理", "mcp", "知识", "笔记", "项目", "整理",
]


def embed_one(text: str):
    vec = [0.0] * len(WORDS)
    low = text.lower()
    for i, w in enumerate(WORDS):
        vec[i] = low.count(w.lower())
    # 非词表字符 n-gram 轻量信号（让"桌面常驻程序"等也产生差异向量，而非全零）
    if sum(vec) == 0:
        h = 0
        for ch in text:
            h = (h * 31 + ord(ch)) % 100000
        for i in range(len(WORDS)):
            vec[i] = ((h >> (i % 8)) & 0xFF) / 255.0
    norm = math.sqrt(sum(x * x for x in vec)) or 1.0
    return [x / norm for x in vec]


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
        inputs = body.get("input", [])
        if isinstance(inputs, str):
            inputs = [inputs]
        data = [
            {"embedding": embed_one(str(t)), "index": i}
            for i, t in enumerate(inputs)
        ]
        resp = {"model": model, "data": data}
        payload = json.dumps(resp, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):  # 静默
        pass


if __name__ == "__main__":
    print(f"mock embedding 服务: http://127.0.0.1:{PORT}（关键词词袋维度 {len(WORDS)}）")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
