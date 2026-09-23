#!/usr/bin/env python3
"""A minimal, real MCP server (stdio JSON-RPC transport) used only to
exercise src/mcp/mod.rs's McpClient against actual wire traffic in
tests. Not part of the shipped product — a test fixture, same role as
any other tests/fixtures file.

Implements exactly enough of the spec for those tests: `initialize`,
`notifications/initialized` (ignored — no reply expected), `tools/list`
(one tool, "echo"), and `tools/call` (echoes its "text" argument back,
or returns an MCP-protocol error for any other tool name).
"""
import json
import sys


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = msg.get("method")
        msg_id = msg.get("id")

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-mcp-server", "version": "0.0.1"},
                },
            })
        elif method == "notifications/initialized":
            pass  # notification — no reply
        elif method == "tools/list":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echoes back the 'text' argument.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"text": {"type": "string"}},
                                "required": ["text"],
                            },
                        }
                    ]
                },
            })
        elif method == "tools/call":
            params = msg.get("params", {})
            name = params.get("name")
            args = params.get("arguments", {})
            if name == "echo":
                send({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {"content": [{"type": "text", "text": args.get("text", "")}], "isError": False},
                })
            else:
                send({
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {"content": [{"type": "text", "text": f"unknown tool: {name}"}], "isError": True},
                })
        elif msg_id is not None:
            send({"jsonrpc": "2.0", "id": msg_id, "error": {"code": -32601, "message": f"method not found: {method}"}})


if __name__ == "__main__":
    main()
