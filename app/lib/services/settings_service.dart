// app/lib/services/settings_service.dart

import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import '../models/endpoint.dart';

/// Хранилище настроек транспорта: список gateway-endpoint'ов, shared secret,
/// SOCKS-порт. Инкапсулирует миграцию legacy-однострочного формата в
/// multi-endpoint формат.
///
/// Ревизор A6: `endpoints` был публичным изменяемым полем, из-за чего UI (A7)
/// писал в него напрямую, минуя addEndpoint/removeEndpointAt/reorderEndpoint —
/// эти методы фактически не использовались. Теперь список приватный и доступен
/// только на чтение (`endpoints` — unmodifiable view), мутация — только через
/// методы этого класса, которые сами вызывают persist().
class SettingsService extends ChangeNotifier {
  static const _keyEndpoints = 'gateway_endpoints_v2';
  static const _keySocksPort = 'socks_port';
  static const _keySharedSecret = 'shared_secret';

  // Legacy (однострочный формат v0.1.0, до multi-endpoint).
  // Ключи соответствуют реальным именам из SettingsService v0.1.0:
  //   _keyGatewayHost='gateway_host', _keyGatewayPort='gateway_port',
  //   _keySniList='sni_list', _keyServerPublic='server_public',
  //   _keyClientPsk='client_psk', _keySharedSecret='shared_secret'.
  // shared_secret — ключ не изменился, грузится напрямую в load().
  static const _legacyHost = 'gateway_host';
  static const _legacyPort = 'gateway_port';
  static const _legacySni = 'sni_list';
  static const _legacyPub = 'server_public';
  static const _legacyPsk = 'client_psk';

  List<GatewayEndpoint> _endpoints = [];
  int _socksPort = 18080;
  String _sharedSecret = '';
  bool _loaded = false;

  /// Список endpoint'ов только для чтения. Мутация — через addEndpoint/
  /// removeEndpointAt/reorderEndpoint/replaceEndpoint/setEndpoints.
  List<GatewayEndpoint> get endpoints => List.unmodifiable(_endpoints);

  int get socksPort => _socksPort;
  String get sharedSecret => _sharedSecret;
  bool get isLoaded => _loaded;

  /// Транспорт считается настроенным, если есть хотя бы один валидный
  /// endpoint и sharedSecret — ровно 64 hex-символа (используем единую
  /// GatewayEndpoint.isHex64, устраняя дублирование проверки "только по
  /// длине" в A5/A6/A7, отмеченное ревизором как сквозная проблема).
  ///
  /// Примечание: sharedSecret на текущем этапе НЕ участвует в шифровании на
  /// Rust-стороне (см. A3/A4 — session key строится на steal-handshake,
  /// server_pub/psk берутся из endpoint). Поле сохранено как зарезервированное
  /// в frozen FFI-контракте; это явно задокументировано здесь и в A5/A7, чтобы
  /// не вводить пользователя в заблуждение относительно его роли.
  bool get isConfigured =>
      _endpoints.isNotEmpty &&
      _endpoints.every((e) => e.isValid) &&
      GatewayEndpoint.isHex64(_sharedSecret);

  /// Загружает настройки из SharedPreferences. Выполняет миграцию legacy
  /// однострочного формата при первом запуске после обновления.
  ///
  /// Fail-safe вместо fail-crash (Ревизор A6, критично): битый JSON в
  /// _keyEndpoints раньше ронял jsonDecode без перехвата, из-за чего load()
  /// пробрасывал исключение и приложение не стартовало. Теперь любая ошибка
  /// парсинга откатывается на пустой список endpoints — пользователь увидит
  /// экран настроек с пустым списком вместо краша.
  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();

    _socksPort = _clampPort(prefs.getInt(_keySocksPort) ?? 18080);
    _sharedSecret = prefs.getString(_keySharedSecret) ?? '';

    final rawEndpoints = prefs.getString(_keyEndpoints);
    if (rawEndpoints != null) {
      _endpoints = _decodeEndpointsListSafe(rawEndpoints);
    } else {
      // Нет multi-endpoint ключа — пробуем мигрировать legacy-формат.
      await _migrateLegacyIfPresent(prefs);
    }

