# IMPLEMENTATION_PLAN.md

> **Назначение документа.** Поэтапный план реализации двух ТЗ — **AMO** (multi-gateway failover) и **iOS Port**.
> Документ ориентирован на **поэтапную проверку ревизором**: каждый этап атомарен, имеет чёткий deliverable, критерии приёмки и то, что можно проверить независимо.
> **Порядок работ:** сначала AMO целиком в Rust core («Rust core first»), затем iOS-порт, наследующий готовый FFI-контракт.

---

## 0. Как читать этот план

- **Deliverable** — что должно появиться в коде по итогу этапа.
- **Проверка** — что ревизор проверяет для приёмки этапа.
- **Изолированно проверяемо** — можно ли принять этап без остального проекта (тесты без сети/железа).
- **Ворота (Gate)** — блокирующие контрольные точки; следующий блок не начинается, пока ворота не пройдены.

| Ворота | После этапа | Что блокирует |
|---|---|---|
| **G-Contract** | A4 | FFI-сигнатура заморожена; iOS зависит от неё |
| **G-AMO** | A8 | Нельзя начинать iOS, пока failover не работает на Android |
| **G-Spike** | B0 | Go/No-Go всего iOS-порта (провал → kill iOS port) |
| **G-iOS** | B6 | Готовность к App Store |

### SCOPE G-Contract (уточнение Ревизора L2)
**Замораживается** ядро: `transport_start` (multi-endpoint), `transport_get_status`, `transport_version`, `transport_stop`.
**Разрешены** post-freeze аддитивные платформо-специфичные функции (напр. `transport_start_http_bridge()` в B2) — при условии, что они НЕ меняют замороженные сигнатуры и НЕ требуются Android.

### ПАРАЛЛЕЛЬНЫЙ WORKSTREAM: Gate 0a App Store policy risk (правка Ревизора L2)
B0 (техническая выполнимость) и Gate 0a (коммерческая жизнеспособность) — **независимые риски**.
Последовательное выполнение = можно пройти B0, потратить B1–B6, и умереть в B7 по policy-причинам.
**Решение:** параллельно с B0 запустить дешёвый policy-risk pass:
- TestFlight-сабмит со stub-proxy extension (сэмплировать реакцию Review);
- письменный анализ precedent (исходы App Review подобных приложений).
Трудозатраты низкие, не блокирует timeline B0, de-risk на ранней стадии.

---

## БЛОК A — AMO (Rust core → Dart → UI)

### A1. EndpointConfig + serde
- **Deliverable:** структура `EndpointConfig` (host, port, sni, server_pub hex64, psk hex64) + `serde::Deserialize`; парсер JSON-массива с валидацией (len 1–16, hex-длины, malformed → ошибка).
- **Проверка:** unit-тесты парсинга — валидный массив, пустой (`[]`→err), 17 элементов (→err), битый hex, malformed JSON. Старые тесты 7/7 не сломаны, компиляция зелёная.
- **Изолированно проверяемо:** да (чистая логика без сети).

### A2. FailoverManager: report_failure + current_endpoint + per-endpoint cooldown
- **Deliverable:** расширение готового `FailoverManager` — счётчик неудач на endpoint, `try_switch()` после N подряд timeout, per-endpoint cooldown 60s, `current_endpoint() -> &EndpointConfig`.
- **Проверка:** unit-тесты — switch после N timeouts; per-endpoint cooldown (плохой помечен, здоровые retry немедленно); global cooldown НЕ блокирует; min interval 60s.
- **Изолированно проверяемо:** да (детерминированные тесты на состоянии).

### A3. proxy.rs — интеграция FailoverManager
- **Deliverable:** `Socks5Proxy` использует `Arc<FailoverManager>` вместо одиночного `host+port`; на каждое соединение `current()`, при timeout/refused/DNS-fail/TLS-handshake-fail → `report_failure()` + возможный `try_switch()` + retry. **НЕ** failover на protocol error mid-handshake (риск DPI-фингерпринта).
- **Проверка:** integration-тест с mock-gateway — timeout на primary → переключение на secondary; protocol error mid-handshake → НЕ переключается.
- **Изолированно проверяемо:** да (через mock, без реального gateway).

