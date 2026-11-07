# opencode-rs

[OpenCode](https://github.com/sst/opencode) yapay zeka kodlama ajanının Rust ile sıfırdan yazılmış sürümü.

## Kurulum

Hazır derlenmiş programı [Releases](https://github.com/exgg1453/opencode-rs/releases/latest) sayfasından indirin; Rust kurmanız gerekmez.

**Linux / macOS:**

```sh
# Kendi sisteminize uyan satırı seçin:
T=x86_64-unknown-linux-musl     # Linux, Intel/AMD
T=aarch64-unknown-linux-musl    # Linux, ARM (Raspberry Pi, Termux/proot, sunucular)
T=aarch64-apple-darwin          # macOS, Apple Silicon (M1/M2/M3...)
T=x86_64-apple-darwin           # macOS, Intel

curl -fsSL "https://github.com/exgg1453/opencode-rs/releases/latest/download/opencode-$T.tar.gz" | tar -xz
sudo install "opencode-$T/opencode" /usr/local/bin/   # ya da: mv "opencode-$T/opencode" ~/.local/bin/
```

**Windows:** [opencode-x86_64-pc-windows-msvc.zip](https://github.com/exgg1453/opencode-rs/releases/latest/download/opencode-x86_64-pc-windows-msvc.zip) dosyasını indirip açın, içindeki `opencode.exe`'yi çalıştırın.

Her arşivin yanında doğrulama için bir `.sha256` dosyası vardır.

## Kullanım

```sh
opencode auth login anthropic        # API anahtarını bir kez kaydeder
opencode                             # terminal arayüzü
opencode run "bu projeyi açıkla"     # tek seferlik, arayüzsüz
opencode models                      # desteklenen sağlayıcılar
```

## Crate'ler

| Crate | Görevi |
|---|---|
| `oc-ai` | LLM sağlayıcıları (Anthropic, OpenAI uyumlu, Gemini, Azure...), SSE akışı |
| `oc-core` | Ajan döngüsü, oturumlar, yapılandırma, izinler, yerleşik araçlar |
| `oc-cli` | `opencode` programı: `run`, `tui`, `serve`, `auth`, `share`, `sessions`, `models` |
| `oc-tui` | ratatui tabanlı terminal arayüzü ve temalar |
| `oc-server` | axum HTTP API, SSE olay akışı, oturum paylaşma |
| `oc-ext` | MCP istemcisi ve süreç eklentileri ([PLUGINS.md](crates/oc-ext/PLUGINS.md)) |
| `oc-lsp` | Dil sunucusu (LSP) tanılamaları |

## Kaynaktan derleme

```sh
cargo install --path crates/oc-cli
```

## Yeni sürüm yayınlama

`v` ile başlayan bir etiket gönderildiğinde GitHub Actions tüm platformlar için derleyip Releases'e ekler:

```sh
git tag v0.1.1 && git push origin v0.1.1
```
