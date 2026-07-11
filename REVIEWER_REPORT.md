# Отчет для ревизора — Isolated Browser steal-TLS реализация

**Дата:** 2026-07-03
**Бюджет:** ~$0.02 (max 2000 токенов)
**Модель:** anthropic/claude-sonnet-5

---

## Контекст

Isolated Browser — Flutter + Rust FFI приложение для обхода DPI в России 2026.
Не VPN (нет VpnService), использует SOCKS5 proxy внутри приложения.

**Предыдущий вердикт ревизора v2:**
"VLESS+Reality избыточен. Нужна steal-TLS техника на Rust-транспорте:
- Убрать magic number 0x49425257 → X25519 аутентификация
- Маскировать payload под TLS Application Data (0x17 0x03 0x03)
- SNI ротация (пул CDN-доменов)
- Stealth mode: при невалидном auth — закрытие соединения"

---

## Что сделано (P0 задачи)

### 1. protocol.rs — Маскировка под TLS Application Data
```rust
const TLS_CONTENT_TYPE: u8 = 0x17;
const TLS_VERSION_MAJOR: u8 = 0x03;
const TLS_VERSION_MINOR: u8 = 0x03;

// Фрейм: 0x17 0x03 0x03 + length (u16) + nonce (12B) + ciphertext
```

### 2. auth.rs — X25519 аутентификация
```rust
pub struct AuthKeyPair {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

impl AuthKeyPair {
    pub fn derive_shared_key(&self, peer_public: &PublicKey) -> [u8; 32] {
        let shared_secret = self.secret.diffie_hellman(peer_public);
        let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut derived_key = [0u8; 32];
        hkdf.expand(b"isolated-browser-auth", &mut derived_key).unwrap();
        derived_key
    }
}
```

### 3. steal.rs — Steal-oncall handshake
```rust
pub struct ClientAuth {
    pub ephemeral_public: [u8; 32],
    pub auth_token: [u8; 32],
}

pub fn generate_client_auth(server_public: &[u8; 32]) -> ClientAuth {
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let server_public_key = PublicKey::from(*server_public);
    let shared_secret = ephemeral_secret.diffie_hellman(&server_public_key);
    
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(None, shared_secret.as_bytes());
    let mut auth_token = [0u8; 32];
    hkdf.expand(b"isolated-browser-auth", &mut auth_token).unwrap();
    
    ClientAuth {
        ephemeral_public: *ephemeral_public.as_bytes(),
        auth_token,
    }
}

// Клиент отправляет: ephemeral_public (32B) + auth_token (32B) = 64 bytes
// Сервер проверяет: вычисляет expected_token из ephemeral_public + server_secret
```

### 4. proxy.rs — Интеграция
```rust
// После TLS handshake:
crate::steal::send_auth_frame(&mut tls_stream, &server_public).await?;
// Затем CONNECT frame (замаскирован под TLS Application Data)
```

### 5. gateway/src/main.rs — Серверная часть
```rust
async fn handle_client(mut stream, key, server_private_key) {
    // 1. Читаем auth frame
    let (ephemeral_public, auth_token) = read_auth_frame(&mut stream).await?;
    
    // 2. Проверяем auth
    if !verify_server_auth(&server_private_key, &ephemeral_public, &auth_token) {
        debug!("auth failed — closing connection (stealth mode)");
        return Ok(());  // Stealth: закрываем без ошибки
    }
    
    // 3. Читаем CONNECT frame
    let frame = codec.read_frame(&mut stream).await?;
    // ... relay
}
```

### 6. tls.rs — SNI ротация
```rust
pub struct TlsClient {
    connector: TlsConnector,
    sni_pool: Vec<String>,  // ["cloudflare.com", "google.com", ...]
    current_sni_idx: usize,
}

pub async fn connect(&mut self, host, port) {
    let sni = self.current_sni().to_string();
    // TLS ClientHello с SNI = CDN-домен
    self.rotate_sni();  // Автопереключение после каждого соединения
}
```

---

## Архитектура (финальная)

```
[GeckoView] → SOCKS5 127.0.0.1:18080 → [Rust Transport]
                                              │
                                    1. TCP connect к Gateway:443
                                    2. TLS ClientHello с SNI = CDN-домен (из pool, ротация)
                                    3. TLS handshake с сертификатом CDN-домена
                                    4. Auth frame: ephemeral X25519 public (32B) + HKDF token (32B)
                                    5. Сервер проверяет token → если невалиден, закрывает (stealth)
                                    6. CONNECT host:port фрейм (замаскирован под TLS Application Data)
                                    7. Relay: payload в TLS Application Data records (0x17 0x03 0x03)
```

---

## Вопрос к ревизору

**Правильно ли реализована steal-TLS техника для обхода ТСПУ 2026?**

1. **X25519 аутентификация:** Клиент генерирует ephemeral keypair, отправляет public + HKDF-derived token. Сервер проверяет через свой static secret. Это защищает от active probing?

2. **Auth frame placement:** Auth frame отправляется ПОСЛЕ TLS handshake (не в TLS extension). Это правильно или нужно в TLS extension?

3. **Stealth mode:** При невалидном auth сервер закрывает соединение без ошибки. Это достаточно для stealth или нужно проксировать на реальный CDN?

4. **SNI ротация:** Пул CDN-доменов с автопереключением. Это достаточно или нужна дополнительная логика (например, при блокировке)?

5. **Payload маскировка:** Фреймы замаскированы под TLS Application Data (0x17 0x03 0x03). Это выглядит как настоящий TLS трафик для DPI?

---

## Ограничения ревизора

- **Бюджет:** ~$0.02 (max 2000 токенов)
- **Модель:** anthropic/claude-sonnet-5
- **Право на вопросы:** До 3 итераций (но если ситуация ясна — не задавать)
- **Задача:** Сжатый анализ, фиксация вывода

---

## Ожидаемый ответ ревизора

1. Подтверждение правильности реализации ИЛИ
2. Указание на критические ошибки ИЛИ
3. Рекомендации по улучшению (если есть)

**Формат:** Краткое заключение (1-2 абзаца) + список проблем (если есть).
