# Backlog: Isolated Private Browser

**Обновлено:** 2026-06-04

---

## Фаза 0 — Проектирование (текущая)

- [ ] Спецификация кастомного TLS-транспортного протокола (`docs/transport-protocol.md`)
- [ ] Детальная архитектура системы (`docs/architecture.md`)
- [ ] Описание GeckoView интеграции (`docs/platform-android.md`)
- [ ] Описание WKWebView + прокси на iOS (`docs/platform-ios.md`)
- [ ] Описание Windows WebView2 (`docs/platform-windows.md`)

## Фаза 1 — Транспортный модуль (MVP core)

- [ ] Прототип Rust-библиотеки кастомного протокола поверх TLS
- [ ] Flutter FFI-обёртка для транспортного модуля
- [ ] Прототип сервера-шлюза (Go/Rust)
- [ ] Тест сквозного соединения: клиент → шлюз → целевой сайт

## Фаза 2 — Android MVP

- [ ] Flutter приложение с GeckoView плагином
- [ ] Встроенный SOCKS5 прокси (127.0.0.1)
- [ ] Настройка GeckoView через `setProxy()` на локальный прокси
- [ ] Подключение транспортного модуля (Rust FFI)
- [ ] Тест: Telegram Web, WhatsApp Web через GeckoView
- [ ] Тест: значок service не появляется

## Фаза 3 — iOS MVP

- [ ] WKWebView + WKURLSchemeHandler интеграция
- [ ] Локальный HTTP-сервер внутри приложения (Swift)
- [ ] Подключение транспортного модуля
- [ ] Ограниченный список поддерживаемых сервисов (без полного WebRTC)
- [ ] Тест App Store compliance (нет service API)

## Фаза 4 — Windows MVP

- [ ] Flutter Windows + WebView2
- [ ] Транспортный модуль (тот же Rust)
- [ ] Тест сквозного соединения

## Фаза 5 — Тестирование обхода

- [ ] Тест на российских провайдерах (ТСПУ)
- [ ] Тест в ОАЭ (Etisalat/Du DPI)
- [ ] Тест в Иране
- [ ] Стресс-тест: YouTube, Telegram видео, WebSocket-соединения

## Фаза 6 — Android TV / Роутер

- [ ] Android TV: адаптация UI под D-pad (тот же Flutter)
- [ ] Роутер: нативный транспортный клиент (Rust/Go binary)
