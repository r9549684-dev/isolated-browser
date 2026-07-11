enum SubscriptionTier {
  trial,
  base,
  pro,
  premium,
}

extension SubscriptionTierExtension on SubscriptionTier {
  String get name {
    switch (this) {
      case SubscriptionTier.trial:
        return 'Trial';
      case SubscriptionTier.base:
        return 'Base';
      case SubscriptionTier.pro:
        return 'Pro';
      case SubscriptionTier.premium:
        return 'Premium';
    }
  }

  int get priceRub {
    switch (this) {
      case SubscriptionTier.trial:
        return 0;
      case SubscriptionTier.base:
        return 150;
      case SubscriptionTier.pro:
        return 250;
      case SubscriptionTier.premium:
        return 350;
    }
  }

  int get maxServices {
    switch (this) {
      case SubscriptionTier.trial:
        return 2;
      case SubscriptionTier.base:
        return 2;
      case SubscriptionTier.pro:
        return 5;
      case SubscriptionTier.premium:
        return 999; // All services
    }
  }

  int get rateLimitMbps {
    switch (this) {
      case SubscriptionTier.trial:
        return 3;
      case SubscriptionTier.base:
        return 3;
      case SubscriptionTier.pro:
        return 3;
      case SubscriptionTier.premium:
        return 10;
    }
  }

  int get rateLimitBytesPerSecond => rateLimitMbps * 1024 * 1024;

  Duration get trialDuration {
    switch (this) {
      case SubscriptionTier.trial:
        return const Duration(days: 3);
      default:
        return const Duration(days: 30);
    }
  }

  bool get hasPriority {
    return this == SubscriptionTier.premium;
  }
}

class Subscription {
  final String id;
  final String userId;
  final SubscriptionTier tier;
  final DateTime createdAt;
  final DateTime? expiresAt;
  final List<String> selectedServices;
  final int additionalDevices;
  final bool speedBoostEnabled;
  final String? promoCode;

  Subscription({
    required this.id,
    required this.userId,
    required this.tier,
    required this.createdAt,
    this.expiresAt,
    this.selectedServices = const [],
    this.additionalDevices = 0,
    this.speedBoostEnabled = false,
    this.promoCode,
  });

  int get effectiveRateLimitBytesPerSecond {
    int baseLimit = tier.rateLimitBytesPerSecond;
    if (speedBoostEnabled) {
      return 10 * 1024 * 1024; // 10 MB/s
    }
    return baseLimit;
  }

  int get monthlyPriceRub {
    int basePrice = tier.priceRub;
    int deviceCost = additionalDevices * 50;
    int speedBoostCost = speedBoostEnabled ? 50 : 0;
    return basePrice + deviceCost + speedBoostCost;
  }

  bool get isActive {
    if (expiresAt == null) return true;
    return DateTime.now().isBefore(expiresAt!);
  }

  bool get isTrial {
    return tier == SubscriptionTier.trial;
  }

  int get availableServiceSlots {
    return tier.maxServices - selectedServices.length;
  }

  Subscription copyWith({
    SubscriptionTier? tier,
    DateTime? expiresAt,
    List<String>? selectedServices,
    int? additionalDevices,
    bool? speedBoostEnabled,
    String? promoCode,
  }) {
    return Subscription(
      id: id,
      userId: userId,
      tier: tier ?? this.tier,
      createdAt: createdAt,
      expiresAt: expiresAt ?? this.expiresAt,
      selectedServices: selectedServices ?? this.selectedServices,
      additionalDevices: additionalDevices ?? this.additionalDevices,
      speedBoostEnabled: speedBoostEnabled ?? this.speedBoostEnabled,
      promoCode: promoCode ?? this.promoCode,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'id': id,
      'userId': userId,
      'tier': tier.name,
      'createdAt': createdAt.toIso8601String(),
      'expiresAt': expiresAt?.toIso8601String(),
      'selectedServices': selectedServices,
      'additionalDevices': additionalDevices,
      'speedBoostEnabled': speedBoostEnabled,
      'promoCode': promoCode,
    };
  }

  factory Subscription.fromJson(Map<String, dynamic> json) {
    return Subscription(
      id: json['id'] as String,
      userId: json['userId'] as String,
      tier: SubscriptionTier.values.firstWhere(
        (t) => t.name == json['tier'],
        orElse: () => SubscriptionTier.trial,
      ),
      createdAt: DateTime.parse(json['createdAt'] as String),
      expiresAt: json['expiresAt'] != null
          ? DateTime.parse(json['expiresAt'] as String)
          : null,
      selectedServices: List<String>.from(json['selectedServices'] ?? []),
      additionalDevices: json['additionalDevices'] as int? ?? 0,
      speedBoostEnabled: json['speedBoostEnabled'] as bool? ?? false,
      promoCode: json['promoCode'] as String?,
    );
  }
}
