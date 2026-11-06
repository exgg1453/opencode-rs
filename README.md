# opencode-rs

[OpenCode](https://github.com/sst/opencode) yapay zeka kodlama ajanının Rust ile sıfırdan yazılmış sürümü.

## Crate'ler

| Crate | Görevi |
|---|---|
| `oc-ai` | LLM sağlayıcıları (Anthropic, OpenAI uyumlu), SSE akışı |
| `oc-core` | Ajan döngüsü, oturumlar, yapılandırma, yerleşik araçlar |
| `oc-cli` | `opencode` ikili dosyası: `run`, `tui`, `serve`, `sessions`, `models` |
| `oc-tui` | ratatui tabanlı terminal arayüzü |
| `oc-server` | axum HTTP API ve SSE olay akışı |
| `oc-ext` | MCP istemcisi ve eklenti altyapısı |

## Derleme

```sh
cargo build --release
ANTHROPIC_API_KEY=... ./target/release/opencode
```
