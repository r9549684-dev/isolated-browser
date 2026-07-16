// app/lib/models/endpoint.dart

/// Модель одного gateway-endpoint для multi-gateway failover (AMO_SPEC).
/// Отражает Rust EndpointConfig 1:1 (host, port, sni, server_pub hex64, psk hex64).
class GatewayEndpoint {
  final String host;
  final int port;
  final String sni;
  final String serverPub; // hex-encoded 32 bytes X25519 public key
  final String psk;       // hex-encoded 32 bytes pre-shared key

  static final RegExp _hex64 = RegExp(r'^[0-9a-fA-F]{64}$');

  const GatewayEndpoint({
    required this.host,
    required this.port,
    required this.sni,
    required this.serverPub,
    required this.psk,
  });

  Map<String, dynamic> toJson() => {
        'host': host,
        'port': port,
        'sni': sni,
        'server_pub': serverPub,
        'psk': psk,
      };

  /// Безопасный парсинг из JSON. Не бросает исключение на неверные типы —
  /// возвращает endpoint с "пустыми"/дефолтными полями, который затем
  /// провалит isValid и будет отфильтрован/показан как ошибка выше по стеку
  /// (SettingsService.load(), см. A6), вместо падения всего load() целиком
  /// (Ревизор A5: "as String/as int бросит на double/String/отсутствующих ключах").
  factory GatewayEndpoint.fromJson(Map<String, dynamic> json) {
    return GatewayEndpoint(
      host: _asString(json['host']),
      port: _asPort(json['port']),
      sni: _asString(json['sni']),
      serverPub: _asString(json['server_pub']),
      psk: _asString(json['psk']),
    );
  }

  static String _asString(dynamic v) {
    if (v is String) return v;
    return '';
  }

  /// Порт может прийти как int, как double (частый случай при jsonDecode
  /// ручного JSON) или как строка при ручном редактировании файла настроек.
  static int _asPort(dynamic v) {
    if (v is int) return v;
    if (v is double) return v.toInt();
    if (v is String) return int.tryParse(v) ?? 0;
    return 0;
  }

  /// Проверка hex-строки: ровно 64 hex-символа (32 байта). Единая функция —
  /// используется здесь, а также должна использоваться в A6/A7 вместо
  /// повторяющихся проверок только по length (Ревизор: "валидация hex только
  /// по длине повторяется в трёх местах").
  static bool isHex64(String s) => _hex64.hasMatch(s);

  /// Полная валидация: непустой host, port в диапазоне 1..65535, непустой sni,
  /// server_pub/psk — строго 64 hex-символа (не просто length == 64).
  bool get isValid =>
      host.isNotEmpty &&
      host.trim() == host &&
      port >= 1 &&
      port <= 65535 &&
      sni.isNotEmpty &&
      isHex64(serverPub) &&
      isHex64(psk);

  GatewayEndpoint copyWith({String? host, int? port, String? sni, String? serverPub, String? psk}) =>
      GatewayEndpoint(
        host: host ?? this.host,
        port: port ?? this.port,
        sni: sni ?? this.sni,
        serverPub: serverPub ?? this.serverPub,
        psk: psk ?? this.psk,
      );

  /// Равенство по значению — нужно для дедупликации дубликатов host:port (A1
  /// проверяет это на Rust-стороне, здесь дублируем для UI до отправки в FFI)
  /// и для стабильных ключей ReorderableListView/Dismissible в A7 (устраняет
  /// "stale index"/"duplicate keys" баги из ревизии A7).
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is GatewayEndpoint &&
          host == other.host &&
          port == other.port &&
          sni == other.sni &&
          serverPub == other.serverPub &&
          psk == other.psk;

  @override
  int get hashCode => Object.hash(host, port, sni, serverPub, psk);

  /// Ключ вида "host:port" — используется для проверки дубликатов в списке
  /// (независимо от sni/serverPub/psk, т.к. AMO_SPEC запрещает дубликаты именно
  /// по host:port на Rust-стороне, A1).
  String get hostPortKey => '$host:$port';
}