import 'dart:convert';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import '../models/subscription.dart';

class SubscriptionService {
  static const String _subscriptionKey = 'current_subscription';
  static const String _userIdKey = 'user_id';
  static const String _trialStartedKey = 'trial_started_at';
  static const String _jwtTokenKey = 'jwt_token';

  // Secure storage для JWT — шифрование at rest (Keystore Android, Keychain iOS).
  // SharedPreferences хранит plaintext, что небезопасно для токенов.
  final FlutterSecureStorage _secureStorage = const FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
    iOptions: IOSOptions(accessibility: KeychainAccessibility.first_unlock),
  );

  String? _userId;
  Subscription? _currentSubscription;
  String? _jwtToken;

  Future<void> init() async {
    final prefs = await SharedPreferences.getInstance();
    
    _userId = prefs.getString(_userIdKey);
    if (_userId == null) {
      _userId = DateTime.now().millisecondsSinceEpoch.toString();
      await prefs.setString(_userIdKey, _userId!);
    }

    final subJson = prefs.getString(_subscriptionKey);
    if (subJson != null) {
      _currentSubscription = Subscription.fromJson(
        json.decode(subJson) as Map<String, dynamic>,
      );
    }

    // JWT из secure storage (не из SharedPreferences).
    _jwtToken = await _secureStorage.read(key: _jwtTokenKey);

    if (_currentSubscription == null) {
      await _startTrialIfNeeded();
    }

    // Генерируем JWT токен для текущей подписки
    await _generateJwtTokenIfNeeded();
  }

  Future<void> _generateJwtTokenIfNeeded() async {
    if (_currentSubscription == null) return;
    
    // В реальности JWT токен должен генерироваться сервером после оплаты
    // Здесь генерируем локально для тестирования
    final now = DateTime.now();
    final exp = _currentSubscription!.expiresAt ?? now.add(const Duration(days: 30));
    
    final payload = {
      'sub': _currentSubscription!.id,
      'user_id': _userId!,
      'tier': _currentSubscription!.tier.name,
      'rate_limit_bps': _currentSubscription!.effectiveRateLimitBytesPerSecond,
      'exp': exp.millisecondsSinceEpoch ~/ 1000,
      'iat': now.millisecondsSinceEpoch ~/ 1000,
    };
    
    // Кодируем в base64 (упрощённо, без подписи)
    final header = base64Url.encode(utf8.encode('{"alg":"HS256","typ":"JWT"}'));
    final body = base64Url.encode(utf8.encode(json.encode(payload)));
    _jwtToken = '$header.$body.test_signature';
    
    // Сохраняем в secure storage (шифрование at rest).
    await _secureStorage.write(key: _jwtTokenKey, value: _jwtToken!);
  }

  String? get jwtToken => _jwtToken;

  Future<void> _startTrialIfNeeded() async {
    final prefs = await SharedPreferences.getInstance();
    final trialStarted = prefs.getString(_trialStartedKey);
    
    if (trialStarted != null) {
      return;
    }

    final now = DateTime.now();
    await prefs.setString(_trialStartedKey, now.toIso8601String());

    _currentSubscription = Subscription(
      id: 'sub_${now.millisecondsSinceEpoch}',
      userId: _userId!,
      tier: SubscriptionTier.trial,
      createdAt: now,
      expiresAt: now.add(const Duration(days: 3)),
      selectedServices: [],
    );

    await _saveSubscription();
  }

  Subscription? get currentSubscription => _currentSubscription;

  String? get userId => _userId;

  Future<void> _saveSubscription() async {
    if (_currentSubscription == null) return;
    
    final prefs = await SharedPreferences.getInstance();
    final jsonStr = json.encode(_currentSubscription!.toJson());
    await prefs.setString(_subscriptionKey, jsonStr);
  }

  Future<bool> upgradeToTier(SubscriptionTier tier) async {
    if (_currentSubscription == null) return false;

    _currentSubscription = _currentSubscription!.copyWith(
      tier: tier,
      expiresAt: DateTime.now().add(const Duration(days: 30)),
    );

    await _saveSubscription();
    return true;
  }

  Future<bool> addSelectedService(String serviceId) async {
    if (_currentSubscription == null) return false;

    final current = _currentSubscription!;
    if (current.selectedServices.length >= current.tier.maxServices) {
      return false;
    }

    final updatedServices = [...current.selectedServices, serviceId];
    _currentSubscription = current.copyWith(selectedServices: updatedServices);
    await _saveSubscription();
    return true;
  }

  Future<bool> removeSelectedService(String serviceId) async {
    if (_currentSubscription == null) return false;

    final current = _currentSubscription!;
    final updatedServices = current.selectedServices
        .where((id) => id != serviceId)
        .toList();
    
    _currentSubscription = current.copyWith(selectedServices: updatedServices);
    await _saveSubscription();
    return true;
  }

  Future<bool> confirmServiceSelection() async {
    if (_currentSubscription == null) return false;
    // После подтверждения выбор сервисов фиксируется до смены тарифа
    await _saveSubscription();
    return true;
  }

  Future<bool> enableSpeedBoost() async {
    if (_currentSubscription == null) return false;

    _currentSubscription = _currentSubscription!.copyWith(
      speedBoostEnabled: true,
    );
    await _saveSubscription();
    return true;
  }

  Future<bool> disableSpeedBoost() async {
    if (_currentSubscription == null) return false;

    _currentSubscription = _currentSubscription!.copyWith(
      speedBoostEnabled: false,
    );
    await _saveSubscription();
    return true;
  }

  Future<bool> addDevice() async {
    if (_currentSubscription == null) return false;

    _currentSubscription = _currentSubscription!.copyWith(
      additionalDevices: _currentSubscription!.additionalDevices + 1,
    );
    await _saveSubscription();
    return true;
  }

  Future<bool> applyPromoCode(String code, SubscriptionTier tier, int months) async {
    if (_currentSubscription == null) return false;

    final now = DateTime.now();
    final expiresAt = now.add(Duration(days: 30 * months));

    _currentSubscription = Subscription(
      id: 'sub_${now.millisecondsSinceEpoch}',
      userId: _userId!,
      tier: tier,
      createdAt: now,
      expiresAt: expiresAt,
      selectedServices: _currentSubscription!.selectedServices,
      promoCode: code,
    );

    await _saveSubscription();
    return true;
  }

  int getRateLimitBytesPerSecond() {
    if (_currentSubscription == null) {
      return 3 * 1024 * 1024; // Default 3 MB/s
    }
    return _currentSubscription!.effectiveRateLimitBytesPerSecond;
  }

  bool canAccessService(String serviceId) {
    if (_currentSubscription == null) return false;
    
    final sub = _currentSubscription!;
    if (!sub.isActive) return false;
    
    if (sub.tier == SubscriptionTier.premium) return true;
    
    return sub.selectedServices.contains(serviceId);
  }

  Future<void> resetForTesting() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_subscriptionKey);
    await prefs.remove(_trialStartedKey);
    await _secureStorage.delete(key: _jwtTokenKey);
    _currentSubscription = null;
    await _startTrialIfNeeded();
  }
}
