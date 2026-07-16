// app/test/services/transport_service_test.dart

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:isolated_browser/models/endpoint.dart';
import 'package:isolated_browser/services/settings_service.dart';
import 'package:isolated_browser/services/transport_service.dart';

GatewayEndpoint _validEndpoint() => GatewayEndpoint(
      host: 'gw.example.com',
      port: 443,
      sni: 'cloudflare.com',
      serverPub: 'a' * 64,
      psk: 'b' * 64,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('TransportService.start() guard clauses', () {
    test('start() with empty endpoints stays idle, does not attempt FFI call', () async {
      final settings = SettingsService();
      await settings.load();
      await settings.setSharedSecret('a' * 64);
      await settings.setSocksPort(18080);

      final service = TransportService(settings);
      await service.start();

      expect(service.state, TransportState.idle);
      expect(service.errorMessage, contains('not configured'));
    });

    test('start() with invalid shared secret length is rejected before any native allocation', () async {
      final settings = SettingsService();
      await settings.load();
      await settings.setEndpoints([_validEndpoint()]);
      await settings.setSharedSecret('short');
      await settings.setSocksPort(18080);

      final service = TransportService(settings);
      await service.start();

      expect(service.state, TransportState.idle);
    });

    test('state remains idle until start() is called', () {
      final settings = SettingsService();
      final service = TransportService(settings);
      expect(service.state, TransportState.idle);
      expect(service.isRunning, isFalse);
    });
  });
}