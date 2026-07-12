# Phase 2 Production Readiness — План реализации

**Проект:** Isolated Browser
**Создан:** 2026-07-12
**Текущая readiness:** ~45% (по claude-sonnet-5 Phase 1 review)
**Целевая readiness:** 60-65%
**Сервер:** 38.180.253.219 (2 CPU, 4GB RAM, Intel Xeon Gold 6154 @ 3.00GHz)

---

## Источники рекомендаций

| # | Ревизор | Вердикт | Readiness | Файл |
|---|---------|---------|-----------|------|
| 1 | GPT-5.6-sol | REJECT | 35% | — |
| 2 | Claude-opus-4.8 | CONDITIONAL APPROVE | 35-45% | — |
| 3 | Claude-sonnet-5 (plan) | CONDITIONAL APPROVE | 35-45% → 60-65% | plan_approval_utf8_final.txt |
| 4 | Claude-sonnet-5 (Phase 1) | — | ~45% | reviewer_phase1_result.txt |

**Phase 1 статус:** Все 7 правок выполнены, stress test PASSED (800/800 valid, 150/150 rejected, 0 errors).

---

## Phase 2 — План (12 задач, приоритезирован)

### P0 — Критичные (блокеры production)

#### 1. Forward Secrecy (ECDHE per session)
**Ревизор:** claude-sonnet-5 Phase 1, п.1 security measures
**Проблема:** Текущий ключ — статический PSK. Компрометация ключа = дешифровка всего прошлого трафика.
**Решение:**
- Ephemeral X25519 key exchange на каждую сессию (ECDHE)
- Session key = HKDF(X25519(client_ephemeral, server_ephemeral))
- Server ephemeral key ротируется per-connection (не static)
- PFS: компрометация server key не раскрывает прошлые сессии
**Файлы:**
- `transport-core/src/steal.rs` — extend auth frame (exchange ephemeral keys, derive session key)
- `transport-core/src/protocol.rs` — FrameCodec::new() принимает session-derived key
- `gateway/src/main.rs` — server-side ephemeral key generation per connection
**Тесты:**
- Unit: two ECDHE exchanges produce same session key
- Unit: different sessions have different keys
- Integration: key compromise doesn't decrypt past sessions
**Оценка:** 2-3 дня

#### 2. Completed Rekey Handshake (end-to-end)
**Ревизор:** claude-sonnet-5 Phase 1, п.1 critical
**Проблема:** `RekeyNeeded` — только триггер. Нет протокола доставки нового ключа.
**Решение:**
- In-band control frame: `REKEY_INIT` (server → client, содержит new kid + new ephemeral public)
- Client отвечает `REKEY_ACK` (подтверждение готовности)
- Overlap window: оба ключа валидны N секунд (dual-validation)
- После overlap — старый key удаляется
- Downgrade-защита: kid monotonic, reject lower kid
**Файлы:**
- `transport-core/src/protocol.rs` — control frame types (REKEY_INIT, REKEY_ACK)
- `transport-core/src/steal.rs` — rekey handshake protocol
- `gateway/src/main.rs` — server initiates rekey at threshold
**Тесты:**
- Unit: rekey produces new valid key
- Unit: old key rejected after overlap window
- Unit: downgrade attack rejected
- Integration: 2^32 frames → rekey → continued operation
**Оценка:** 2-3 дня

#### 3. Fuzzing FrameCodec (cargo-fuzz)
**Ревизор:** claude-sonnet-5 Phase 1, п.3 security + plan approval п.7
**Проблема:** Парсер бинарного протокола — классический вектор атаки (malformed length, integer overflow).
**Решение:**
- `cargo-fuzz` targets:
  - `fuzz_read_frame` — random bytes → read_frame (no crash/panic)
  - `fuzz_write_frame` — random plaintext → write_frame → read_frame roundtrip
  - `fuzz_handshake_state_machine` — random frame sequences
