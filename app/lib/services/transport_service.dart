// app/lib/services/transport_service.dart

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import '../models/endpoint.dart';
import 'settings_service.dart';
import 'subscription_service.dart';

// ── FFI bindings (контракт из A4, frozen G-Contract) ────
typedef _TransportStartNative = Int32 Function(
  Uint16 socksPort,
  Pointer<Utf8> endpointsJson,
  Pointer<Uint8> keyBytes,
  Uint64 rateLimitBps,
);
typedef _TransportStartDart = int Function(
  int socksPort,
  Pointer<Utf8> endpointsJson,
  Pointer<Uint8> keyBytes,
  int rateLimitBps,
);

typedef _TransportVersionNative = Pointer<Utf8> Function();
typedef _TransportVersionDart = Pointer<Utf8> Function();

typedef _TransportStopNative = Int32 Function();
typedef _TransportStopDart = int Function();

typedef _TransportGetStatusNative = Pointer<Utf8> Function();
typedef _TransportGetStatusDart = Pointer<Utf8> Function();

typedef _TransportFreeStringNative = Void Function(Pointer<Utf8>);
typedef _TransportFreeStringDart = void Function(Pointer<Utf8>);

enum TransportState { idle, starting, running, error }

/// Результат безопасного hex-декодирования — либо валидные 32 байта, либо
/// явная причина ошибки (не exception), т.к. вызывающая сторона (start())
/// уже успела аллоцировать endpointsPtr до этой проверки в старой версии,
/// что приводило к утечке при исключении (Ревизор A5).
class _HexDecodeResult {
  final List<int>? bytes;
  final String? error;
  const _HexDecodeResult.ok(this.bytes) : error = null;
  const _HexDecodeResult.err(this.error) : bytes = null;
}

class TransportService extends ChangeNotifier {
  final SettingsService _settings;
  final SubscriptionService? _subscriptionService;

  TransportState _state = TransportState.idle;
  String _errorMessage = '';
  String _version = '';

  // Кэш DynamicLibrary + resolved-функций — устраняет пересборку lookupFunction
  // на каждый вызов getStatus()/start()/stop() (Ревизор A5: "дорого при поллинге,
  // потенциальная утечка хендлов").
  DynamicLibrary? _lib;
  _TransportStartDart? _fnStart;
  _TransportVersionDart? _fnVersion;
  _TransportStopDart? _fnStop;
  _TransportGetStatusDart? _fnGetStatus;
  _TransportFreeStringDart? _fnFreeString;

  TransportService(this._settings, [this._subscriptionService]);

  TransportState get state => _state;
  String get errorMessage => _errorMessage;
  String get version => _version;
  bool get isRunning => _state == TransportState.running;

  DynamicLibrary _loadLibOnce() {
    final cached = _lib;
    if (cached != null) return cached;

    final DynamicLibrary lib;
    if (Platform.isAndroid) {
      lib = DynamicLibrary.open('libtransport_core.so');
    } else if (Platform.isWindows) {
      lib = DynamicLibrary.open('transport_core.dll');
    } else if (Platform.isIOS || Platform.isMacOS) {
      lib = DynamicLibrary.process();
    } else {
      throw UnsupportedError('Platform not supported: ${Platform.operatingSystem}');
    }
    _lib = lib;
    return lib;
  }

  _TransportStartDart get _start => _fnStart ??=
      _loadLibOnce().lookupFunction<_TransportStartNative, _TransportStartDart>('transport_start');
  _TransportVersionDart get _versionFn => _fnVersion ??=
      _loadLibOnce().lookupFunction<_TransportVersionNative, _TransportVersionDart>('transport_version');
  _TransportStopDart get _stop => _fnStop ??=
      _loadLibOnce().lookupFunction<_TransportStopNative, _TransportStopDart>('transport_stop');
  _TransportGetStatusDart get _getStatus => _fnGetStatus ??= _loadLibOnce()
      .lookupFunction<_TransportGetStatusNative, _TransportGetStatusDart>('transport_get_status');
  _TransportFreeStringDart get _freeString => _fnFreeString ??= _loadLibOnce()
      .lookupFunction<_TransportFreeStringNative, _TransportFreeStringDart>('transport_free_string');

  String loadVersion() {
    try {
      _version = _versionFn().toDartString();
      return _version;
    } catch (e) {
      // Ревизор A5: старая версия не сбрасывала _version при ошибке, оставляя
      // расхождение между return value и полем. Здесь синхронизируем оба.
      _version = 'unknown';
      return _version;
    }
  }

