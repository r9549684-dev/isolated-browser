// transport-core/src/endpoint.rs

use serde::Deserialize;

/// Конфигурация одного gateway-эндпоинта.
/// Приходит с Dart-стороны как элемент JSON-массива через FFI (transport_start).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub host: String,
    pub port: u16,
    pub sni: String,
    /// hex-encoded 32-byte X25519 public key of the server (регистр не важен)
    pub server_pub: String,
    /// hex-encoded 32-byte pre-shared key (регистр не важен)
    pub psk: String,
}

#[derive(Debug)]
pub enum EndpointConfigError {
    EmptyList,
    TooManyEndpoints(usize),
    InvalidHex { field: &'static str, index: usize },
    WrongHexLength { field: &'static str, index: usize, expected: usize, got: usize },
    EmptyHost { index: usize },
    EmptySni { index: usize },
    ZeroPort { index: usize },
    DuplicateEndpoint { index: usize, host: String, port: u16 },
    MalformedJson(String),
}

impl std::fmt::Display for EndpointConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyList => write!(f, "endpoint list is empty"),
            Self::TooManyEndpoints(n) => write!(f, "too many endpoints: {n} (max 16)"),
            Self::InvalidHex { field, index } => {
                write!(f, "invalid hex in field '{field}' at endpoint index {index}")
            }
            Self::WrongHexLength { field, index, expected, got } => write!(
                f,
                "field '{field}' at index {index} has wrong length: expected {expected} hex chars, got {got}"
            ),
            Self::EmptyHost { index } => write!(f, "empty host at endpoint index {index}"),
            Self::EmptySni { index } => write!(f, "empty sni at endpoint index {index}"),
            Self::ZeroPort { index } => write!(f, "port is 0 at endpoint index {index}"),
            Self::DuplicateEndpoint { index, host, port } => {
                write!(f, "duplicate endpoint at index {index}: {host}:{port} already present")
            }
            Self::MalformedJson(msg) => write!(f, "malformed JSON: {msg}"),
        }
    }
}

impl std::error::Error for EndpointConfigError {}

const MAX_ENDPOINTS: usize = 16;
const KEY_HEX_LEN: usize = 64; // 32 bytes -> 64 hex chars

/// Парсит и валидирует JSON-массив EndpointConfig, приходящий из FFI.
/// Fail-closed: любая неопределённость (лишнее поле, дубликат, нулевой порт,
/// пустой host/sni) — ошибка, а не молчаливое принятие.
pub fn parse_endpoints(json: &str) -> Result<Vec<EndpointConfig>, EndpointConfigError> {
    let endpoints: Vec<EndpointConfig> =
        serde_json::from_str(json).map_err(|e| EndpointConfigError::MalformedJson(e.to_string()))?;

    if endpoints.is_empty() {
        return Err(EndpointConfigError::EmptyList);
    }
    if endpoints.len() > MAX_ENDPOINTS {
        return Err(EndpointConfigError::TooManyEndpoints(endpoints.len()));
    }

    let mut seen: Vec<(String, u16)> = Vec::with_capacity(endpoints.len());

    for (i, ep) in endpoints.iter().enumerate() {
        if ep.host.trim().is_empty() {
            return Err(EndpointConfigError::EmptyHost { index: i });
        }
        if ep.sni.trim().is_empty() {
            return Err(EndpointConfigError::EmptySni { index: i });
        }
        if ep.port == 0 {
            return Err(EndpointConfigError::ZeroPort { index: i });
        }
        validate_hex_field("server_pub", &ep.server_pub, i)?;
        validate_hex_field("psk", &ep.psk, i)?;

        let key = (ep.host.clone(), ep.port);
        if seen.contains(&key) {
            return Err(EndpointConfigError::DuplicateEndpoint { index: i, host: ep.host.clone(), port: ep.port });
        }
        seen.push(key);
    }

    Ok(endpoints)
}

