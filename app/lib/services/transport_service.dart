import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart';
import 'settings_service.dart';
import 'subscription_service.dart';

// ── FFI bindings ─────────────────────────────────────────────────────────────

// int32_t transport_start(
//   uint16_t socks_port,
//   const char* gateway_host,
//   uint16_t    gateway_port,
//   const uint8_t* key_bytes,      // 32 bytes
//   const char* sni_list,           // comma-separated SNI pool
//   const uint8_t* server_pub,      // 32 bytes X25519 public key
//   uint64_t rate_limit_bps         // bytes per second (0 = no limit)
// )
typedef _TransportStartNative = Int32 Function(
  Uint16 socksPort,
  Pointer<Utf8> gatewayHost,
  Uint16 gatewayPort,
  Pointer<Uint8> keyBytes,
  Pointer<Utf8> sniList,
  Pointer<Uint8> serverPub,
  Uint64 rateLimitBps,
);
typedef _TransportStartDart = int Function(
  int socksPort,
  Pointer<Utf8> gatewayHost,
  int gatewayPort,
  Pointer<Uint8> keyBytes,
  Pointer<Utf8> sniList,
  Pointer<Uint8> serverPub,
  int rateLimitBps,
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
  final SubscriptionService? _subscriptionService;

  TransportState _state = TransportState.idle;
  String _errorMessage = '';
  String _version = '';

  TransportService(this._settings, [this._subscriptionService]);

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
    debugPrint('[Transport] start() called, state=$_state, isConfigured=${_settings.isConfigured}');
    if (_state == TransportState.running) return;
    if (!_settings.isConfigured) {
      debugPrint('[Transport] Not configured: host=${_settings.gatewayHost}, secret_len=${_settings.sharedSecret.length}, pub_len=${_settings.serverPublic.length}');
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
      final serverPubBytes = _hexToBytes(_settings.serverPublic);
      final serverPubPtr   = malloc.allocate<Uint8>(32);
      final hostPtr = _settings.gatewayHost.toNativeUtf8();
      final sniPtr  = _settings.sniList.toNativeUtf8();
      
      try {
        for (var i = 0; i < 32; i++) {
          keyPtr[i] = keyBytes[i];
        }
        for (var i = 0; i < 32; i++) {
          serverPubPtr[i] = serverPubBytes[i];
        }

        // Получаем rate limit из подписки
        final rateLimitBps = _subscriptionService?.getRateLimitBytesPerSecond() ?? (3 * 1024 * 1024);
        
        debugPrint('[Transport] Calling transport_start: port=${_settings.socksPort}, host=${_settings.gatewayHost}:${_settings.gatewayPort}, rate_limit=$rateLimitBps bps');
        final result  = fn(
          _settings.socksPort,
          hostPtr,
          _settings.gatewayPort,
          keyPtr,
          sniPtr,
          serverPubPtr,
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
      } finally {
        // Освобождаем память ВСЕГДА, даже если fn() бросил исключение
        malloc.free(hostPtr);
        malloc.free(keyPtr);
        malloc.free(sniPtr);
        malloc.free(serverPubPtr);
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
