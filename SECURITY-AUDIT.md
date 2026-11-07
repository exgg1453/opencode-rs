# opencode-rs — Saldırgan Gözüyle Güvenlik Denetimi

Kapsam: kaynak kod incelemesi (~12k LOC), saldırı yüzeyi haritalama, sömürü zinciri analizi.
İlk denetim: 2026-11-07. Yetki: proje sahibinin kendi talebi.

Özet: 1. turda bulunan zincirlenebilir **1 kritik (uzaktan RCE)**, bypass edilebilir izin
katmanı (**yüksek**), proje sınırı kaçışı (**orta**) ve bir düşük bulgu.

## Düzeltme turu 1 — durum: #1–#4 KAPATILDI (2026-11-07)

Her düzeltme için gösteren testler eklendi; `cargo test --workspace` (208 test) ve
`cargo clippy --workspace --all-targets -- -D warnings` temiz.

---

## #1 — KRİTİK: Kimlik doğrulamasız yerel sunucu → drive-by RCE — ✅ DÜZELTİLDİ

`oc-server` hiçbir endpoint'te kimlik doğrulaması yapmıyordu, `CorsLayer::permissive()`
kullanıyordu ve `POST /session` gövdedeki `cwd`'yi saldırgana seçtiriyordu. Varsayılan izin
kuralı `("*","*",Allow)` ile bash sormadan çalışıyordu. Tarayıcıdaki zararlı bir sayfa
CSRF ile `127.0.0.1:4096`'ya POST atıp ajana bash çalıştırabiliyordu → tam RCE + `auth.json` sızması.

**Yapılan:** `crates/oc-server/src/security.rs` eklendi, router'a middleware olarak bağlandı:
- **Bearer token:** başlangıçta rastgele ~244-bit token üretilir (ya da `OPENCODE_SERVER_TOKEN`),
  stderr'e yazdırılır. `/health` ve share *okuma/secret'lı yazma* dışındaki her istekte zorunlu.
  Sabit zamanlı karşılaştırma. `EventSource` header koyamadığı için `/event` ayrıca `?token=` kabul eder (yalnızca /event'te).
- **Origin allowlist:** izin verilmeyen `Origin` reddedilir → tarayıcıdaki çapraz-site sayfa,
  token sızsa bile isteği sürremez (CSRF kapandı). `CorsLayer::permissive()` kaldırıldı; CORS
  yalnızca `OPENCODE_CORS_ORIGINS` için açılır.
- **Host allowlist:** loopback'te yalnızca loopback host adları kabul edilir → DNS-rebinding kapandı.
- **cwd hapsi:** `POST /session` ve her prompt `Security::session_dir` ile sunucunun kök dizinine
  `canonicalize` üzerinden hapsedilir; symlink/`..` ile dışarı çıkılamaz.

Testler: `requires_bearer_token`, `blocks_dns_rebinding_hosts`, `blocks_cross_site_pages`
(audit'teki PoC token sızsa bile 403), `session_cwd_cannot_leave_root`,
`share_creation_needs_token_but_pages_are_public`, `default_state_is_locked`
(`AppState::new` güvensiz varsayılana düşmez).

---

## #2 — YÜKSEK: Bash izin kuralları trivial bypass — ✅ DÜZELTİLDİ

`deny` kuralları komutun ham metnine prefix-glob ile uygulanıyordu; normalizasyon yoktu.

**Yapılan:** `crates/oc-core/src/bashcmd.rs` — komut satırını gerçekten çözümleyen `analyze_bash`:
- ayırıcılara (`; & | && || newline`) ve `$( )`/backtick ikamelerine bölür (tırnak farkında);
- her sözcüğün tırnak/escape'ini çözer, program adını **basename**'e indirger →
  `\rm`, `r""m`, `/bin/rm`, `'rm'` hepsi `rm`'e normalize olur;
- sarmalayıcıları açar: `sudo`, `env`, `command`, `nohup`, `timeout <süre>`, `VAR=val …`, `busybox`…;
- komut çalıştıranları rekürsif denetler: `sh -c '<script>'`, `find -exec … ;`, `xargs …`;
- yönlendirme hedeflerini (`>`, `>>`, `2>`) toplar → her biri **`edit` iznine** tabi tutulur,
  böylece `echo x > ~/.bashrc` artık edit iznini atlayamaz;
- çözümleyemediği satırda `uncertain` işaretler → izin katmanı **en az "ask"** yapar (fail-closed).

İzin katmanı (`permission.rs::items`) artık `split_commands` yerine bu çözümlemeyi kullanır.
Testler: `path_prefix_is_stripped`, `quote_and_escape_splitting_is_defeated`,
`wrappers_are_unwrapped`, `sh_c_is_recursed`, `find_exec_is_recursed`, `xargs_is_recursed`,
`redirection_is_a_write`, `unbalanced_quotes_are_uncertain` (toplam 15 birim test).

---

## #3 — ORTA: Symlink ile proje sınırı kaçışı — ✅ DÜZELTİLDİ

`normalize_path` yalnızca leksikti; cwd içindeki `data -> /etc` symlink'i ile `write data/x`
sınır kontrolünü atlatıp `/etc/x`'e yazabiliyordu. `.env` kuralı da büyük/küçük harf duyarlıydı.

**Yapılan:** `permission.rs::real_path` — yolun **var olan** kısmını `canonicalize` ile çözer
(symlink'leri izleyerek), kalan parçayı ekler. İzin kontrolü artık gerçek yol üzerinden yapılır;
cwd içindeki symlink dışarı gösteriyorsa `external_directory` tetiklenir. `.env` eşleşmesi artık
yol ham + küçük-harf formuyla yapılıp daha kısıtlayıcı sonuç alınır → `.ENV` de sorulur.
Testler: `real_path_follows_symlinks_out_of_cwd`, `env_file_case_insensitive` (+ mevcut yol testleri).

---

## #4 — ORTA/DÜŞÜK: Kimlik doğrulamasız share create → disk DoS — ✅ DÜZELTİLDİ

`POST /share` artık #1'deki bearer token'ı istiyor (share *okuma* uçları public kalır,
*yazma* uçları zaten per-share secret ister). Anonim DoS kapandı.
Test: `share_creation_needs_token_but_pages_are_public`.

---

## İyi olan taraflar (1. turda teyit edildi, değişmedi)
- `share_html.rs` XSS'e karşı sağlam: tüm metin önce escape, markdown inline sonra.
- `valid_share_id` path-traversal'ı engelliyor; `secrets_match` sabit-zamanlı; share secret 256-bit.
- `auth.json` 0600, atomik rename, tmp de private.

## 2. tur için not
Yapay zekaya yeniden denetlettirilecek. Bakılacak yeni/kalan yüzey: proje düzeyi
`opencode.json`/`.opencode/plugins` ile klonlanan depoda otomatik komut çalışması (mcp/lsp/plugin
komutları), prompt injection (araç çıktısı/MCP/dosya içeriğiyle ajan yönlendirme),
eklenti araç adı çakışması, paylaşılan oturumda API anahtarı/gizli veri sızması, bash tool
sembolik bağ TOCTOU.
