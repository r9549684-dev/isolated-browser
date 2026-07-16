// app/test/models/endpoint_test.dart

import 'package:flutter_test/flutter_test.dart';
import 'package:isolated_browser/models/endpoint.dart';

GatewayEndpoint _valid({String host = 'gw.example.com', int port = 443}) => GatewayEndpoint(
      host: host,
      port: port,
      sni: 'cloudflare.com',
      serverPub: 'a' * 64,
      psk: 'b' * 64,
    );

void main() {
  group('GatewayEndpoint.isValid', () {
    test('valid endpoint passes', () {
      expect(_valid().isValid, isTrue);
    });

    test('port == 0 fails', () {
      expect(_valid(port: 0).isValid, isFalse);
    });

    test('port == 70000 fails', () {
      expect(_valid(port: 70000).isValid, isFalse);
    });

    test('port == 65535 passes (upper boundary)', () {
      expect(_valid(port: 65535).isValid, isTrue);
    });

    test('empty host fails', () {
      expect(_valid(host: '').isValid, isFalse);
    });

    test('host with leading/trailing spaces fails', () {
      expect(_valid(host: ' gw.example.com ').isValid, isFalse);
    });

    test('serverPub/psk length != 64 fails', () {
      final e = _valid().copyWith(serverPub: 'a' * 63);
      expect(e.isValid, isFalse);
    });

    test('64 non-hex characters fails (regression: length-only check was insufficient)', () {
      final e = _valid().copyWith(serverPub: 'z' * 64);
      expect(e.isValid, isFalse);
    });

    test('64 uppercase hex characters passes', () {
      final e = _valid().copyWith(serverPub: 'A' * 64, psk: 'B' * 64);
      expect(e.isValid, isTrue);
    });

    test('empty sni fails', () {
      final e = _valid().copyWith(sni: '');
      expect(e.isValid, isFalse);
    });
  });

  group('fromJson/toJson', () {
    test('round-trip preserves all fields', () {
      final e = _valid();
      final restored = GatewayEndpoint.fromJson(e.toJson());
      expect(restored, e);
    });

    test('port as double is coerced to int', () {
      final json = {'host': 'h', 'port': 443.0, 'sni': 's', 'server_pub': 'a' * 64, 'psk': 'b' * 64};
      final e = GatewayEndpoint.fromJson(json);
      expect(e.port, 443);
    });

    test('port as numeric string is coerced', () {
      final json = {'host': 'h', 'port': '443', 'sni': 's', 'server_pub': 'a' * 64, 'psk': 'b' * 64};
      final e = GatewayEndpoint.fromJson(json);
      expect(e.port, 443);
    });

    test('missing keys do not throw, produce invalid endpoint instead', () {
      expect(() => GatewayEndpoint.fromJson(<String, dynamic>{}), returnsNormally);
      final e = GatewayEndpoint.fromJson(<String, dynamic>{});
      expect(e.isValid, isFalse);
    });

    test('non-string host does not throw', () {
      final json = {'host': 12345, 'port': 443, 'sni': 's', 'server_pub': 'a' * 64, 'psk': 'b' * 64};
      expect(() => GatewayEndpoint.fromJson(json), returnsNormally);
      expect(GatewayEndpoint.fromJson(json).host, '');
    });
  });

  group('equality / hashCode', () {
    test('identical field values are equal', () {
      expect(_valid(), _valid());
      expect(_valid().hashCode, _valid().hashCode);
    });

    test('different port is not equal', () {
      expect(_valid(port: 443), isNot(_valid(port: 444)));
    });

    test('hostPortKey identifies duplicates independent of sni/keys', () {
      final a = _valid();
      final b = _valid().copyWith(sni: 'other.com', serverPub: 'c' * 64);
      expect(a.hostPortKey, b.hostPortKey);
      expect(a, isNot(b)); // разные по полностью, но дублирующие host:port
    });
  });
}