# Threat Model — Isolated Browser Gateway

**Version:** 1.0 (Phase 2)
**Date:** 2026-07-13

---

## 1. Overview

Isolated Browser — система для обхода DPI-блокировок через steal-TLS gateway.
Клиент (Flutter app) подключается к gateway, маскируя трафик под легитимный TLS
к handshake к CDN (например, cloudflare.com). После TLS handshake клиент
проходит ECDHE аутентификацию и устанавливает зашифрованный туннель (ChaCha20-Poly1305)
для проксирования трафика к целевым сайтам.

### Архитектура
```
[Client App] → [SOCKS5 proxy] → [TLS (SNI=CDN)] → [Gateway] → [Target site]
                                     ↓
                              [DPI / ТСПУ наблюдает]
```

### Компоненты
- **Client** (Flutter + Rust transport-core): SOCKS5 proxy, ECDHE client, FrameCodec
- **Gateway** (Rust): TLS terminator, ECDHE server, relay, rate limiter
- **CDN** (fallback): реальный CDN для неавторизованных соединений (stealth)

---

## 2. Attackers

### 2.1 Passive DPI (ТСПУ)
**Capabilities:**
- Наблюдает весь TCP трафик
- Извлекает SNI из TLS ClientHello
- Анализирует TLS fingerprint (JA3/JA4)
- Блокирует по сигнатурам (домены, IP, протоколы)

**Mitigations:**
- SNI = CDN domain (cloudflare.com) — выглядит как легитимный трафик к CDN
- TLS handshake через rustls (стандартный fingerprint)
- Application data маскирован под TLS 1.3 record (0x17, 0x0303)
- FrameCodec payload = ChaCha20-Poly1305 (неразличим от случайных байт)

**Residual risks:**
- DPI может заблокировать gateway IP по другим признакам ( traffic volume, packet timing)
- TLS fingerprint rustls может отличаться от браузеров (Phase 3: uTLS)

### 2.2 Active MITM
**Capabilities:**
- Перехват, модификация, повтор TCP пакетов
- Active probing: подключается к gateway и пытается определить сервис
- Replay атак

**Mitigations:**
- TLS handshake: MITM без CDN private key не может расшифровать
- ECDHE per-session: replay auth_token не раскрывает session_key (forward secrecy)
- Replay protection: sliding window (64 frames), counter-based nonce
- Constant-time auth verification (subtle::ConstantTimeEq)
- Unified error response ("ERR") — no timing oracle
- Fallback к CDN для неавторизованных проб (steal-oncall)

**Residual risks:**
- MITM может блокировать TLS handshake (DoS)
- Replay auth_token до завершения TLS — возможен, но session_key не раскрывается

### 2.3 State-level censor
**Capabilities:**
- Активные пробы gateway IP
- Блокировка IP по гео/ASN
- Корреляция трафика (timing, volume)
- Запросы к CDN для верификации

**Mitigations:**
- Steal-oncall: неавторизованные пробы → fallback к реальному CDN
- Gateway отвечает как CDN для неавторизованных (real TLS handshake to cloudflare.com)
- No server-side state leak (auth failures = "ERR")

**Residual risks:**
- Censor может заблокировать gateway IP по подозрению (volume-based)
- Корреляция client↔gateway traffic patterns (Phase 3: traffic shaping)

### 2.4 Malicious relay (compromised gateway)
**Capabilities:**
- Полный доступ к plaintext трафику после расшифровки
- Модификация, логирование, инъекция

**Mitigations:**
- Forward secrecy: compromising server_static_secret НЕ раскрывает прошлые сессии
  (ECDHE ephemeral keys уничтожаются после соединения)
- Session keys уникальны per-connection (HKDF из ECDHE shared secret)
- Rekey каждые ~4B фреймов (rotation)

**Residual risks:**
- Compromised gateway видит plaintext в реальном времени
- Mitigation: end-to-end encryption (Phase 3: double encryption)

### 2.5 Network attacker (DoS)
**Capabilities:**
- SYN flood, Slowloris, resource exhaustion
- Connection flooding
- Bandwidth exhaustion

**Mitigations:**
- Per-IP rate limiting: 20 conn/min, LRU cap=10000 IPs
- Connection semaphore: max 5000 concurrent
- Idle timeout: 300s
- Frame size limits: 16KB payload, 64KB max frame
- Bounded queues (no unbounded growth)