### A4. FFI transport_start — новая сигнатура + status  ⚠️ ВОРОТА G-Contract
- **Deliverable:** `transport_start(socks_port, endpoints_json, key32, rate_limit)`; null-checks; parse→validate→FailoverManager→spawn. Malformed/empty → `-1` (fail-closed). Плюс `transport_get_status() -> *const c_char` (JSON: current_endpoint, last_switch, failures).
- **Проверка:** FFI-тесты на уровне C-контракта — валидный JSON→0, битый→-1, empty→-1; status возвращает валидный JSON.
- **Изолированно проверяемо:** да.
- **⚠️ После A4 сигнатура заморожена** — iOS от неё зависит, менять нельзя.

### A5. Dart FFI bindings + модель Endpoint
- **Deliverable:** `models/endpoint.dart` (`GatewayEndpoint` + toJson/fromJson); новые typedefs в `transport_service.dart`; `start()` сериализует `endpoints` в JSON, готовит key32.
- **Проверка:** Dart unit-тесты сериализации; smoke-тест вызова FFI на Android (start возвращает 0).
- **Изолированно проверяемо:** да (Dart-тесты) + Android smoke.

### A6. settings_service.dart — multi-endpoint + миграция
- **Deliverable:** `List<GatewayEndpoint> endpoints`, `sharedSecret`, `isConfigured`; legacy-getters `gatewayHost/gatewayPort` (=endpoints.first); миграция старых ключей `gateway_host/port/server_public/client_psk` → `endpoints[0]`, удаление старых ключей.
- **Проверка:** unit-тест миграции (старый формат → endpoints[0], старые ключи удалены); `isConfigured` валидирует длины.
- **Изолированно проверяемо:** да.

### A7. settings_screen.dart — UI списка endpoints
- **Deliverable:** ReorderableListView (приоритет = порядок), «Add endpoint» форма, swipe-удаление, сохранение в SharedPreferences как JSON.
- **Проверка:** widget-тесты (добавить/удалить/reorder → persist); визуальная проверка.
- **Изолированно проверяемо:** да (widget-тесты).

### A8. Android on-device тест + CI  ⚠️ ВОРОТА G-AMO
- **Deliverable:** обновлённый `build-transport.yml`; ручной тест на Redmi Note 9 Pro.
- **Проверка (главный критерий приёмки AMO):** добавить 2 endpoint, отключить первый → трафик автоматически переключается; single-endpoint (legacy) по-прежнему работает; CI зелёный.
- **Проверяемо:** end-to-end, «живая» демонстрация failover.
- **→ Ворота G-AMO:** все 10 критериев приёмки AMO_SPEC §8. Только после прохождения — переход к iOS.

---

## БЛОК B — iOS PORT

### B0. Phase 0 Spike (на Mac)  ⚠️ ВОРОТА G-Spike (Go/No-Go)
- **Deliverable:** минимальный Swift-прототип `WKWebsiteDataStore.proxyConfigurations` (iOS 17+) + локальный HTTP-CONNECT stub, проверка через Wireshark/tcpdump.
- **Проверка:** **ноль утечек** (виден только localhost + туннель к gateway); `allowFailover=false` при падении прокси → навигация падает, а НЕ уходит напрямую; проверить keep-alive trap (конфиг ДО первого запроса).
- **⚠️ HTTP/3/QUIC leak test (правка Ревизора L2, КРИТИЧНО):** QUIC = UDP, `proxyConfigurations` = TCP-only. Если браузерный стек попытается HTTP/3 и OS не направит UDP-поток в туннель — он уйдёт напрямую (bypass прокси = утечка, нарушает "zero leaks"). Тест-кейс: запрос к origin с поддержкой QUIC → убедиться, что нет UDP-egress мимо прокси (Wireshark). При необходимости — отключить HTTP/3/Alt-Svc в networking-стеке ИЛИ блокировать outbound UDP (кроме loopback-to-SOCKS5).
- **Критично:** провал → kill iOS port (Gate 0e). Проверяется однозначно инструментом.

