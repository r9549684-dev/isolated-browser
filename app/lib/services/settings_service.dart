import 'package:shared_preferences/shared_preferences.dart';

class SettingsService {
  static const _keyGatewayHost = 'gateway_host';
  static const _keyGatewayPort = 'gateway_port';
  static const _keySharedSecret = 'shared_secret';
  static const _keySocksPort   = 'socks_port';

  String gatewayHost   = '';
  int    gatewayPort   = 443;
  String sharedSecret  = '';   // hex-encoded 32 bytes
  int    socksPort     = 18080;

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    gatewayHost  = prefs.getString(_keyGatewayHost)  ?? '';
    gatewayPort  = prefs.getInt(_keyGatewayPort)     ?? 443;
    sharedSecret = prefs.getString(_keySharedSecret) ?? '';
    socksPort    = prefs.getInt(_keySocksPort)       ?? 18080;
  }

  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keyGatewayHost,  gatewayHost);
    await prefs.setInt   (_keyGatewayPort,  gatewayPort);
    await prefs.setString(_keySharedSecret, sharedSecret);
    await prefs.setInt   (_keySocksPort,    socksPort);
  }

  bool get isConfigured =>
      gatewayHost.isNotEmpty && sharedSecret.length == 64;
}