- CI: 24h continuous fuzzing, 0 crashes required
- Coverage: ASAN + UBSAN
**Файлы:**
- `transport-core/fuzz/fuzz_targets/fuzz_read_frame.rs`
- `transport-core/fuzz/fuzz_targets/fuzz_write_frame.rs`
- `transport-core/fuzz/fuzz_targets/fuzz_handshake.rs`
- `transport-core/fuzz/Cargo.toml`
**Оценка:** 1-2 дня + 24h fuzzing run

#### 4. Soak Test (24h+ continuous)
**Ревизор:** claude-sonnet-5 Phase 1, testing
**Проблема:** Нет проверки на утечки памяти/FD/counter overflow при длительной работе.
**Решение:**
- Long-running test: 1000 concurrent connections, 24h
- Мониторинг: RSS, FD count, counter values (монотонный рост)
- Auto-reconnect после rekey threshold
- Assertions: RSS < 100MB после 24h, FD < 1000, no panics
**Файлы:**
- `gateway/tests/soak_test.rs`
- `gateway/tests/soak_monitor.sh` (CPU/RAM/FD logging)
**Оценка:** 1 день + 24h run

---

### P1 — Высокий приоритет

#### 5. Observability (Prometheus metrics + alerting)
**Ревизор:** claude-sonnet-5 Phase 1, operational
**Проблема:** Нет метрик, нет security-логирования, нет алертинга.
**Решение:**
- Prometheus endpoint (`/metrics`) на gateway:
  - `gateway_connections_active` (gauge)
  - `gateway_connections_total` (counter)
  - `gateway_auth_failures_total` (counter, labels: type=invalid|replay|truncated)
  - `gateway_handshake_latency_seconds` (histogram, p50/p95/p99)
  - `gateway_cpu_percent`, `gateway_memory_bytes`, `gateway_fd_count`
  - `gateway_rekey_total`, `gateway_replay_detected_total`
- Structured logging (tracing-subscriber JSON output)
- Alert rules: spike in auth_failures > N/min = attack signal
**Файлы:**
- `gateway/src/metrics.rs` (new)
- `gateway/src/main.rs` — integrate metrics middleware
- `gateway/Cargo.toml` — add `prometheus` crate
**Оценка:** 2 дня

#### 6. Bounded IpRateLimiter — adversarial test
**Ревизор:** claude-sonnet-5 Phase 1, п.5 security (уже реализован cap=10000 + LRU)
**Проблема:** Не верифицирован под adversarial many-IP нагрузкой.
**Решение:**
- Test: 50000 distinct IPs in 60s (simulate botnet)
- Assert: memory bounded (RSS growth < 10MB)
- Assert: LRU eviction works (oldest entries removed)
- Test: Slowloris (1000 connections, no auth, hold 300s)
- Assert: legitimate clients still served
**Файлы:**
- `gateway/tests/adversarial_test.rs`
**Оценка:** 1 день

#### 7. Threat Model Document
**Ревизор:** claude-sonnet-5 Phase 1, п.4 security
**Проблема:** Нет явной threat model — какие гарантии протокол даёт против каждого типа атакующего.
**Решение:**
- Документ: `docs/THREAT_MODEL.md`
- Атакующие:
  - Passive DPI (ТСПУ) — наблюдение SNI, TLS fingerprint
  - Active MITM — перехват, модификация, replay
  - State-level censor — активные пробы, блокировка по IP
  - Malicious relay — компрометированный gateway
  - Network attacker — SYN flood, Slowloris, resource exhaustion
- Гарантии протокола против каждого
- Известные ограничения (что НЕ защищается)
**Файлы:**
- `docs/THREAT_MODEL.md`
**Оценка:** 1 день

#### 8. Secure Storage для JWT (Flutter)
**Ревизор:** original Phase 2 plan
**Проблема:** JWT хранится в SharedPreferences (plaintext, доступен root).
**Решение:**
- Заменить на `flutter_secure_storage` (Keychain на iOS, Keystore на Android)
- Шифрование JWT at rest
- Auto-refresh токена перед exp
**Файлы:**
- `app/lib/services/subscription_service.dart`
- `app/pubspec.yaml` — add `flutter_secure_storage`
**Оценка:** 0.5 дня

