import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import 'settings_service.dart';

// ── FFI bindings ─────────────────────────────────────────────────────────────

// int32_t transport_start(
//   uint16_t socks_port,
//   const char* gateway_host,
//   uint16_t    gateway_port,
//   const uint8_t* key_bytes   // 32 bytes
// )
typedef _TransportStartNative = Int32 Function(
  Uint16 socksPort,
  Pointer<Utf8> gatewayHost,
  Uint16 gatewayPort,
  Pointer<Uint8> keyBytes,
);
typedef _TransportStartDart = int Function(
  int socksPort,
  Pointer<Utf8> gatewayHost,
  int gatewayPort,
  Pointer<Uint8> keyBytes,
);

// const char* transport_version()
typedef _TransportVersionNative = Pointer<Utf8> Function();
typedef _TransportVersionDart   = Pointer<Utf8> Function();

// int32_t transport_stop()
typedef _TransportStopNative = Int32 Function();
typedef _TransportStopDart   = int Function();

// ── Service ──────────────────────────────────────────────────────────────────

enum TransportState { idle, starting, running, error }

class TransportService extends ChangeNotifier {
  final SettingsService _settings;

  TransportState _state = TransportState.idle;
  String _errorMessage = '';
  String _version = '';

  TransportService(this._settings);

  TransportState get state        => _state;
  String         get errorMessage => _errorMessage;
  String         get version      => _version;
  bool           get isRunning    => _state == TransportState.running;

  /// Загружает нативную библиотеку и возвращает её.
  DynamicLibrary _loadLib() {
    if (Platform.isAndroid) {
      return DynamicLibrary.open('libtransport_core.so');
    } else if (Platform.isWindows) {
      return DynamicLibrary.open('transport_core.dll');
    } else if (Platform.isIOS || Platform.isMacOS) {
      return DynamicLibrary.process();
    }
    throw UnsupportedError('Platform not supported: ${Platform.operatingSystem}');
  }

  /// Читает версию транспортного модуля.
  String loadVersion() {
    try {
      final lib = _loadLib();
      final fn  = lib.lookupFunction<_TransportVersionNative, _TransportVersionDart>(
        'transport_version',
      );
      _version = fn().toDartString();
      return _version;
    } catch (e) {
      return 'unknown';
    }
  }

  /// Запускает SOCKS5-прокси + транспортный туннель.
  Future<void> start() async {
    if (_state == TransportState.running) return;
    if (!_settings.isConfigured) {
      _state = TransportState.idle;
      _errorMessage = 'Gateway not configured. Open Settings.';
      notifyListeners();
      return;
    }

    _state = TransportState.starting;
    notifyListeners();

    try {
      final lib = _loadLib();
      final fn  = lib.lookupFunction<_TransportStartNative, _TransportStartDart>(
        'transport_start',
      );

      // Декодируем hex-ключ в байты
      final keyBytes = _hexToBytes(_settings.sharedSecret);
      final keyPtr   = malloc.allocate<Uint8>(32);
      for (var i = 0; i < 32; i++) {
        keyPtr[i] = keyBytes[i];
      }

      final hostPtr = _settings.gatewayHost.toNativeUtf8();
      final result  = fn(
        _settings.socksPort,
        hostPtr,
        _settings.gatewayPort,
        keyPtr,
      );

      malloc.free(hostPtr);
      malloc.free(keyPtr);

      if (result == 0) {
        _state = TransportState.running;
        _errorMessage = '';
      } else {
        _state = TransportState.error;
        _errorMessage = 'transport_start returned $result';
      }
    } catch (e) {
      _state = TransportState.error;
      _errorMessage = e.toString();
    }

    notifyListeners();
  }

  /// Останавливает транспортный модуль.
  Future<void> stop() async {
    if (_state != TransportState.running) return;

    _state = TransportState.idle;
    notifyListeners();

    try {
      final lib = _loadLib();
      final fn = lib.lookupFunction<_TransportStopNative, _TransportStopDart>(
        'transport_stop',
      );
      final result = fn();
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

  /// Конвертирует hex-строку (64 символа) в List<int> (32 байта).
  static List<int> _hexToBytes(String hex) {
    final result = <int>[];
    for (var i = 0; i < hex.length; i += 2) {
      result.add(int.parse(hex.substring(i, i + 2), radix: 16));
    }
    return result;
  }
}