  /// Безопасное hex→bytes: не бросает исключение, возвращает явный результат.
  /// Проверяет чётность длины и итоговый размер == 32 байта (Ревизор A5:
  /// "RangeError при более короткой строке, инвариант держится только на
  /// isConfigured — ненадёжно, если start() вызван в обход").
  static _HexDecodeResult _hexToBytesSafe(String hex) {
    if (hex.length != 64) {
      return _HexDecodeResult.err('shared secret must be exactly 64 hex characters, got ${hex.length}');
    }
    final result = <int>[];
    for (var i = 0; i < hex.length; i += 2) {
      final byteStr = hex.substring(i, i + 2);
      final byte = int.tryParse(byteStr, radix: 16);
      if (byte == null) {
        return _HexDecodeResult.err('invalid hex byte "$byteStr" at offset $i');
      }
      result.add(byte);
    }
    if (result.length != 32) {
      return _HexDecodeResult.err('decoded length ${result.length} != 32');
    }
    return _HexDecodeResult.ok(result);
  }

  /// Запускает SOCKS5-прокси + транспортный туннель с multi-endpoint failover.
  Future<void> start() async {
    debugPrint('[Transport] start() called, state=$_state, isConfigured=${_settings.isConfigured}');
    if (_state == TransportState.running) return;

    if (!_settings.isConfigured) {
      debugPrint('[Transport] Not configured: endpoints=${_settings.endpoints.length}');
      _state = TransportState.idle;
      _errorMessage = 'Gateway not configured. Open Settings.';
      notifyListeners();
      return;
    }

    _state = TransportState.starting;
    notifyListeners();

    // Декодируем секрет ДО любой нативной аллокации — устраняет утечку
    // endpointsPtr при ошибке _hexToBytes (Ревизор A5, критично).
    final decoded = _hexToBytesSafe(_settings.sharedSecret);
    if (decoded.error != null) {
      _state = TransportState.error;
      _errorMessage = 'Invalid shared secret: ${decoded.error}';
      debugPrint('[Transport] $_errorMessage');
      notifyListeners();
      return;
    }
    final keyBytes = decoded.bytes!;

    Pointer<Utf8>? endpointsPtr;
    Pointer<Uint8>? keyPtr;
    try {
      final endpointsJsonStr = jsonEncode(
        _settings.endpoints.map((e) => e.toJson()).toList(),
      );
      endpointsPtr = endpointsJsonStr.toNativeUtf8();
      keyPtr = malloc.allocate<Uint8>(32);
      for (var i = 0; i < 32; i++) {
        keyPtr[i] = keyBytes[i];
      }

      final rateLimitBps = _subscriptionService?.getRateLimitBytesPerSecond() ?? (3 * 1024 * 1024);

      debugPrint('[Transport] Calling transport_start: port=${_settings.socksPort}, '
          'endpoints=${_settings.endpoints.length}, rate_limit=$rateLimitBps bps');

      final result = _start(
        _settings.socksPort,
        endpointsPtr,
        keyPtr,
        rateLimitBps,
      );

      debugPrint('[Transport] transport_start returned: $result');

      if (result == 0) {
        _state = TransportState.running;
        _errorMessage = '';
        debugPrint('[Transport] State -> running');
      } else {
        _state = TransportState.error;
        _errorMessage = 'transport_start returned $result';
        debugPrint('[Transport] State -> error: $_errorMessage');
      }
    } catch (e) {
      _state = TransportState.error;
      _errorMessage = e.toString();
    } finally {
      if (endpointsPtr != null) malloc.free(endpointsPtr);
      if (keyPtr != null) malloc.free(keyPtr);
    }
    notifyListeners();
  }

  /// Возвращает текущий статус failover (endpoint, failures, last_switch) как Map,
  /// либо null если транспорт не запущен или вызов не удался. Не открывает
  /// DynamicLibrary заново (кэшировано в _loadLibOnce/lazy-геттерах выше).
  Map<String, dynamic>? getStatus() {
    try {
      final ptr = _getStatus();
      if (ptr.address == 0) return null;
      try {
        final jsonStr = ptr.toDartString();
        final decoded = jsonDecode(jsonStr);
        if (decoded is Map<String, dynamic>) return decoded;
        return null;
      } finally {
        _freeString(ptr);
      }
    } catch (e) {
      debugPrint('[Transport] getStatus() failed: $e');
      return null;
    }
  }

  Future<void> stop() async {
    if (_state != TransportState.running) return;
    try {
      final result = _stop();
      if (result == 0) {
        _state = TransportState.idle;
        _errorMessage = '';
      } else {
        _state = TransportState.error;
        _errorMessage = 'transport_stop returned $result';
      }
    } catch (e) {
      _state = TransportState.error;
      _errorMessage = e.toString();
    }
    notifyListeners();
  }
}