---

### P2 — Средний приоритет

#### 9. Multi-Endpoint Failover (медленный/детерминированный)
**Ревизор:** claude-sonnet-5 plan approval п.8
**Проблема:** Fast switch между gateway = DPI fingerprint (смена IP при неудаче).
**Решение:**
- Failover только по timeout (не по error)
- Min interval между переключениями: 60s
- Список endpoints в config (не dynamic discovery)
- На клиенте: deterministic priority list
**Файлы:**
- `transport-core/src/tls.rs` — endpoint rotation logic
- `app/lib/services/transport_service.dart` — endpoint config
**Оценка:** 1 день

#### 10. Защита Origin IP
**Ревизор:** original Phase 2 plan
**Проблема:** Fallback CDN может утечь origin IP.
**Решение:**
- Audit: проверить что fallback проксирует на CDN, не на origin
- Проверить DNS records (no A record for origin domain)
- TLS cert для CDN domain (не origin)
**Оценка:** 0.5 дня

#### 11. Audit promo.rs (constant-time verification)
**Ревизор:** claude-sonnet-5 Phase 1, п.5 security
**Проблема:** Нужно явное подтверждение что promo.rs применяет constant-time + unified-error.
**Статус:** Уже реализовано (subtle::ConstantTimeEq в verify_token).
**Решение:** Добавить тесты, документировать.
**Оценка:** 0.5 дня

#### 12. Реалистичный Scale Test (10k+, WAN)
**Ревизор:** claude-sonnet-5 Phase 1, п.8 path to 60-65%
**Проблема:** Только localhost тест, нет WAN условий.
**Решение:**
- 10000 concurrent connections (требует 4+ core сервер)
- WAN: клиент с другого гео (не localhost)
- Packet loss simulation (tc netem)
- MTU fragmentation test
- p99 < 300ms target (с учётом WAN RTT)
**Оценка:** 2 дня (требует аренды 4+ core VPS)

---

### P3 — Опционально (после 60-65%)

#### 13. Independent Security Review
**Ревизор:** claude-sonnet-5 plan approval п.7 + Phase 1
**Проблема:** Все ревизии — LLM текстовое ревью, не line-by-line аудит.
**Решение:**
- Внешний аудитор (человек или LLM с полным кодом)
- Line-by-line анализ: handshake, replay window, key rotation, FrameCodec
- Pentest: попытка обхода auth, replay, downgrade
**Оценка:** $200-500 (внешний аудитор)

#### 14. DPI Testbed Testing
**Ревизор:** GPT-5.6-sol + claude-sonnet-5
**Проблема:** DPI-resistance не протестирована против реального DPI.
**Решение:**
- Тест против реального ТСПУ (из России)
- Или: simulated DPI (nDPI, Suricata rules)
- Проверить: SNI masking, TLS fingerprint, fallback behavior
**Оценка:** 1-2 дня

---

## Порядок реализации

```
Week 1: P0 (1-4)
  ├── #1 Forward Secrecy (ECDHE)          [2-3 дня]
  ├── #2 Rekey Handshake                  [2-3 дня]
  ├── #3 Fuzzing setup                    [1-2 дня + 24h run]
  └── #4 Soak Test                        [1 день + 24h run]

Week 2: P1 (5-8)
  ├── #5 Observability (Prometheus)       [2 дня]
  ├── #6 Adversarial Test                 [1 день]
  ├── #7 Threat Model Doc                 [1 день]
  └── #8 Secure Storage JWT               [0.5 дня]

Week 3: P2 (9-12)
  ├── #9 Multi-Endpoint Failover          [1 день]
  ├── #10 Origin IP Protection            [0.5 дня]
  ├── #11 promo.rs Audit                  [0.5 дня]
  └── #12 Scale Test 10k+ WAN             [2 дня]

Week 4: P3 (опционально)
  ├── #13 Independent Security Review     [external]
  └── #14 DPI Testbed                     [1-2 дня]
```

