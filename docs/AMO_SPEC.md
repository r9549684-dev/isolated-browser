# ТЗ: AMO (Active Management & Optimization) — Multi-Gateway Failover
# Isolated Browser — Android + iOS
# Дата: 2026-07-15
# Подтверждено Ревизором (claude-sonnet-5): вариант C — Rust core first

---

## 1. РЕШЕНИЕ РЕВИЗОРА

**Вариант C: Rust core first.**

Обоснование:
- Rust transport-core компилируется в одинаковый бинар для Android (.so) и iOS (.a)
- FFI контракт `transport_start` — общий для платформ
- DPI-fingerprint-safe failover (по timeout, не по error) — протокольное решение, должно быть в единой реализации
- Если делать на Android сначала → iOS унаследует полусырой FFI контракт
- Параллельно (B) → две команды соревнуются на одной FFI поверхности

**Порядок:** Rust core → FFI → Dart bindings → Flutter UI → Android test → iOS test

---

## 2. ТЕКУЩЕЕ СОСТОЯНИЕ

| Компонент | Текущее состояние |
|-----------|------------------|
| `failover.rs` | `FailoverManager` реализован, тесты 7/7 pass, **НЕ интегрирован** |
| `proxy.rs` | `Socks5Proxy::new(bind, host:String, port, key, sni, server_pub, psk)` — один endpoint |
| `lib.rs` FFI | `transport_start(socks_port, host, port, key32, sni, server_pub32, psk32, rate_limit)` — один host |
| `settings_service.dart` | Один `gatewayHost`, один `gatewayPort` |
| Gateway | Один: `38.180.253.219:9443` |
| UI | Settings Screen: одно поле host + port |

---

## 3. АРХИТЕКТУРА AMO

```
[Flutter UI: Settings — список endpoints]
    ↓ (JSON array)
[transport_start(endpoints_json, key32, rate_limit)]
    ↓ parse + validate
[FailoverManager::new(Vec<GatewayEndpoint>)]
    ↓
[Socks5Proxy] — на каждое соединение:
    endpoint = failover.current()
    connect → handshake
    if timeout → failover.report_failure()
    if cooldown elapsed → failover.try_switch()
    ↓
[Steal-TLS туннель] → [endpoint N]
```

**Правила failover (из failover.rs + правки Ревизора):**
- Deterministic priority list (не dynamic discovery)
- Failover triggers:
  - Timeout (основной триггер)
  - Connection refused (endpoint мертв)
  - DNS resolution failure
  - TLS handshake failure (до начала протокола)
- НЕ failover on: protocol error mid-handshake (DPI fingerprint risk)
- Cooldown: PER-ENDPOINT (не global) — помечать плохой endpoint,
  другие retry немедленно. Global cooldown блокирует здоровые endpoints.
- Min interval между переключениями одного endpoint: 60s
- Список endpoints из config

---

## 4. ПЛАН РЕАЛИЗАЦИИ — RUST CORE (transport-core)

### Шаг 1: Cargo.toml
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"  # для парсинга endpoints из FFI
```

### Шаг 2: Новая структура EndpointConfig (failover.rs или новый модуль)
```rust
#[derive(Clone, Debug, serde::Deserialize)]
pub struct EndpointConfig {
    pub host: String,
    pub port: u16,
    pub sni: String,
    pub server_pub: String,  // hex 64 chars
    pub psk: String,         // hex 64 chars
}
```

### Шаг 3: FFI — новая сигнатура transport_start (lib.rs)
```rust
#[no_mangle]
pub unsafe extern "C" fn transport_start(
    socks_port: c_ushort,
    endpoints_json: *const c_char,   // JSON array of EndpointConfig
    key_bytes: *const u8,             // 32 bytes — shared identity key
    rate_limit_bps: u64,
) -> c_int {
    // 1. Null check
    // 2. Parse CStr → &str
    // 3. serde_json::from_str → Vec<EndpointConfig>
    // 4. Validate len >= 1
    // 5. Build FailoverManager
    // 6. Spawn thread + tokio runtime
    // 7. Socks5Proxy::new(bind, failover_mgr, key, rate_limit)
    // 8. rt.block_on(proxy.run())
}
```

### Шаг 4: proxy.rs — интеграция FailoverManager
```rust
pub struct Socks5Proxy {
    bind: SocketAddr,
    failover: Arc<FailoverManager>,  // ЗАМЕНА host:String + port
    key: [u8; 32],
    rate_limiter: Option<RateLimiter>,
}