### B1. Rust кросс-компиляция под iOS
- **Deliverable:** `staticlib` в crate-type; iOS targets в `.cargo/config.toml`; сборка device+sim; `lipo`/`.xcframework`; реализация `transport_stop()` (реальный shutdown tokio); проверка `getrandom`/`mio` backend на iOS.
- **Проверка:** `.a`/`.xcframework` собирается; символы `transport_start/version/stop` не stripped; unit-тесты Rust проходят на arm64.
- **Изолированно проверяемо:** да (сборка + `nm` на символы).

### B2. HTTP-CONNECT bridge в transport-core
- **Deliverable:** новый модуль — слушает `127.0.0.1:xxxx`, принимает HTTP/1.1 `CONNECT host:port`, «слепой» туннель через SOCKS5→Steal-TLS, **без MITM**; FFI `transport_start_http_bridge()` (или расширение).
- **⚠️ UDP-bypass enforcement (правка Ревизора L2):** QUIC/HTTP3 (UDP) может обходить TCP-only прокси. Bridge/конфиг должны либо отключать HTTP/3/Alt-Svc в networking-стеке, либо блокировать outbound UDP кроме loopback-to-SOCKS5 — иначе утечка. Проверено тест-кейсом из B0.
- **Проверка:** unit-тест CONNECT-парсинга; локальный тест `curl -x` через bridge; проверка что байты не расшифровываются; нет UDP-egress мимо прокси.
- **Изолированно проверяемо:** да (локально без iOS).

### B3. Flutter iOS-проект (каркас)
- **Deliverable:** `flutter create --platforms=ios .`; Bundle ID `com.isolatedbrowser.app`; **deployment target 17.0** (не 14 — по Gate 0b!); интеграция `.a`/xcframework в Xcode; Info.plist (NSAppTransportSecurity localhost).
- **Проверка:** пустое приложение собирается и запускается на симуляторе/устройстве; `transport_version()` вызывается из Dart.
- **Изолированно проверяемо:** да (запуск каркаса).

### B4. WKWebView нативный plugin (Swift)
- **Deliverable:** `WKWebViewPluginFactory` + `WKWebViewWrapper`; эталонная конфигурация `proxyConfigurations` (`allowFailover=false`, конфиг ДО первого запроса); MethodChannel (`onPageStarted/onPageFinished/reload/goBack/loadUrl`); регистрация в `AppDelegate.swift`.
- **Проверка:** WKWebView грузит страницу через прокси на симуляторе; колбэки навигации работают.
- **Изолированно проверяемо:** да (на локальном bridge, без реального gateway).

### B5. browser_screen.dart — UiKitView для iOS
- **Deliverable:** платформо-зависимый виджет (Android→AndroidView/Gecko, iOS→UiKitView/WKWebView), передача `socks_proxy_host/port`.
- **Проверка:** на iOS рендерится WKWebView, на Android ничего не сломано.
- **Изолированно проверяемо:** да.

### B6. Интеграция + тесты (Phase 4)  ⚠️ ВОРОТА G-iOS
- **Deliverable:** полная цепочка WKWebView→HTTP-bridge→SOCKS5→Steal-TLS→gateway на реальном iPhone.
- **Проверка (критерии приёмки IOS_SPEC §13):** страница грузится через туннель; **нет VPN-иконки**; другие приложения не затронуты; подписки/промокоды/rate-limit работают; start/stop без утечек памяти; AMO-failover работает и на iOS (наследуется из БЛОКА A).
- **Проверяемо:** end-to-end на устройстве + Wireshark.

