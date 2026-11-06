# opencode-rs process plugins

A plugin is a separate executable, written in any language, that speaks
JSON-RPC 2.0 over stdio: one JSON message per line on stdin/stdout. stderr is
ignored. Plugins are launched once per opencode session in the project cwd.

## Discovery

1. `plugin` in opencode.json:
   `{ "plugin": { "guard": { "command": ["python3", "guard.py"], "environment": {"K":"V"}, "enabled": true, "timeout": 10000 } } }`
   (`timeout` = per-hook timeout in ms, default 10000)
2. Executable files in `~/.config/opencode/plugins/` and `<cwd>/.opencode/plugins/`
   (non-executable files are skipped with a warning; file name = plugin name).
   Earlier definitions win on name clashes.

## Handshake

opencode sends request `initialize`:

    {"protocolVersion": 1, "cwd": "/path/to/project", "opencodeVersion": "0.1.0"}

The plugin answers with what it wants:

    {"hooks": ["tool.execute.before", "tool.execute.after", "event"],
     "tools": [{"name": "current_time", "description": "...", "inputSchema": {"type":"object"}}]}

A plugin is only called for hooks it listed.

## Hooks

| Method | Kind | Params | Result |
|---|---|---|---|
| `tool.execute.before` | request | `{tool, input}` | `{input?}` rewrites the input. A JSON-RPC **error** blocks the call; its message becomes the tool result. |
| `tool.execute.after` | request | `{tool, input, output}` | `{output?}` rewrites the output (string). Errors are logged and ignored. |
| `event` | notification (no id, no reply) | `{sessionId, event}` | `event` is an AgentEvent, e.g. `{"type":"tool_start","id":..,"name":..,"input":..}`, `text_delta`, `tool_end`, `done`, `error`. |
| `tool.execute` | request | `{name, input, cwd}` | `{output}` for tools the plugin declared in `initialize`. An error becomes a tool error. |

## Failure handling

Each request has a timeout (hooks: 10 s by default, `tool.execute`: 120 s). If a
plugin crashes, closes stdout, or times out, a warning is logged, the plugin is
disabled for the rest of the session, and the agent continues unaffected (hooks
fail open). A JSON-RPC error reply is a normal answer, not a failure.

## Python example

```python
#!/usr/bin/env python3
import sys, json
for line in sys.stdin:
    msg = json.loads(line)
    m, p, i = msg.get("method"), msg.get("params", {}), msg.get("id")
    if m == "initialize":
        res = {"hooks": ["tool.execute.before"], "tools": []}
    elif m == "tool.execute.before":
        if p["tool"] == "bash" and "rm -rf /" in p["input"].get("command", ""):
            print(json.dumps({"jsonrpc": "2.0", "id": i, "error": {"code": -32000, "message": "no"}}), flush=True)
            continue
        res = {}
    else:
        continue  # notifications (event) and unknown methods
    print(json.dumps({"jsonrpc": "2.0", "id": i, "result": res}), flush=True)
```

A Rust example lives in `src/bin/example_plugin.rs`.

## Embedding

Rust code can use `oc_ext::load_plugins_with_tools(config, cwd, &mut registry)`
(hooks + plugin tools) or `load_plugins` (hooks only). In-process plugins
implement `oc_ext::Plugin` and are registered in a `PluginRegistry`, which is
itself a `ToolHook`.