impl Socks5Proxy {
    pub fn new(
        bind: SocketAddr,
        failover: Arc<FailoverManager>,
        key: [u8; 32],
        rate_limit_bps: u64,
    ) -> Self { ... }

    // На каждое новое соединение:
    async fn handle_client(&self, stream: TcpStream) {
        let endpoint = self.failover.current();  // ← failover
        // connect to endpoint.host:endpoint.port
        // TLS handshake with endpoint.sni
        // steal-TLS auth with server_pub + psk from endpoint
        // if timeout → self.failover.report_failure()
        //   if can_switch() → try_switch()
        //   retry with new endpoint
    }
}
```

### Шаг 5: FailoverManager — расширение
```rust
impl FailoverManager {
    // Существующие: current(), try_switch(), can_switch()

    // НОВОЕ: отчёт о неудаче
    pub fn report_failure(&self) {
        // Увеличить счётчик неудач текущего endpoint
        // Если N подряд timeout → вызвать try_switch()
    }

    // НОВОЕ: текущий endpoint с полным конфигом
    pub fn current_endpoint(&self) -> &EndpointConfig {
        &self.endpoints[*self.current_idx.lock().unwrap()]
    }
}
```

### Шаг 6: Тесты
- Unit: FailoverManager.report_failure → switch after N timeouts
- Integration: Socks5Proxy с mock gateway — timeout → failover
- Fuzz: случайные последовательности timeout/error

---

## 5. ПЛАН — DART FFI BINDINGS (transport_service.dart)

### Шаг 7: Новые typedefs
```dart
typedef _TransportStartNative = Int32 Function(
  Uint16 socksPort,
  Pointer<Utf8> endpointsJson,    // JSON array
  Pointer<Uint8> keyBytes,        // 32 bytes
  Uint64 rateLimitBps,
);
typedef _TransportStartDart = int Function(
  int socksPort,
  Pointer<Utf8> endpointsJson,
  Pointer<Uint8> keyBytes,
  int rateLimitBps,
);
```

### Шаг 8: Новая модель Endpoint (models/endpoint.dart)
```dart
class GatewayEndpoint {
  final String host;
  final int port;
  final String sni;
  final String serverPublic;  // hex 64
  final String psk;            // hex 64
  // toJson, fromJson
}
```

### Шаг 9: transport_service.dart — start()
```dart
Future<void> start() async {
  final endpoints = _settings.endpoints;  // List<GatewayEndpoint>
  final endpointsJson = jsonEncode(endpoints.map((e) => e.toJson()).toList());
  final jsonPtr = endpointsJson.toNativeUtf8();
  final keyBytes = _hexToBytes(_settings.sharedSecret);
  final keyPtr = malloc.allocate<Uint8>(32);
  // ...
  final result = fn(socksPort, jsonPtr, keyPtr, rateLimitBps);
  // ...
}
```

---

## 6. ПЛАН — FLUTTER UI (settings_service.dart + settings_screen.dart)

### Шаг 10: settings_service.dart — multi-endpoint
```dart
class SettingsService {
  List<GatewayEndpoint> endpoints = [];
  String sharedSecret = '';     // общий identity key
  int socksPort = 18080;

  // Для обратной совместимости — getters:
  String get gatewayHost => endpoints.isNotEmpty ? endpoints.first.host : '';
  int get gatewayPort => endpoints.isNotEmpty ? endpoints.first.port : 443;