### B7. App Store подготовка (Phase 5)
- **Deliverable:** терминология «secure browsing» (без VPN/tunnel/gateway); без NEVPNManager/Network Extension entitlement; Privacy Policy; демо-аккаунт + Review Notes; TestFlight.
- **Проверка:** аудит бинарных строк на запрещённые слова; проверка entitlements; TestFlight-сборка проходит.
- **Проверяемо:** чек-лист + отклик Review.

---

## Классификация этапов по типу проверки

- **Чистые изолированные модули** (тесты без сети/железа, легко ревьюить): A1, A2, B1, B2.
- **Интеграционные точки** (нужен mock/локальный прогон): A3, A4, B4.
- **«Живые» ворота** (device + Wireshark): A8, B0, B6.

---

## ФИКСАЦИЯ ДЛЯ ANDROID-РАЗРАБОТЧИКОВ

> Отдельный зафиксированный раздел: **где и когда Android-команда получает код аварийного переключения (failover).**

### Ключевой принцип
Код аварийного переключения пишется **один раз в Rust core** (`transport-core`), а НЕ отдельно под Android. Android получает его автоматически через тот же FFI-контракт и общий бинарь. **Отдельной Android-реализации failover не будет и не требуется.**

### Где находится код failover
| Слой | Файл / модуль | Что содержит |
|---|---|---|
| Rust core (логика) | `transport-core/src/failover.rs` | `FailoverManager`, per-endpoint cooldown, `report_failure`, `try_switch`, `current_endpoint` |
| Rust core (конфиг) | `transport-core/src/` (EndpointConfig + serde) | Парсинг/валидация JSON-массива endpoints |
| Rust core (интеграция) | `transport-core/src/proxy.rs` | Использование `FailoverManager` в `Socks5Proxy` |
| Rust core (FFI-граница) | `transport-core/src/lib.rs` | `transport_start(...endpoints_json...)`, `transport_get_status()` |
| Dart (Android потребитель) | `models/endpoint.dart`, `transport_service.dart` | Сериализация endpoints в JSON → передача в FFI |
| UI (Android) | `settings_service.dart`, `settings_screen.dart` | Список endpoints, reorder, миграция старых настроек |

### Когда Android получает код failover — по этапам

| Момент | Этап | Что именно доступно Android-разработчикам |
|---|---|---|
| **Заморозка контракта** | **после A4 (G-Contract)** | Финальная FFI-сигнатура `transport_start` + `transport_get_status`. С этого момента Android-команда может начинать интеграцию, зная, что контракт **не изменится**. |
| **Готовые Dart-биндинги** | **после A5** | `GatewayEndpoint` + обновлённый `transport_service.dart`. Android-приложение уже может вызывать multi-endpoint `start()`. |
| **Настройки + миграция** | **после A6–A7** | Пользователь на Android может добавлять/сортировать endpoints; старые настройки мигрируют автоматически в `endpoints[0]`. |
| **Полностью рабочий failover на Android** | **после A8 (G-AMO)** | Аварийное переключение **проверено на реальном устройстве** (Redmi Note 9 Pro). Это точка, когда фича считается сданной для Android. |

### Что Android-команде делать НЕ нужно
- ❌ НЕ писать собственную логику переключения на Kotlin/Java.
- ❌ НЕ дублировать таймеры/cooldown в Android-слое.
- ❌ НЕ хранить список gateway отдельно от общего JSON-контракта.
- ✅ Только: собрать общий Rust-бинарь (`build-transport.yml`), передать `endpoints_json` через FFI, отобразить статус из `transport_get_status()`.

### Итоговая точка ответственности
- **Код failover сдаётся Android-команде на этапе A4** (контракт заморожен, можно интегрировать).
- **Failover считается работающим на Android на этапе A8** (проверено on-device, ворота G-AMO пройдены).
