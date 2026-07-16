# ТЕХНИЧЕСКОЕ ЗАДАНИЕ
# Isolated Browser — iOS Port
# Репозиторий: isolated-browser-ios
# Дата: 2026-07-15
# Владелец: Павленко Сергей Анатольевич

---

## 1. ОПИСАНИЕ ПРОДУКТА

Isolated Browser — приватный браузер с собственным транспортным слоем.
Трафик браузера инкапсулируется в TLS 1.3 Application Data записи
(ChaCha20-Poly1305), маскируется под обычное TLS-соединение с CDN
(SNI=cloudflare.com) и направляется на gateway-сервер.

**Это НЕ VPN.** Нет VpnService, нет NetworkExtension, нет TUN/TAP.
Прокси работает in-process на localhost. Только трафик браузера идёт
через туннель. Другие приложения устройства не затронуты.
VPN-иконка в статус-баре НЕ появляется.

---

## 2. ЦЕЛЬ

Создать iOS-версию (iPhone) Isolated Browser на базе существующего
Flutter + Rust кода. Android-версия уже работает и протестирована
на Redmi Note 9 Pro.

---

## 3. АРХИТЕКТУРА (ЦЕЛЕВАЯ ДЛЯ iOS)

```
[Flutter UI]
    ↓
[WKWebView] (нативный iOS браузерный движок)
    ↓
[Локальный HTTP-CONNECT прокси 127.0.0.1:xxxx] (Rust, in-process)
    ↓
[Локальный SOCKS5 прокси 127.0.0.1:18080] (Rust, in-process)
    ↓
[Steal-TLS туннель: TLS 1.3 + ChaCha20-Poly1305 + SNI rotation]
    ↓
[Gateway 38.180.253.219:9443] (развёрнут, работает)
    ↓
[Целевой веб-сервис]
```

**Ключевое отличие от Android:**
- Android: GeckoView + prefs.js (network.proxy.socks)
- iOS: WKWebView + локальный HTTP-CONNECT bridge (WKWebView не поддерживает SOCKS5 напрямую)

**КРИТИЧЕСКОЕ ОГРАНИЧЕНИЕ iOS:**
У WKWebView нет public API для per-webview proxy настройки.
Варианты решения:
1. `URLSessionConfiguration.connectionProxyDictionary` с SOCKS5 (`kCFProxyTypeSOCKS`) — но WKWebView игнорирует кастомный URLSession
2. `kCFNetworkProxiesHTTPProxy` — process-wide proxy (может затронуть другие сетевые запросы приложения)
3. WKURLSchemeHandler — только для кастомных схем, НЕ для https://
4. Локальный HTTP-CONNECT прокси в Rust + `kCFNetworkProxiesHTTPProxy`指向 него


**⚠️ PHASE 0 — ПРЕД-ПРОЕКТНЫЕ ВОРОТА (проверены Ревизором L2, claude-sonnet-5):**

Выполняются ДО начала Phase 1. Ворота Go/No-Go. Каждое — с документированным результатом.
**Все 5 ворот проанализированы архитектором Felix + валидированы Ревизором.**
**OVERALL VERDICT: RISK — CONDITIONAL GO** (порт возможен только при min iOS target = 17+).

---

### РЕЗУЛЬТАТЫ 5 ВОРОТ (зафиксировано 2026-07-16):

**Gate 0a — App Store policy risk assessment — RISK (mitigable):**
- Apple guideline 5.4: "Apps offering VPN services must utilize NEVPNManager API and
  may only be offered by developers enrolled as an organization."
- Scrutiny направлено на **system-wide traffic capture**, не на API.
- Precedent: Opera Mini, Brave, Tenta — in-app proxied browsers БЕЗ NEVPNManager — одобрены.
- App Review непоследователен: иногда флагает слова "tunnel"/"gateway" в UI/бинарных строках.
- Mitigation:
  1. НЕ использовать слова "VPN/tunnel" в UI и App Store copy → "secure browsing/proxy"
  2. НЕ подключать `NEVPNManager`, НЕ запрашивать `Network Extension` entitlement
  3. Scope прокси строго in-app (WKWebView only) — структурная защита
  4. Подготовить демо-аккаунт + описание архитектуры для Review Notes