  bool get isConfigured =>
      endpoints.isNotEmpty && sharedSecret.length == 64 &&
      endpoints.every((e) => e.serverPublic.length == 64 && e.psk.length == 64);
}
```

### Шаг 11: settings_screen.dart — UI для списка endpoints
- Список endpoints (ReorderableListView для приоритета)
- Кнопка "Add endpoint" → форма (host, port, sni, server_pub, psk)
- Свайп для удаления
- Drag для изменения приоритета
- Сохранение в SharedPreferences как JSON

### Шаг 12: Миграция существующих настроек
- При загрузке: если есть старые `gateway_host` + `gateway_port` + `server_public` + `client_psk` → мигрировать в endpoints[0]
- Удалить старые ключи после миграции

---

## 7. ПОРЯДОК ВЫПОЛНЕНИЯ

| # | Задача | Платформа | Effort |
|---|-------|-----------|--------|
| 1 | EndpointConfig + serde в failover.rs | Rust | 0.5 дня |
| 2 | FailoverManager.report_failure + current_endpoint | Rust | 0.5 дня |
| 3 | FFI transport_start — новая сигнатура | Rust | 0.5 дня |
| 4 | proxy.rs — интеграция FailoverManager | Rust | 1 день |
| 5 | Rust unit + integration тесты | Rust | 1 день |
| 6 | Dart FFI typedefs + start() | Flutter | 0.5 дня |
| 7 | models/endpoint.dart | Flutter | 0.5 дня |
| 8 | settings_service.dart — multi-endpoint | Flutter | 0.5 дня |
| 9 | settings_screen.dart — UI списка | Flutter | 1 день |
| 10 | Миграция старых настроек | Flutter | 0.5 дня |
| 11 | Android тест на устройстве | Android | 0.5 дня |
| 12 | CI: обновить build-transport.yml | CI | 0.5 дня |

**Итого: ~7 дней (1.5 недели)**

---

## 8. КРИТЕРИИ ПРИЁМКИ AMO

1. ✅ transport_start принимает JSON array endpoints
2. ✅ FailoverManager интегрирован в Socks5Proxy
3. ✅ При timeout gateway → автоматическое переключение на следующий endpoint
4. ✅ Cooldown 60s между переключениями
5. ✅ Failover только по timeout, не по error
6. ✅ UI: список endpoints с приоритетом (drag reorder)
7. ✅ Миграция старых настроек (single → endpoints[0])
8. ✅ Android тест: добавить второй endpoint, отключить первый → переключение
9. ✅ Rust unit тесты pass
10. ✅ Старый функционал не сломан (single endpoint работает)

---

## 9. ЧТО НЕ МЕНЯЕТСЯ

- ❌ Криптография (X25519, ChaCha20, HKDF)
- ❌ Steal-TLS handshake
- ❌ Protocol (FrameCodec, rekey, replay)
- ❌ Gateway (уже работает)
- ❌ Rate limiter
- ❌ Подписки/тарифы/промокоды

---

## 10. ПОСЛЕ AMO → iOS PORT

AMO реализуется в Rust core → автоматически доступно на iOS:
1. iOS FFI использует тот же `transport_start` контракт
2. iOS Settings UI — тот же Dart код (UiKitView не нужен для настроек)
3. iOS не требует дополнительных изменений для AMO

---

*ТЗ составлено: 2026-07-15*
*Ревизор: claude-sonnet-5, вариант C (Rust core first)*
*Стоимость: $0.012 (2 вызова)*

---

## ПРАВКИ РЕВИЗОРА (добавлены после проверки)

### 1. Failover triggers — расширить
**Было:** Failover только по timeout
**Стало:** Timeout + connection refused + DNS failure + TLS handshake failure (до протокола). НЕ failover on protocol error mid-handshake (DPI fingerprint).

### 2. Cooldown — per-endpoint, не global
**Было:** Min interval 60s (не указано scope)
**Стало:** Cooldown per-endpoint. Плохой endpoint помечается, другие endpoints retry немедленно. Global cooldown блокирует здоровые endpoints — недопустимо.

### 3. FFI schema — явно описать
**endpoints_json JSON схема:**
- Array order = priority (index 0 = primary)
- Required: host (string), port (u16), sni (string), server_pub (hex64), psk (hex64)
- Max endpoints: 16
- Malformed JSON → return -1 (fail-closed)
- Empty array → return -1

### 4. Error propagation FFI → Dart
**Добавить:** callback или status struct для отчёта о failover событиях в Dart.
Например: `transport_get_status() -> *const c_char` (JSON: current_endpoint, last_switch, failures).
