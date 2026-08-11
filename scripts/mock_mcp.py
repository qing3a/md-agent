#!/usr/bin/env python3
"""mock MCP server（stdio）：固定 3 个工具，回显参数——md-agent MCP 客户端协议测试用。

工具：
- echo：回显 message 参数
- add：两数相加（a + b）
- status：返回固定状态文本

用法: python scripts/mock_mcp.py   （stdio 传输，由 md-agent 客户端 spawn，无端口）
"""
import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "回显消息",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string", "description": "要回显的文本"}},
            "required": ["message"],
        },
    },
    {
        "name": "add",
        "description": "两数相加",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"],
        },
    },
    {
        "name": "status",
        "description": "返回固定状态",
        "inputSchema": {"type": "object", "properties": {}},
    },
]


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        id_ = msg.get("id")
        method = msg.get("method")
        params = msg.get("params") or {}
        if method == "initialize":
            result = {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.0.1"},
            }
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            result = {"tools": TOOLS}
        elif method == "tools/call":
            name = params.get("name")
            args = params.get("arguments") or {}
            if name == "echo":
                result = {"content": [{"type": "text", "text": "echo: " + str(args.get("message", ""))}]}
            elif name == "add":
                result = {"content": [{"type": "text", "text": str(args.get("a", 0) + args.get("b", 0))}]}
            elif name == "status":
                result = {"content": [{"type": "text", "text": "mock status ok"}]}
            else:
                sys.stdout.write(
                    json.dumps({"jsonrpc": "2.0", "id": id_, "error": {"code": -32602, "message": f"unknown tool: {name}"}}) + "\n"
                )
                sys.stdout.flush()
                continue
        else:
            sys.stdout.write(
                json.dumps({"jsonrpc": "2.0", "id": id_, "error": {"code": -32601, "message": f"method not found: {method}"}}) + "\n"
            )
            sys.stdout.flush()
            continue
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": id_, "result": result}) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