---

## Acceptance Criteria (для 60-65%)

| Критерий | Цель | Источник |
|----------|------|----------|
| Latency p99 (100 concurrent) | < 100ms (loopback) | reviewer п.1 |
| Latency p99 (1000 concurrent, 4+ core) | < 300ms | reviewer п.8 |
| RAM (1000 conn) | < 100 MB | plan approval |
| CPU (1000 conn) | < 5% (4+ core) | plan approval |
| FD count (1000 conn) | < 1000 | — |
| Fuzzing | 0 crashes / 24h | reviewer п.3 |
| Soak test | 24h, no leaks | reviewer п.4 |
| Forward secrecy | ECDHE per session | reviewer п.1 |
| Rekey handshake | end-to-end + overlap | reviewer п.1 |
| Observability | Prometheus + alerts | reviewer п.7 |
| Threat model | documented | reviewer п.4 |
| Security review | external audit | reviewer п.6 |

**Hardware baseline (нормировка):** 4-core CPU, 8GB RAM, SSD (для scale test)

---

## Бюджет ревизора

| Фаза | Модель | Стоимость |
|------|--------|-----------|
| Phase 1 plan | claude-sonnet-5 | $0.045 |
| Phase 1 results | claude-sonnet-5 | $0.071 |
| Phase 2 review (план) | claude-sonnet-5 | ~$0.05 |
| Phase 2 review (результаты) | claude-sonnet-5 | ~$0.07 |
| **Итого** | | **~$0.24** |

Бюджет: $0.50 max. Запас: ~$0.26.

---

## Файлы для создания/изменения

### Новые файлы
- `transport-core/fuzz/Cargo.toml`
- `transport-core/fuzz/fuzz_targets/fuzz_read_frame.rs`
- `transport-core/fuzz/fuzz_targets/fuzz_write_frame.rs`
- `transport-core/fuzz/fuzz_targets/fuzz_handshake.rs`
- `gateway/tests/soak_test.rs`
- `gateway/tests/adversarial_test.rs`
- `gateway/src/metrics.rs`
- `docs/THREAT_MODEL.md`

### Изменяемые файлы
- `transport-core/src/steal.rs` — ECDHE, rekey handshake
- `transport-core/src/protocol.rs` — control frames (REKEY_INIT, REKEY_ACK)
- `gateway/src/main.rs` — metrics, ephemeral keys, rekey
- `gateway/Cargo.toml` — prometheus, cargo-fuzz
- `app/lib/services/subscription_service.dart` — flutter_secure_storage
- `app/lib/services/transport_service.dart` — multi-endpoint config
- `transport-core/src/tls.rs` — endpoint rotation

---

## Ключевые архитектурные решения

1. **ECDHE per session** — не per-frame, не per-rekey. Одна сессия = один ephemeral key.
2. **Rekey** — in-band control frame, overlap window 60s, kid monotonic.
3. **Fuzzing** — cargo-fuzz + libFuzzer, 24h continuous, ASAN/UBSAN.
4. **Metrics** — Prometheus exposition format, /metrics endpoint.
5. **Failover** — deterministic, 60s min interval, не reactive.
6. **Secure storage** — flutter_secure_storage (Keystore/Keychain).
7. **Threat model** — документ, не код. Обоснование решений.

---

## Риски

1. **ECDHE может увеличить latency** — X25519 вычисление per-connection. Mitigation: pre-generate server ephemeral pool.
2. **Fuzzing может найти баги** — потребует fixes, может задержать. Mitigation: заложить 2 дня на fixes.
3. **Scale test требует 4+ core VPS** — текущий сервер 2-core. Mitigation: аренда временного VPS.
4. **Independent review требует бюджет** — $200-500 внешнему аудитору. Mitigation: LLM line-by-line как fallback.
5. **DPI testbed требует доступ из РФ** — может быть затруднён. Mitigation: simulated DPI.