- **Вердикт: RISK, mitigable. GO при дисциплине терминологии.**

**Gate 0b — WKWebView proxy mechanism — RISK/NO-GO без iOS 17+:**
- iOS 17+ представляет `WKWebsiteDataStore.proxyConfigurations`
  (NWEndpoint/NEProxyConfiguration-based) — **настоящий per-instance proxy API**.
  Это правильный современный механизм, НЕ варианты 1/2/3 ниже.
- До iOS 17: механизм 1 (kCFNetworkProxiesHTTPProxy process-wide) —
  работает НЕНАДЁЖНО: WebSocket, некоторые XHR/redirects/CORS preflight
  обходят process-wide proxy (WKWebView использует свой network process).
- NSURLProtocol НЕ перехватывает main-frame loads WKWebView (подтверждённый gap,
  SO 41068675 / SO 43100358: "WKWebView runs web connectivity in separate process,
  there is no possibility to override anything").
- NSURLSessionConfiguration.connectionProxyDictionary НЕ влияет на WKWebView.
- WKURLSchemeHandler — только custom schemes, НЕ https://.

**⚠️ ИНТЕРНЕТ-ИССЛЕДОВАНИЕ (2026-07-16, Felix + владелец):**
- API подтверждено: SO 77933064 (Feb 2024) — реальный код iOS 17.
- Apple Developer Documentation: `WKWebsiteDataStore.proxyConfigurations` (iOS 17+),
  `ProxyConfiguration` (Network framework).
- WebKit.org: "WebKit Features in Safari 17" — официально декларируется поддержка
  `ProxyConfiguration` для изоляции трафика конкретных экземпляров WKWebView.

**✅ ПОДТВЕРЖДЕНО ВЛАДЕЛЬЦЕМ (2026-07-16) — подробный отчёт:**
- iOS 17+, localhost (127.0.0.1), без аутентификации → работает "из коробки".
- Покрытие классов трафика:
  - Main Frame / Subresources (HTML, JS, CSS, images): **100% перехват**
  - XHR / fetch / FormData: **100% перехват** (нет проблем с CORS/POST как у URLSchemeHandler)
  - HTTP/2: **перехват** (WebKit CONNECT к прокси, ALPN h2 внутри туннеля)
  - HTTP/3 (QUIC/UDP): может откатываться к HTTP/2 over TCP (httpCONNECTProxy — TCP-only)
  - WebSockets (ws/wss): **перехват** (CONNECT-туннель, затем Upgrade handshake)
- Прямых утечек мимо прокси НЕТ при корректной настройке.

**⚠️ ДВЕ КРИТИЧЕСКИЕ ЛОВУШКИ (документировать в Phase 3):**

1. **Connection Pooling (keep-alive trap):**
   Если применить `proxyConfigurations` на УЖЕ АКТИВНЫЙ WKWebsiteDataStore,
   существующие TCP-соединения (keep-alive) продолжат идти напрямую.
   > РЕШЕНИЕ: настраивать `proxyConfigurations` СТРОГО ДО первого запроса WKWebView,
   > либо использовать non-persistent (incognito) data store.

2. **Флаг `allowFailover` (КРИТИЧНО для безопасности):**
   По умолчанию может быть `true`. Если локальный прокси упадёт — WebKit
   **пустит трафик напрямую**, чтобы страница не "умерла". Это УТЕЧКА.
   > РЕШЕНИЕ: явно `proxyConfig.allowFailover = false`.
   > При падении прокси WebKit вернёт ошибку навигации, заблокировав утечку.

**ЭТАЛОННАЯ КОНФИГУРАЦИЯ (Phase 3):**
```swift
import WebKit
import Network

let proxyEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .init(integerLiteral: 8080))
let proxyConfig = ProxyConfiguration(httpCONNECTProxy: proxyEndpoint)
proxyConfig.allowFailover = false  // КРИТИЧНО: запретить утечку при сбое

let store = WKWebsiteDataStore.default()  // или nonPersistent() для инкогнито
store.proxyConfigurations = [proxyConfig]  // СТРОГО ДО первого запроса

let webConfig = WKWebViewConfiguration()
webConfig.websiteDataStore = store
let webView = WKWebView(frame: .zero, configuration: webConfig)
```

**ТРЕБОВАНИЯ К ЛОКАЛЬНОМУ ПРОКСИ (Rust, Phase 1-2):**
- Должен обрабатывать HTTP-метод **CONNECT** (HTTP/1.1 CONNECT host:port).
- Режим "слепого" туннеля: установить TCP к целевому хосту (через SOCKS5→Steal-TLS)
  и пересылать сырые байты. **БЕЗ MITM** — не расшифровывать TLS.
- Без MITM → нет проблем с сертификатами, ATS, CA installation.

**РЕШЕНИЕ: min iOS target = 17.0. Использовать `WKWebsiteDataStore.proxyConfigurations`.**
**Вердикт: GO при min iOS 17+ (подтверждено владельцем + интернет-исследованием).**
- P0 Spike (на Mac) остаётся для эмпирической верификации на конкретном устройстве,
  но теоретический риск теперь НИЗКИЙ.
- Критерий PASS Spike: ноль утечек (verified via Wireshark/tcpdump — visible только
  localhost + tunnel к gateway). Проверить `allowFailover=false` при падении прокси.

**Gate 0c — transport-core iOS-portability — GO (с landmines):**
- Прочитаны lib.rs, failover.rs, proxy.rs, Cargo.toml.
- Pure std + tokio + rustls + chacha20poly1305 + x25519-dalek. NO Android, NO JNI, NO VpnService.
- Landmines (зафиксировать в Phase 1):
  1. `staticlib` НЕ достаточно → нужен `.xcframework` (arm64 device + arm64/x86_64 sim slices),
     через `cargo-lipo`/`cargo-xcodebuild` или manual `lipo`.
  2. tokio/mio: default threaded backend → kqueue-based на iOS (auto). Pin-check `mio` version.
  3. `getrandom` на iOS: проверить backend. Если default syscall не работает →
     `SecRandomCopyBytes` fallback через `rand` crate iOS support.
  4. Background execution: iOS suspense app → tokio threads paused/killed.
     НЕ предполагать persistent socket в background (no Network Extension = no bg privilege).
- **Вердикт: GO, fully portable. Build/runtime hardening в Phase 1.**

**Gate 0d — ATS compatibility — GO:**
- Мы НЕ делаем MITM TLS WKWebView. Трафик туннелируется через SOCKS5/CONNECT,
  end-to-end TLS к реальному таргету сохраняется.
- WKWebView подключается к localhost прокси — loopback exempt от ATS
  (NSAllowsLocalNetworking, или ATS не применяется к 127.0.0.1 по умолчанию).
- Реальный TLS таргета удовлетворяет ATS на "real leg".
- Rust Steal-TLS работает НИЖЕ NSURLSession/CFNetwork (raw socket via tokio) →
  ATS никогда не инспектирует его.
- No MITM cert chain = no ATS violation, no exception justification file
  (только localhost exemption).
- **Вердикт: GO. Reasoning корректен.**

**Gate 0e — Fallback path — GO (kill is correct):**
- Если `proxyConfigurations` (iOS 17+) НЕ работает эмпирически:
  - (A) Kill iOS port — **ПРАВИЛЬНО** (только если реальный fix 17+ недоступен)
  - (B) NEVPNManager — REJECT. Противоречит "NOT VPN" identity. Poison pill.
  - (C) WKURLSchemeHandler URL rewriting — REJECT. Ломает relative URLs/service workers/WebSocket.
  - (D) SFSafariViewController — REJECT. Нет proxy control вообще.
- Reframe: это НЕ "fallback list" — это "ship iOS 17+ using native proxy API" ИЛИ "kill iOS port".
- **Вердикт: GO. Kill = correct fallback logic.**

---

**ОБЛАСТЬ ДЕЙСТВИЯ (SCOPE):**
Прокси работает ТОЛЬКО для трафика внутри приложения (WKWebView).
Системный трафик, Safari, другие приложения НЕ затронуты.
Никакого per-app VPN configuration, никаких NetworkExtension профилей.

**MINIMUM iOS TARGET: 17.0** (из-за WKWebsiteDataStore.proxyConfigurations, Gate 0b).

**APP STORE REVIEW RISK:**
- Positioning: "private browser with secure transport layer"
- НЕ упоминать: VPN, tunnel, gateway, proxy interception, TLS stripping, censorship bypass
- Privacy Policy: proxy только для браузерного трафика
- No NEVPNManager, no Network Extension entitlement
- Демо-аккаунт + архитектурное описание в Review Notes

**АРХИТЕКТУРНЫЙ ПОДХОД (обновлено Gate 0b):**
iOS 17+: `WKWebsiteDataStore.proxyConfigurations` → local HTTP-CONNECT bridge (Rust, 127.0.0.1:xxxx)
→ SOCKS5 (127.0.0.1:18080) → Steal-TLS tunnel → gateway.
Приложение работает только в foreground — продуктовое ограничение (Gate 0c, landmine 4).

---

## 4. ИСХОДНЫЙ КОД (РЕПОЗИТОРИЙ-ИСТОЧНИК)

**GitHub:** https://github.com/r9549684-dev/isolated-browser
**Ветка:** `wip-flutter`
**Локальный путь:** `D:\Felix\projects\isolated-browser\`

### Структура проекта:
```
isolated-browser/
├── app/                          # Flutter-приложение
│   ├── lib/                      # Dart-код (17 файлов)
│   │   ├── main.dart             # Entry point, MultiProvider
│   │   ├── models/
│   │   │   ├── subscription.dart # SubscriptionTier enum, Subscription class
│   │   │   └── web_service.dart  # WebService class, kBuiltinServices (12 сервисов)
│   │   ├── screens/              # 8 экранов
│   │   │   ├── home_screen.dart
│   │   │   ├── browser_screen.dart    # AndroidView → нужен UiKitView
│   │   │   ├── settings_screen.dart
│   │   │   ├── subscription_screen.dart
│   │   │   ├── select_services_screen.dart
│   │   │   ├── service_selection_screen.dart
│   │   │   ├── owner_panel_screen.dart
│   │   │   └── payment_simulation_screen.dart
│   │   ├── services/             # 5 сервисов
│   │   │   ├── transport_service.dart  # FFI bindings (уже поддерживает iOS)
│   │   │   ├── settings_service.dart
│   │   │   ├── subscription_service.dart
│   │   │   ├── catalog_service.dart
│   │   │   └── promo_service.dart
│   │   └── widgets/
│   │       ├── service_grid.dart
│   │       └── transport_status_bar.dart
│   ├── android/                  # Android-специфика (GeckoView)
│   │   └── app/src/main/
│   │       ├── kotlin/com/isolatedbrowser/app/MainActivity.kt
│   │       └── java/com/isolatedbrowser/gecko/GeckoViewPlugin.kt
│   ├── pubspec.yaml              # Зависимости Flutter
│   └── assets/icons/             # Пусто (Material Icons)
├── transport-core/               # Rust: транспортный слой (FFI)
│   ├── Cargo.toml
│   ├── src/                      # 11 модулей
│   ├── tests/
│   ├── fuzz/
│   └── .cargo/config.toml        # Только aarch64-linux-android
├── gateway/                      # Rust: gateway-сервер (уже развёрнут)
│   ├── Cargo.toml
│   └── src/                      # main.rs, auth.rs, promo.rs, metrics.rs
├── .env                          # Ключи gateway
├── docs/THREAT_MODEL.md          # Модель угроз
├── adr/001-stack-selection.md    # Architecture Decision Record
└── .github/workflows/build-transport.yml  # CI
```

---

## 5. RUST TRANSPORT-CORE — FFI КОНТРАКТ

### 5.1. Функции (точные сигнатуры)

```rust
// Запуск SOCKS5 прокси на отдельном потоке + tokio runtime.
// Возвращает 0 при успехе, -1 при ошибке.
#[no_mangle]
pub unsafe extern "C" fn transport_start(
    socks_port: u16,           // локальный порт SOCKS5 (18080)
    gateway_host: *const c_char,  // C-string: "38.180.253.219"
    gateway_port: u16,         // 9443
    key_bytes: *const u8,      // 32 байта: shared_secret (legacy, PFS заменяет)
    sni_list: *const c_char,   // C-string: "cloudflare.com,google.com"
    server_pub: *const u8,     // 32 байта: X25519 server public key
    psk_bytes: *const u8,      // 32 байта: client pre-shared key
    rate_limit_bps: u64,       // байт/сек (0 = без лимита)
) -> i32

// Версия. Статическая память, не освобождать.
#[no_mangle]
pub extern "C" fn transport_version() -> *const c_char  // "0.1.0\0"

// Остановка. TODO: сейчас stub (возвращает 0).
// НУЖНО реализовать для iOS: корректный shutdown tokio runtime.
#[no_mangle]
pub extern "C" fn transport_stop() -> i32
```

### 5.2. Загрузка библиотеки на iOS

```dart
// Уже реализовано в transport_service.dart:
if (Platform.isIOS || Platform.isMacOS) {
    return DynamicLibrary.process();  // статически слинкована в бинар
}
```

На iOS Rust компилируется как **staticlib** и линкуется в основной бинар
приложения. `DynamicLibrary.process()` ищет символы в главном исполняемом файле.

### 5.3. Зависимости Cargo (все поддерживают iOS)

```toml
tokio = { version = "1", features = ["full"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
tokio-rustls = { version = "0.26", default-features = false, features = ["ring"] }
webpki-roots = "0.26"
rand = "0.8"
chacha20poly1305 = "0.10"
x25519-dalek = { version = "2", features = ["static_secrets"] }
hkdf = "0.12"
sha2 = "0.10"
subtle = "2"
bytes = "1"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

`ring` (crypto backend) поддерживает `aarch64-apple-ios` нативно.

### 5.4. Кросс-компиляция (нужно добавить)

Текущий `.cargo/config.toml` содержит только Android target. Для iOS нужно:

```toml
# Добавить в [target] секцию:
[target.aarch64-apple-ios]
linker = "rust-lld"  # или clang из Xcode

[target.aarch64-apple-ios-sim]
linker = "rust-lld"

# Опционально для Intel-симулятора:
[target.x86_64-apple-ios]
linker = "rust-lld"
```

Команды сборки (на macOS):
```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

# Device:
cargo build --release --target aarch64-apple-ios
# Simulator (Apple Silicon):
cargo build --release --target aarch64-apple-ios-sim
# Simulator (Intel):
cargo build --release --target x86_64-apple-ios

# Lipo в универсальную библиотеку:
lipo -create \
    target/aarch64-apple-ios/release/libtransport_core.a \
    target/aarch64-apple-ios-sim/release/libtransport_core.a \
    -output libtransport_core_universal.a
```

**Cargo.toml изменение:** добавить `staticlib` в `crate-type`:
```toml
[lib]
name = "transport_core"
crate-type = ["cdylib", "rlib", "staticlib"]
```

### 5.5. Интеграция с Xcode / Flutter iOS

1. Создать Podspec для `transport-core`:
   - `vendored_libraries = ["libtransport_core_universal.a"]`
   - `preserve_paths = ["libtransport_core_universal.a"]`
2. Или: добавить `.a` напрямую в Xcode → "Link Binary with Libraries"
3. Bridging header не нужен (FFI через `DynamicLibrary.process()` в Dart)
4. Символы `transport_start`, `transport_version`, `transport_stop`
   не должны быть stripped (проверить Build Settings → Strip Style)

---

## 6. WKWEBVIEW — НИЖНИЙ УРОВЕНЬ БРАУЗЕРА

### 6.1. Регистрация PlatformView

**AppDelegate.swift** (зеркало MainActivity.kt):
```swift
import Flutter

@UIApplicationMain
class AppDelegate: FlutterAppDelegate {
    override func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: ...
    ) -> Bool {
        let controller = window?.rootViewController as! FlutterViewController
        controller.engine.platformViewsController.registry.register(
            WKWebViewPluginFactory(messenger: controller.binaryMessenger),
            withId: "com.isolatedbrowser/wkwebview"
        )
        return super.application(application, didFinishLaunchingWithOptions: launchOptions)
    }
}
```

### 6.2. WKWebViewPluginFactory (Swift)

Создать нативный plugin (Swift), который:
1. Принимает `creationParams`: `{url, socks_proxy_host, socks_proxy_port}`
2. Создаёт `WKWebView` с конфигурацией
3. Настраивает прокси (см. §6.3)
4. Загружает URL
5. Отправляет колбэки `onPageStarted`/`onPageFinished` через MethodChannel
6. Обрабатывает `reload`/`goBack`/`loadUrl` из Dart

### 6.3. Настройка прокси для WKWebView

**Проблема:** WKWebView не имеет per-instance proxy API.

**Рекомендуемое решение:**

1. Rust стартует HTTP-CONNECT прокси на `127.0.0.1:xxxx` (новый модуль в transport-core)
2. HTTP-CONNECT прокси принимает `CONNECT host:443` и перенаправляет в SOCKS5 на `127.0.0.1:18080`
3. Настройка process-wide proxy перед созданием WKWebView:
```swift
let proxyConfig: [AnyHashable: Any] = [
    kCFNetworkProxiesHTTPProxy: "127.0.0.1",
    kCFNetworkProxiesHTTPPort: xxxx,
    kCFNetworkProxiesHTTPSEnable: true,
    kCFNetworkProxiesHTTPEnable: true,
]
CFNetworkExecuteProxyAutoConfigurationTest(
    proxyConfig as CFDictionary,
    ...
)
// Или через URLSessionConfiguration:
let config = URLSessionConfiguration.ephemeral
config.connectionProxyDictionary = proxyConfig
```

4. Альтернатива: `URLSessionConfiguration.connectionProxyDictionary` с SOCKS5 напрямую:
```swift
let config = URLSessionConfiguration.ephemeral
config.connectionProxyDictionary = [
    kCFProxyTypeSOCKS: "127.0.0.1",
    kCFProxyPortKey: 18080,
]
```
Но WKWebView может игнорировать кастомный URLSession — проверить на устройстве.

### 6.4. Flutter-сторона (browser_screen.dart)

```dart
// Заменить AndroidView на платформо-зависимый виджет:
if (Platform.isAndroid) {
    return AndroidView(
        viewType: 'com.isolatedbrowser/geckoview',
        creationParams: {...},
        creationParamsCodec: StandardMessageCodec(),
    );
} else if (Platform.isIOS) {
    return UiKitView(
        viewType: 'com.isolatedbrowser/wkwebview',
        creationParams: {
            'url': initialUrl,
            'socks_proxy_host': '127.0.0.1',
            'socks_proxy_port': socksProxyPort,
        },
        creationParamsCodec: StandardMessageCodec(),
    );
}
```

---

## 7. FLUTTER — ВЕРХНИЙ УРОВЕНЬ

### 7.1. pubspec.yaml (без изменений)

Все зависимости уже кросс-платформенные:
- `provider: ^6.1.2` — state management
- `ffi: ^2.1.3` — dart:ffi
- `shared_preferences: ^2.3.2` — UserDefaults на iOS
- `flutter_secure_storage: ^9.2.2` — Keychain на iOS
- `flutter_svg: ^2.0.10+1` — SVG
- `logging: ^1.3.0`

### 7.2. Создание iOS-проекта

```bash
cd app/
flutter create --platforms=ios .
```

Это создаст `app/ios/` директорию со стандартным Flutter iOS проектом.
Затем настроить:
1. Bundle identifier: `com.isolatedbrowser.app`
2. Deployment target: iOS 14.0+ (для WKWebView features)
3. Добавить `libtransport_core_universal.a` в Xcode
4. Заменить `AppDelegate.swift` на версию с регистрацией PlatformView
5. Добавить `NSAppTransportSecurity` в Info.plist для localhost:
```xml
<key>NSAppTransportSecurity</key>
<dict>
    <key>NSExceptionDomains</key>
    <dict>
        <key>127.0.0.1</key>
        <dict>
            <key>NSExceptionAllowsInsecureHTTPLoads</key>
            <true/>
        </dict>
    </dict>
</dict>
```

### 7.3. transport_service.dart — БЕЗ ИЗМЕНЕНИЙ

Код уже поддерживает iOS:
```dart
DynamicLibrary _loadLib() {
    if (Platform.isIOS || Platform.isMacOS) {
        return DynamicLibrary.process();  // уже реализовано
    }
    ...
}
```

### 7.4. settings_service.dart — БЕЗ ИЗМЕНЕНИЙ

SharedPreferences → NSUserDefaults автоматически.

### 7.5. flutter_secure_storage — БЕЗ ИЗМЕНЕНИЙ

Уже настроено:
```dart
IOSOptions(accessibility: KeychainAccessibility.first_unlock)
```

**Рекомендация:** перенести `sharedSecret`, `serverPublic`, `clientPsk`
из SharedPreferences в flutter_secure_storage (Keychain).
Сейчас они хранятся plaintext в SharedPreferences.

---

## 8. GATEWAY (БЕЗ ИЗМЕНЕНИЙ)

Gateway уже развёрнут и работает. iOS-клиент подключается к нему.

```
Хост: 38.180.253.219
Порт: 9443
SNI: cloudflare.com,google.com (rotation)
Fallback CDN: cloudflare.com:443
Промокоды: PRO3M, PRO6M, PRO12M
Тарифы: Trial (3 дня), Base (150₽), Pro (250₽), Premium (350₽)
```

### Ключи (из .env, передаются через Settings Screen):
```
GATEWAY_HOST=38.180.253.219
GATEWAY_PORT=9443
SHARED_SECRET=6cafcbd72f18f83be37fbd44b8d340cbbc0809ce4b974482dd2908f554f0cbb5
SERVER_PUBLIC_KEY=ddc2127c2fb7d7e0222073da0a7390ff594ecc769b664b706631f88c7e76d80a
SNI_LIST=cloudflare.com,google.com
SOCKS_PORT=18080
```

---

## 9. КРИПТОГРАФИЯ (БЕЗ ИЗМЕНЕНИЙ, ВСЁ В RUST)

| Назначение | Алгоритм | Параметры |
|-----------|----------|-----------|
| Key exchange | X25519 | ECDHE ephemeral per-session (PFS) |
| Auth token | HKDF-SHA256 | info=`"isolated-browser-auth"` |
| Session key | HKDF-SHA256 | info=`"isolated-browser-session"` |
| Шифрование | ChaCha20-Poly1305 | AEAD, 256-bit |
| Nonce | 12 bytes | prefix(8) ‖ counter(4) |
| Replay window | Sliding | 64 frames |
| Rekey threshold | 0xF0000000 | ~4B frames |
| Rekey overlap | 256 frames | dual-validation |

Всё реализовано в `transport-core` (Rust). На iOS используется `ring` backend
(нативный arm64 asm). Платформо-специфичный код НЕ требуется.

---

## 10. ДЕЛОНИЕ ЗАДАЧИ

### Phase 1: Rust кросс-компиляция (1 неделя)

1.1. Добавить `staticlib` в `crate-type` в Cargo.toml
1.2. Добавить iOS targets в `.cargo/config.toml`
1.3. `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`
1.4. Скомпилировать staticlib для device + simulator
1.5. Создать universal binary через `lipo`
1.6. Реализовать `transport_stop()` (корректный shutdown tokio runtime)
1.7. Добавить HTTP-CONNECT bridge модуль в transport-core:
     - Слушает `127.0.0.1:xxxx` (HTTP CONNECT method)
     - Перенаправляет в SOCKS5 на `127.0.0.1:18080`
     - FFI: расширить `transport_start` или добавить `transport_start_http_bridge()`
1.8. Unit-тесты для HTTP-CONNECT bridge

### Phase 2: Flutter iOS-проект (1 неделя)

2.1. `flutter create --platforms=ios .` в `app/`
2.2. Настроить Bundle ID: `com.isolatedbrowser.app`
2.3. Deployment target: iOS 14.0+
2.4. Интегрировать `libtransport_core_universal.a` в Xcode
2.5. Создать Podspec (если нужно для автоматизации)
2.6. Настроить Info.plist (NSAppTransportSecurity для localhost)
2.7. Проверить, что FFI-символы не stripped

### Phase 3: WKWebView нативный plugin (1.5 недели)

3.1. Создать `WKWebViewPluginFactory.swift` (FlutterPlatformViewFactory)
3.2. Создать `WKWebViewWrapper.swift` (FlutterPlatformView)
3.3. Настроить WKWebView с прокси (см. §6.3)
3.4. MethodChannel: `onPageStarted`, `onPageFinished`, `reload`, `goBack`, `loadUrl`
3.5. Зарегистрировать в `AppDelegate.swift`
3.6. Обновить `browser_screen.dart`: `UiKitView` для iOS
3.7. Тест на симуляторе
3.08. Тест на реальном iPhone

### Phase 4: Интеграция и тестирование (1 неделя)

4.1. Протестировать транспорт (SOCKS5 + HTTP bridge) на iOS
4.2. Протестировать WKWebView через прокси
4.3. Протестировать steal-TLS туннель до gateway
4.4. Проверить, что НЕТ VPN-иконки в статус-баре
4.5. Проверить, что другие приложения НЕ затронуты
4.6. Тест подписок/промокодов/тарифов
4.7. Тест rate limiting
4.8. Memory leak test (transport_start/stop циклы)
4.9. Тест на разных iOS версиях (14, 15, 16, 17)

### Phase 5: App Store подготовка (0.5 недели)

5.1. Иконки и метаданные
5.2. App Store Review Guidelines проверка
5.3. Убедиться: НЕ используется NetworkExtension, НЕ используется VPN API
5.4. Privacy Policy (proxy только для браузерного трафика)
5.5. TestFlight бета-тестирование

---

## 11. ОГРАНИЧЕНИЯ И РИСКИ

1. **WKWebView proxy API** — главная техническая сложность.
   WKWebView не имеет per-instance proxy настройки.
   Решение: process-wide proxy + HTTP-CONNECT bridge.
   Риск: может потребоваться private API (недопустимо для App Store).

2. **Foreground only** — продуктовое ограничение.
   Туннель работает только пока приложение активно.
   При сворачивании — SOCKS5 прокси останавливается.

3. **macOS required** — сборка iOS только на macOS (Xcode).
   На Windows собрать нельзя.

4. **Apple Developer Account** — $99/год для публикации в App Store.
   НЕ нужны entitlements для VPN/NetworkExtension (не используется).

5. **Cert pinning** — взаимодействие steal-TLS с TLS-стеком WKWebView
   требует отдельной проверки.

6. **`transport_stop()` stub** — сейчас возвращает 0 без реального shutdown.
   Нужно реализовать до production.

---

## 12. ЧТО НЕ НУЖНО ДЕЛАТЬ

- ❌ НЕ использовать NetworkExtension / NEAppProxyProvider / NETunnelProvider
- ❌ НЕ использовать VpnService (это iOS, не Android, но принцип тот же)
- ❌ НЕ создавать VPN-профиль или VPN-configuration
- ❌ НЕ пытаться портить GeckoView на iOS (он Android-only)
- ❌ НЕ менять gateway (уже работает)
- ❌ НЕ менять криптографию (уже в Rust, кросс-платформенная)
- ❌ НЕ менять Flutter UI (Dart-код кросс-платформенный)
- ❌ НЕ использовать private API (приведёт к отказу App Store)

---

## 13. КРИТЕРИИ ПРИЁМКИ

1. ✅ Приложение запускается на iPhone (iOS 14+)
2. ✅ Транспорт стартует (transport_start возвращает 0)
3. ✅ SOCKS5 прокси слушает 127.0.0.1:18080
4. ✅ HTTP-CONNECT bridge слушает 127.0.0.1:xxxx
5. ✅ WKWebView загружает страницу через прокси
6. ✅ Трафик идёт через steal-TLS туннель к gateway
7. ✅ НЕТ VPN-иконки в статус-баре
8. ✅ Другие приложения устройства работают нормально
9. ✅ Подписки, промокоды, тарифы работают
10. ✅ Rate limiting работает
11. ✅ Transport start/stop не течёт памятью
12. ✅ App Store Review проходит без замечаний

---

## 14. ССЫЛКИ

- Исходный репозиторий: https://github.com/r9549684-dev/isolated-browser
- Ветка: wip-flutter
- Модель угроз: docs/THREAT_MODEL.md
- Architecture Decision Record: adr/001-stack-selection.md
- Журнал разработки: JOURNAL.md
- Gateway: 38.180.253.219:9443 (2 core, 4GB RAM, развёрнут)

---

*ТЗ составлено: 2026-07-15*
*Автор: Felix-Architect для Павленко Сергея Анатольевича*