    _loaded = true;
    notifyListeners();
  }

  /// Безопасный декод списка endpoint'ов. Любая ошибка (битый JSON, не-массив,
  /// не-объект внутри массива) приводит к пустому списку, а не к исключению.
  /// Отдельно логируются невалидные (по isValid) элементы, а не молча теряются
  /// (Ревизор A6: "мигрированный невалидный endpoint без диагностики").
  List<GatewayEndpoint> _decodeEndpointsListSafe(String raw) {
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! List) {
        debugPrint('[Settings] $_keyEndpoints is not a JSON array, resetting to empty list');
        return [];
      }
      final result = <GatewayEndpoint>[];
      for (final item in decoded) {
        if (item is! Map<String, dynamic>) {
          debugPrint('[Settings] skipping non-object endpoint entry: $item');
          continue;
        }
        final ep = GatewayEndpoint.fromJson(item);
        if (!ep.isValid) {
          debugPrint('[Settings] loaded endpoint failed validation (host=${ep.host}, port=${ep.port}) — kept in list but will not satisfy isConfigured');
        }
        result.add(ep);
      }
      return result;
    } catch (e) {
      debugPrint('[Settings] failed to decode $_keyEndpoints: $e — falling back to empty list (fail-safe)');
      return [];
    }
  }

  Future<void> _migrateLegacyIfPresent(SharedPreferences prefs) async {
    final legacyHost = prefs.getString(_legacyHost) ?? '';
    if (legacyHost.isEmpty) {
      _endpoints = [];
      return;
    }

    final migrated = GatewayEndpoint(
      host: legacyHost,
      port: _clampPort(prefs.getInt(_legacyPort) ?? 443),
      sni: prefs.getString(_legacySni) ?? '',
      serverPub: prefs.getString(_legacyPub) ?? '',
      psk: prefs.getString(_legacyPsk) ?? '',
    );

    if (!migrated.isValid) {
      debugPrint('[Settings] legacy config present but failed validation after migration '
          '(host="${migrated.host}", port=${migrated.port}) — resulting endpoint kept, '
          'will not satisfy isConfigured until fixed in Settings');
    }

    _endpoints = [migrated];
    await _persistEndpoints();

    // Удаляем legacy-ключи ПОСЛЕ успешной записи нового формата. Не
    // транзакционно (если процесс убьют между этими двумя шагами, legacy-ключи
    // останутся навсегда как безвредный мусор, а следующий load() увидит уже
    // существующий _keyEndpoints и не повторит миграцию — идемпотентно).
    await prefs.remove(_legacyHost);
    await prefs.remove(_legacyPort);
    await prefs.remove(_legacySni);
    await prefs.remove(_legacyPub);
    await prefs.remove(_legacyPsk);
  }

  Future<void> _persistEndpoints() async {
    final prefs = await SharedPreferences.getInstance();
    final json = jsonEncode(_endpoints.map((e) => e.toJson()).toList());
    await prefs.setString(_keyEndpoints, json);
  }

  Future<void> setSocksPort(int port) async {
    _socksPort = _clampPort(port);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_keySocksPort, _socksPort);
    notifyListeners();
  }

  Future<void> setSharedSecret(String secret) async {
    _sharedSecret = secret;
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keySharedSecret, secret);
    notifyListeners();
  }

  /// Валидирует и приводит порт к допустимому диапазону 1..65535. Значения
  /// вне диапазона (включая 0) откатываются на дефолт 18080, а не молча
  /// перезаписывают предыдущее значение (Ревизор A6: "нет валидации socksPort").
  int _clampPort(int port) {
    if (port < 1 || port > 65535) {
      debugPrint('[Settings] socksPort $port out of range 1..65535, falling back to 18080');
      return 18080;
    }
    return port;
  }

  /// Добавляет endpoint. Отклоняет дубликаты по host:port (Ревизор A1/A6:
  /// "нет проверки на дубликаты при addEndpoint") — возвращает false, ничего
  /// не сохраняет, если такой host:port уже есть в списке.
  Future<bool> addEndpoint(GatewayEndpoint endpoint) async {
    final isDuplicate = _endpoints.any((e) => e.hostPortKey == endpoint.hostPortKey);
    if (isDuplicate) {
      debugPrint('[Settings] addEndpoint rejected: duplicate host:port ${endpoint.hostPortKey}');
      return false;
    }
    _endpoints = [..._endpoints, endpoint];
    await _persistEndpoints();
    notifyListeners();
    return true;
  }

  /// Заменяет endpoint по стабильному идентификатору (значению), а не по
  /// индексу — устраняет класс "stale index" багов, которые в A7 возникали
  /// из-за манипуляции списком по позиции после reorder/dismiss.
  Future<bool> replaceEndpoint(GatewayEndpoint oldValue, GatewayEndpoint newValue) async {
    final idx = _endpoints.indexOf(oldValue);
    if (idx == -1) return false;
    final isDuplicateElsewhere = _endpoints
        .asMap()
        .entries
        .any((entry) => entry.key != idx && entry.value.hostPortKey == newValue.hostPortKey);
    if (isDuplicateElsewhere) {
      debugPrint('[Settings] replaceEndpoint rejected: duplicate host:port ${newValue.hostPortKey}');
      return false;
    }
    final next = List<GatewayEndpoint>.from(_endpoints);
    next[idx] = newValue;
    _endpoints = next;
    await _persistEndpoints();
    notifyListeners();
    return true;
  }

  /// Удаляет endpoint по значению (стабильно относительно reorder), а не по
  /// индексу. Вызывающая сторона (A7) должна передавать сам объект endpoint,
  /// захваченный на момент действия пользователя, а не индекс в списке.
  Future<void> removeEndpoint(GatewayEndpoint endpoint) async {
    _endpoints = _endpoints.where((e) => e != endpoint).toList();
    await _persistEndpoints();
    notifyListeners();
  }

  /// Переупорядочивает список, идентифицируя перемещаемый элемент по значению.
  /// `oldIndex`/`newIndex` приходят напрямую из ReorderableListView.onReorder
  /// (Flutter API работает с индексами в момент вызова, до применения этого
  /// метода список ещё не менялся, так что индексы корректны на входе).
  Future<void> reorderEndpoint(int oldIndex, int newIndex) async {
    if (oldIndex < 0 || oldIndex >= _endpoints.length) return;
    var target = newIndex;
    if (target > oldIndex) target -= 1;
    target = target.clamp(0, _endpoints.length - 1);

    final next = List<GatewayEndpoint>.from(_endpoints);
    final item = next.removeAt(oldIndex);
    next.insert(target, item);
    _endpoints = next;
    await _persistEndpoints();
    notifyListeners();
  }

  /// Полная замена списка endpoint'ов одним вызовом (например, импорт
  /// конфигурации). Отклоняет список, содержащий дубликаты host:port.
  Future<bool> setEndpoints(List<GatewayEndpoint> newEndpoints) async {
    final keys = newEndpoints.map((e) => e.hostPortKey).toSet();
    if (keys.length != newEndpoints.length) {
      debugPrint('[Settings] setEndpoints rejected: list contains duplicate host:port entries');
      return false;
    }
    _endpoints = List.unmodifiable(newEndpoints);
    await _persistEndpoints();
    notifyListeners();
    return true;
  }

  // ── Legacy-совместимые геттеры (для миграции/обратной совместимости UI) ──
  //
  // Ревизор A6, критично: раньше при пустом списке возвращали зашитый тестовый
  // PSK ('ABCDEF...'), который "раздавался" любому коду, дёрнувшему геттер, что
  // маскировало отсутствие конфигурации. Теперь при пустом списке — безопасные
  // пустые значения; isValid/isConfigured корректно отсекут неготовую конфигурацию.
  String get legacyHost => _endpoints.isNotEmpty ? _endpoints.first.host : '';
  int get legacyPort => _endpoints.isNotEmpty ? _endpoints.first.port : 0;
  String get legacySni => _endpoints.isNotEmpty ? _endpoints.first.sni : '';
  String get legacyServerPub => _endpoints.isNotEmpty ? _endpoints.first.serverPub : '';
  String get legacyClientPsk => _endpoints.isNotEmpty ? _endpoints.first.psk : '';
}