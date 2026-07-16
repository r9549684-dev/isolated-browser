// app/test/services/settings_service_test.dart

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:isolated_browser/models/endpoint.dart';
import 'package:isolated_browser/services/settings_service.dart';

GatewayEndpoint _ep({String host = 'gw.example.com', int port = 443}) => GatewayEndpoint(
      host: host,
      port: port,
      sni: 'cloudflare.com',
      serverPub: 'a' * 64,
      psk: 'b' * 64,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('migration from legacy format', () {
    test('legacy keys present -> endpoints[0] built correctly, legacy keys removed', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_host': 'legacy.example.com',
        'gateway_port': 8443,
        'sni_list': 'cloudflare.com',
        'server_public': 'c' * 64,
        'client_psk': 'd' * 64,
      });

      final service = SettingsService();
      await service.load();

      expect(service.endpoints.length, 1);
      expect(service.endpoints.first.host, 'legacy.example.com');
      expect(service.endpoints.first.port, 8443);

      final prefs = await SharedPreferences.getInstance();
      expect(prefs.getString('gateway_host'), isNull);
      expect(prefs.getString('client_psk'), isNull);
      expect(prefs.getString('gateway_endpoints_v2'), isNotNull);
    });

    test('empty legacyHost -> endpoints == [], nothing written', () async {
      SharedPreferences.setMockInitialValues({});
      final service = SettingsService();
      await service.load();
      expect(service.endpoints, isEmpty);

      final prefs = await SharedPreferences.getInstance();
      expect(prefs.getString('gateway_endpoints_v2'), isNull);
    });

    test('idempotent: second load() after migration reads multi-format, does not re-migrate', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_host': 'legacy.example.com',
        'gateway_port': 8443,
        'sni_list': 'sni.example.com',
        'server_public': 'c' * 64,
        'client_psk': 'd' * 64,
      });
      final service = SettingsService();
      await service.load();
      final firstEndpoints = service.endpoints;

      final service2 = SettingsService();
      await service2.load();

      expect(service2.endpoints.length, firstEndpoints.length);
      expect(service2.endpoints.first.host, 'legacy.example.com');
    });

    test('regression: actual v0.1.0 key names (gateway_host/gateway_port/sni_list) migrate correctly', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_host': '38.180.253.219',
        'gateway_port': 9443,
        'sni_list': 'cloudflare.com,google.com',
        'server_public': 'ddc2127c2fb7d7e0222073da0a7390ff594ecc769b664b706631f88c7e76d80a',
        'client_psk': '6cafcbd72f18f83be37fbd44b8d340cbbc0809ce4b974482dd2908f554f0cbb5',
        'shared_secret': '6cafcbd72f18f83be37fbd44b8d340cbbc0809ce4b974482dd2908f554f0cbb5',
      });

      final service = SettingsService();
      await service.load();

      expect(service.endpoints.length, 1);
      expect(service.endpoints.first.host, '38.180.253.219');
      expect(service.endpoints.first.port, 9443);
      expect(service.endpoints.first.sni, 'cloudflare.com,google.com');
      expect(service.endpoints.first.serverPub, 'ddc2127c2fb7d7e0222073da0a7390ff594ecc769b664b706631f88c7e76d80a');
      expect(service.endpoints.first.psk, '6cafcbd72f18f83be37fbd44b8d340cbbc0809ce4b974482dd2908f554f0cbb5');
      expect(service.sharedSecret, '6cafcbd72f18f83be37fbd44b8d340cbbc0809ce4b974482dd2908f554f0cbb5');
      expect(service.isConfigured, isTrue);
    });
  });

  group('fail-safe JSON handling', () {
    test('malformed _keyEndpoints JSON -> empty list, no exception', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_endpoints_v2': '{not valid json',
      });
      final service = SettingsService();
      await expectLater(service.load(), completes);
      expect(service.endpoints, isEmpty);
    });

    test('_keyEndpoints as JSON object instead of array -> empty list', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_endpoints_v2': '{"host":"x"}',
      });
      final service = SettingsService();
      await service.load();
      expect(service.endpoints, isEmpty);
    });

    test('array containing non-object entries is skipped, valid entries kept', () async {
      SharedPreferences.setMockInitialValues({
        'gateway_endpoints_v2': '[123, {"host":"gw.example.com","port":443,"sni":"s","server_pub":"${'a' * 64}","psk":"${'b' * 64}"}]',
      });
      final service = SettingsService();
      await service.load();
      expect(service.endpoints.length, 1);
      expect(service.endpoints.first.host, 'gw.example.com');
    });
  });

  group('save/load round-trip', () {
    test('N endpoints persist and reload correctly', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep(host: 'a.com', port: 1));
      await service.addEndpoint(_ep(host: 'b.com', port: 2));
      await service.addEndpoint(_ep(host: 'c.com', port: 3));

      final service2 = SettingsService();
      await service2.load();
      expect(service2.endpoints.length, 3);
      expect(service2.endpoints.map((e) => e.host), ['a.com', 'b.com', 'c.com']);
    });
  });

  group('endpoint list mutation API', () {
    test('addEndpoint rejects duplicate host:port', () async {
      final service = SettingsService();
      await service.load();
      expect(await service.addEndpoint(_ep(host: 'a.com', port: 1)), isTrue);
      expect(await service.addEndpoint(_ep(host: 'a.com', port: 1)), isFalse);
      expect(service.endpoints.length, 1);
    });

    test('removeEndpoint removes by value, unaffected by reorder', () async {
      final service = SettingsService();
      await service.load();
      final a = _ep(host: 'a.com', port: 1);
      final b = _ep(host: 'b.com', port: 2);
      await service.addEndpoint(a);
      await service.addEndpoint(b);
      await service.reorderEndpoint(0, 2); // a moves after b
      await service.removeEndpoint(a);
      expect(service.endpoints.map((e) => e.host), ['b.com']);
    });

    test('reorderEndpoint changes order and persists', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep(host: 'a.com', port: 1));
      await service.addEndpoint(_ep(host: 'b.com', port: 2));
      await service.reorderEndpoint(0, 2);
      expect(service.endpoints.map((e) => e.host), ['b.com', 'a.com']);

      final service2 = SettingsService();
      await service2.load();
      expect(service2.endpoints.map((e) => e.host), ['b.com', 'a.com']);
    });

    test('replaceEndpoint updates by value, not index', () async {
      final service = SettingsService();
      await service.load();
      final a = _ep(host: 'a.com', port: 1);
      await service.addEndpoint(a);
      final updated = a.copyWith(sni: 'new-sni.example.com');
      expect(await service.replaceEndpoint(a, updated), isTrue);
      expect(service.endpoints.first.sni, 'new-sni.example.com');
    });

    test('endpoints getter is unmodifiable', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep());
      expect(() => service.endpoints.add(_ep(host: 'x', port: 9)), throwsUnsupportedError);
    });
  });

  group('isConfigured', () {
    test('empty list -> false', () async {
      final service = SettingsService();
      await service.load();
      await service.setSharedSecret('a' * 64);
      expect(service.isConfigured, isFalse);
    });

    test('valid list + 64-hex secret -> true', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep());
      await service.setSharedSecret('a' * 64);
      expect(service.isConfigured, isTrue);
    });

    test('invalid endpoint in list -> false', () async {
      final service = SettingsService();
      await service.load();
      await service.setEndpoints([_ep().copyWith(port: 0)]);
      await service.setSharedSecret('a' * 64);
      expect(service.isConfigured, isFalse);
    });

    test('secret with non-hex characters -> false', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep());
      await service.setSharedSecret('z' * 64);
      expect(service.isConfigured, isFalse);
    });

    test('secret length != 64 -> false', () async {
      final service = SettingsService();
      await service.load();
      await service.addEndpoint(_ep());
      await service.setSharedSecret('a' * 63);
      expect(service.isConfigured, isFalse);
    });
  });

  group('socksPort validation', () {
    test('out-of-range port falls back to default', () async {
      final service = SettingsService();
      await service.load();
      await service.setSocksPort(70000);
      expect(service.socksPort, 18080);
    });

    test('port 0 falls back to default', () async {
      final service = SettingsService();
      await service.load();
      await service.setSocksPort(0);
      expect(service.socksPort, 18080);
    });

    test('valid port is kept as-is', () async {
      final service = SettingsService();
      await service.load();
      await service.setSocksPort(19999);
      expect(service.socksPort, 19999);
    });
  });

  group('legacy getters return safe defaults', () {
    test('empty list -> empty strings, not a hardcoded test PSK', () async {
      final service = SettingsService();
      await service.load();
      expect(service.legacyClientPsk, '');
      expect(service.legacyServerPub, '');
      expect(service.legacyHost, '');
      expect(service.legacyPort, 0);
    });
  });
}