fn validate_hex_field(field: &'static str, value: &str, index: usize) -> Result<(), EndpointConfigError> {
    if value.len() != KEY_HEX_LEN {
        return Err(EndpointConfigError::WrongHexLength {
            field,
            index,
            expected: KEY_HEX_LEN,
            got: value.len(),
        });
    }
    if !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(EndpointConfigError::InvalidHex { field, index });
    }
    Ok(())
}

/// Единая утилита hex64 → [u8; 32]. Используется здесь для будущей валидации
/// и в A3 (`proxy.rs`) вместо дублирующегося decode_hex32 — устраняет
/// расхождение, отмеченное ревизором (тройное дублирование hex-декодирования).
pub fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != KEY_HEX_LEN {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_key() -> String {
        "a".repeat(64)
    }

    fn make_endpoint_json(host: &str, port: u16, sni: &str, server_pub: &str, psk: &str) -> String {
        format!(
            r#"{{"host":"{host}","port":{port},"sni":"{sni}","server_pub":"{server_pub}","psk":"{psk}"}}"#
        )
    }

    fn make_json(n: usize) -> String {
        let items: Vec<String> = (0..n)
            .map(|i| make_endpoint_json(&format!("gw{i}.example.com"), 443, "example.com", &valid_key(), &valid_key()))
            .collect();
        format!("[{}]", items.join(","))
    }

    #[test]
    fn valid_single_endpoint_parses() {
        let result = parse_endpoints(&make_json(1)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].port, 443);
    }

    #[test]
    fn valid_multiple_endpoints_parses() {
        let result = parse_endpoints(&make_json(5)).unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn empty_list_is_error() {
        assert!(matches!(parse_endpoints("[]").unwrap_err(), EndpointConfigError::EmptyList));
    }

    #[test]
    fn too_many_endpoints_is_error() {
        let err = parse_endpoints(&make_json(17)).unwrap_err();
        assert!(matches!(err, EndpointConfigError::TooManyEndpoints(17)));
    }

    #[test]
    fn exactly_max_endpoints_ok() {
        let result = parse_endpoints(&make_json(16)).unwrap();
        assert_eq!(result.len(), 16);
    }

    #[test]
    fn bad_hex_length_is_error() {
        let json = r#"[{"host":"h","port":1,"sni":"s","server_pub":"abcd","psk":"aaaa"}]"#;
        let err = parse_endpoints(json).unwrap_err();
        assert!(matches!(err, EndpointConfigError::WrongHexLength { field: "server_pub", expected: 64, got: 4, .. }));
    }

    #[test]
    fn non_hex_chars_is_error() {
        let bad_key = "z".repeat(64);
        let json = format!(r#"[{}]"#, make_endpoint_json("h", 1, "s", &bad_key, &valid_key()));
        let err = parse_endpoints(&json).unwrap_err();
        assert!(matches!(err, EndpointConfigError::InvalidHex { field: "server_pub", .. }));
    }

    #[test]
    fn malformed_json_is_error() {
        assert!(matches!(parse_endpoints("{not valid json").unwrap_err(), EndpointConfigError::MalformedJson(_)));
    }

    #[test]
    fn missing_field_is_malformed_json() {
        let json = r#"[{"host":"h","port":1,"sni":"s","psk":"aaaa"}]"#; // нет server_pub
        assert!(matches!(parse_endpoints(json).unwrap_err(), EndpointConfigError::MalformedJson(_)));
    }

    // ── Новые тесты по правкам ревизора ──

    #[test]
    fn zero_port_is_error() {
        let json = format!(r#"[{}]"#, make_endpoint_json("h", 0, "s", &valid_key(), &valid_key()));
        assert!(matches!(parse_endpoints(&json).unwrap_err(), EndpointConfigError::ZeroPort { index: 0 }));
    }

    #[test]
    fn port_over_u16_range_is_malformed_json() {
        // 70000 не влезает в u16 -> serde вернёт ошибку десериализации.
        let json = r#"[{"host":"h","port":70000,"sni":"s","server_pub":"aa","psk":"bb"}]"#;
        assert!(matches!(parse_endpoints(json).unwrap_err(), EndpointConfigError::MalformedJson(_)));
    }

    #[test]
    fn empty_host_is_error() {
        let json = format!(r#"[{}]"#, make_endpoint_json("", 443, "s", &valid_key(), &valid_key()));
        assert!(matches!(parse_endpoints(&json).unwrap_err(), EndpointConfigError::EmptyHost { index: 0 }));
    }

    #[test]
    fn empty_sni_is_error() {
        let json = format!(r#"[{}]"#, make_endpoint_json("h", 443, "", &valid_key(), &valid_key()));
        assert!(matches!(parse_endpoints(&json).unwrap_err(), EndpointConfigError::EmptySni { index: 0 }));
    }

    #[test]
    fn uppercase_hex_is_accepted() {
        let upper = "AA".repeat(32);
        let json = format!(r#"[{}]"#, make_endpoint_json("h", 443, "s", &upper, &upper));
        assert!(parse_endpoints(&json).is_ok(), "верхний регистр hex должен приниматься (зафиксированный контракт)");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let json = r#"[{"host":"h","port":1,"sni":"s","server_pub":"aa","psk":"bb","extra":"oops"}]"#;
        assert!(matches!(parse_endpoints(json).unwrap_err(), EndpointConfigError::MalformedJson(_)));
    }

    #[test]
    fn duplicate_endpoints_is_error() {
        let one = make_endpoint_json("gw.example.com", 443, "s", &valid_key(), &valid_key());
        let json = format!("[{one},{one}]");
        assert!(matches!(
            parse_endpoints(&json).unwrap_err(),
            EndpointConfigError::DuplicateEndpoint { index: 1, .. }
        ));
    }

    #[test]
    fn same_host_different_port_is_not_duplicate() {
        let a = make_endpoint_json("gw.example.com", 443, "s", &valid_key(), &valid_key());
        let b = make_endpoint_json("gw.example.com", 8443, "s", &valid_key(), &valid_key());
        let json = format!("[{a},{b}]");
        assert!(parse_endpoints(&json).is_ok());
    }

    #[test]
    fn hex_length_boundaries_63_and_65() {
        let short = "a".repeat(63);
        let long = "a".repeat(65);

        let json_short = format!(r#"[{}]"#, make_endpoint_json("h", 1, "s", &short, &valid_key()));
        let err = parse_endpoints(&json_short).unwrap_err();
        assert!(matches!(err, EndpointConfigError::WrongHexLength { field: "server_pub", expected: 64, got: 63, .. }));

        let json_long = format!(r#"[{}]"#, make_endpoint_json("h", 1, "s", &long, &valid_key()));
        let err = parse_endpoints(&json_long).unwrap_err();
        assert!(matches!(err, EndpointConfigError::WrongHexLength { field: "server_pub", expected: 64, got: 65, .. }));
    }

    #[test]
    fn json_object_instead_of_array_is_malformed() {
        let obj = format!(r#"{{"host":"h","port":1,"sni":"s","server_pub":"{}","psk":"{}"}}"#, valid_key(), valid_key());
        assert!(matches!(parse_endpoints(&obj).unwrap_err(), EndpointConfigError::MalformedJson(_)));
    }

    #[test]
    fn decode_hex32_roundtrip() {
        let hex = "00".repeat(31) + "ff";
        let decoded = decode_hex32(&hex).unwrap();
        assert_eq!(decoded[31], 0xff);
        assert_eq!(decoded[0], 0x00);
    }

    #[test]
    fn decode_hex32_wrong_length_is_none() {
        assert!(decode_hex32(&"a".repeat(63)).is_none());
        assert!(decode_hex32(&"a".repeat(65)).is_none());
    }

    #[test]
    fn decode_hex32_invalid_char_is_none() {
        let bad = "z".repeat(64);
        assert!(decode_hex32(&bad).is_none());
    }
}