**Residual risks:**
- Volumetric DDoS (требует CDN/Anti-DDoS провайдера)
- DDoS может исчерпать semaphore (легитимные клиенты получат reject)

---

## 3. Protocol Guarantees

### 3.1 Confidentiality
- TLS 1.3 (rustls) между client и gateway
- ChaCha20-Poly1305 AEAD для application data
- Forward secrecy: ECDHE per-session, server ephemeral keys destroyed
- Session key ≠ auth token (HKDF label separation)

### 3.2 Integrity
- ChaCha20-Poly1305 AEAD (authenticated encryption)
- Any tampering → decryption failure → connection close
- Counter-based nonce (monotonic, no reuse)

### 3.3 Authenticity
- ECDHE handshake: client доказывает знание server_static_public
- Auth token = HKDF(X25519(client_ephemeral, server_static), "auth")
- Constant-time comparison (no timing side-channel)
- Server generates fresh ephemeral per-connection (PFS)

### 3.4 Replay Protection
- Sliding window (64 frames) per-session
- Counter-based nonce (u32, monotonic)
- Replay → RekeyNeeded or connection close
- Counter overflow → rekey (threshold 0xF0000000)

### 3.5 Forward Secrecy
- Compromising server_static_secret does NOT reveal past sessions
- Server ephemeral secret destroyed after each connection
- Session key = HKDF(X25519(client_eph, server_eph), "session")
- Rekey rotation каждые ~4B фреймов

---

## 4. Known Limitations

### 4.1 NOT Protected
- **Plaintext at gateway**: gateway расшифровывает и видит plaintext (нет E2E)
- **Traffic analysis**: размер/тайминг пакетов могут выдать тип трафика
- **Volumetric DDoS**: требует external Anti-DDoS
- **Client-side key compromise**: если client key утечёл — атакующий может подключиться
- **Gateway IP blocking**: censor может заблокировать IP без доказательств

### 4.2 Phase 3 (future)
- uTLS fingerprint mimicking (браузерные JA3/JA4)
- Traffic shaping (padding, timing obfuscation)
- Multi-hop (chain of gateways)
- E2E encryption (double encryption client↔target через gateway)
- Client-side multisplit/multidisorder (DPI bypass на клиенте)

---

## 5. Cryptographic Primitives

| Purpose | Algorithm | Parameters |
|---------|-----------|------------|
| Key exchange | X25519 | ECDHE, ephemeral per-session |
| Auth token | HKDF-SHA256 | info="isolated-browser-auth" |
| Session key | HKDF-SHA256 | info="isolated-browser-session" |
| Symmetric encryption | ChaCha20-Poly1305 | AEAD, 256-bit key |
| Nonce | 12 bytes | prefix(8) \|\| counter(4) |
| Replay window | Sliding window | 64 frames |
| Rekey threshold | 0xF0000000 | ~4B frames |
| Rekey overlap | 256 frames | dual-validation window |

### Key Separation
- AUTH_INFO = b"isolated-browser-auth" (auth token derivation)
- SESSION_INFO = b"isolated-browser-session" (session key derivation)
- Different HKDF labels ensure auth_token ≠ session_key

---

## 6. Security Review Checklist

- [x] Constant-time auth verification (subtle::ConstantTimeEq)
- [x] Unified error responses (no oracle)
- [x] Forward secrecy (ECDHE per-session)
- [x] Replay protection (sliding window)
- [x] Rekey handshake (REKEY_INIT/ACK, overlap)
- [x] Downgrade protection (kid monotonic)
- [x] Frame size limits (16KB payload)
- [x] Per-IP rate limiting (LRU bounded)
- [x] Connection semaphore (5000 max)
- [x] Idle timeout (300s)
- [x] Fuzzing (71028 runs, 0 crashes)
- [x] Soak test (32540 conn, 0 leaks)
- [x] Key separation (HKDF labels)
- [ ] External security audit (Phase 3)
- [ ] DPI testbed testing (Phase 3)
- [ ] uTLS fingerprint (Phase 3)

---

## 7. Incident Response

### Auth failure spike
- Monitor: `gateway_auth_failures_total` > N/min
- Action: investigate source IPs, consider blocking

### Replay detected
- Monitor: `gateway_replay_detected_total`
- Action: normal (replay attacks expected), но spike = active attack

### Rekey failures
- Monitor: `gateway_rekey_total` flat + connection drops
- Action: check client compatibility, protocol version

### Resource exhaustion
- Monitor: `gateway_connections_active` near 5000
- Action: scale horizontally or raise semaphore
