import 'package:shared_preferences/shared_preferences.dart';
import '../models/subscription.dart';
import 'subscription_service.dart';

enum PromoCodeType {
  pro3months,
  pro6months,
  pro12months,
}

class PromoService {
  static const Map<String, PromoCodeType> _validCodes = {
    'PRO3M': PromoCodeType.pro3months,
    'PRO6M': PromoCodeType.pro6months,
    'PRO12M': PromoCodeType.pro12months,
    'TEST2024': PromoCodeType.pro3months,
    'FRIEND50': PromoCodeType.pro3months,
    'VIP2024': PromoCodeType.pro6months,
    'GIFT100': PromoCodeType.pro3months,
    'BETA2025': PromoCodeType.pro12months,
  };

  static const String _activatedKey = 'activated_promo_codes';

  static bool isValid(String code) =>
      _validCodes.containsKey(code.toUpperCase().trim());

  static PromoCodeType? getType(String code) =>
      _validCodes[code.toUpperCase().trim()];

  static (SubscriptionTier tier, int months) _getTierAndMonths(
      PromoCodeType type) {
    switch (type) {
      case PromoCodeType.pro3months:
        return (SubscriptionTier.pro, 3);
      case PromoCodeType.pro6months:
        return (SubscriptionTier.pro, 6);
      case PromoCodeType.pro12months:
        return (SubscriptionTier.pro, 12);
    }
  }

  static Future<PromoResult> activate(
      String code, SubscriptionService subscriptionService) async {
    final normalizedCode = code.toUpperCase().trim();

    if (!_validCodes.containsKey(normalizedCode)) {
      return PromoResult.invalid;
    }

    final prefs = await SharedPreferences.getInstance();
    final activated = prefs.getStringList(_activatedKey) ?? [];

    if (activated.contains(normalizedCode)) {
      return PromoResult.alreadyUsed;
    }

    final type = _validCodes[normalizedCode]!;
    final (tier, months) = _getTierAndMonths(type);

    await subscriptionService.applyPromoCode(normalizedCode, tier, months);

    activated.add(normalizedCode);
    await prefs.setStringList(_activatedKey, activated);

    return PromoResult.success;
  }

  static Future<List<String>> getActivated() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getStringList(_activatedKey) ?? [];
  }
}

enum PromoResult {
  success,
  invalid,
  alreadyUsed,
}
