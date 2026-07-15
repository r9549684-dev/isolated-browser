import 'package:shared_preferences/shared_preferences.dart';

class SettingsService {
  static const _keyGatewayHost = 'gateway_host';
  static const _keyGatewayPort = 'gateway_port';
  static const _keySharedSecret = 'shared_secret';
  static const _keySocksPort   = 'socks_port';
  static const _keySniList = 'sni_list';
  static const _keyServerPublic = 'server_public';
  static const _keyClientPsk = 'client_psk';

  String gatewayHost   = '';
  int    gatewayPort   = 443;
  String sharedSecret  = '';   // hex-encoded 32 bytes
  int    socksPort     = 18080;
  String sniList       = 'cloudflare.com,google.com';  // comma-separated SNI pool
  String serverPublic  = '';   // hex-encoded 32 bytes X25519 public key
  String clientPsk     = 'ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789';   // test mode PSK

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    gatewayHost  = prefs.getString(_keyGatewayHost)  ?? '';
    gatewayPort  = prefs.getInt(_keyGatewayPort)     ?? 443;
    sharedSecret = prefs.getString(_keySharedSecret) ?? '';
    socksPort    = prefs.getInt(_keySocksPort)       ?? 18080;
    sniList      = prefs.getString(_keySniList)      ?? 'cloudflare.com,google.com';
    serverPublic = prefs.getString(_keyServerPublic) ?? '';
    clientPsk    = prefs.getString(_keyClientPsk)    ?? 'ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789';
  }

  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_keyGatewayHost,  gatewayHost);
    await prefs.setInt   (_keyGatewayPort,  gatewayPort);
    await prefs.setString(_keySharedSecret, sharedSecret);
    await prefs.setInt   (_keySocksPort,    socksPort);
    await prefs.setString(_keySniList,      sniList);
    await prefs.setString(_keyServerPublic, serverPublic);
    await prefs.setString(_keyClientPsk,    clientPsk);
  }

  bool get isConfigured =>
      gatewayHost.isNotEmpty && sharedSecret.length == 64 && serverPublic.length == 64;
}
