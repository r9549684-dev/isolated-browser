import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import '../models/web_service.dart';

const kBaseSlots = 2; // базовый тариф: 2 сервиса на выбор

enum SubscriptionPlan { none, base, extended }

/// CatalogService — единственный источник правды о каталоге и подписке.
///
/// Бизнес-логика:
///   - none:     доступа нет, только просмотр каталога
///   - base:     пользователь выбирает ЛЮБЫЕ 2 сервиса из каталога
///   - extended: базовые 2 + любые дополнительные (каждый — отдельная доплата)
class CatalogService extends ChangeNotifier {

  List<WebService>   _catalog       = List.of(kBuiltinServices);
  SubscriptionPlan   _plan          = SubscriptionPlan.none;
  List<String>       _baseSelected  = [];  // id сервисов в базовых слотах (≤ kBaseSlots)
  Set<String>        _extraUnlocked = {};  // id дополнительно оплаченных сервисов

  // ── Геттеры ───────────────────────────────────────────────────────────────

  List<WebService>   get catalog => List.unmodifiable(_catalog);
  SubscriptionPlan   get plan    => _plan;

  /// Сервисы, доступные пользователю сейчас.
  List<WebService> get activeServices => _catalog
      .where((s) => _baseSelected.contains(s.id) || _extraUnlocked.contains(s.id))
      .toList();

  int  get baseSlotsUsed  => _baseSelected.length;
  int  get baseSlotsTotal => kBaseSlots;
  bool get hasBaseSlot    => _plan != SubscriptionPlan.none &&
                             _baseSelected.length < kBaseSlots;

  bool isInBase (String id) => _baseSelected.contains(id);
  bool isInExtra(String id) => _extraUnlocked.contains(id);
  bool isActive (String id) => isInBase(id) || isInExtra(id);

  /// Полный список каталога с состоянием для UI.
  List<({WebService service, bool isBase, bool isExtra})>
      get catalogWithStatus => _catalog.map((s) => (
            service: s,
            isBase:  isInBase(s.id),
            isExtra: isInExtra(s.id),
          )).toList();

  // ── Загрузка / сохранение ─────────────────────────────────────────────────

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();

    final planStr     = prefs.getString('subscription_plan') ?? 'none';
    _plan             = SubscriptionPlan.values.byName(planStr);
    _baseSelected     = prefs.getStringList('base_selected')   ?? [];
    _extraUnlocked    = (prefs.getStringList('extra_unlocked') ?? []).toSet();

    final customJson  = prefs.getStringList('custom_services') ?? [];
    final custom      = customJson
        .map((j) => WebService.fromJson(jsonDecode(j) as Map<String, dynamic>))
        .toList();
    _catalog = [...kBuiltinServices, ...custom];

    notifyListeners();
  }

  Future<void> _persist() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString    ('subscription_plan', _plan.name);
    await prefs.setStringList('base_selected',     _baseSelected);
    await prefs.setStringList('extra_unlocked',    _extraUnlocked.toList());

    final custom = _catalog.where((s) => s.isCustom).toList();
    await prefs.setStringList(
      'custom_services',
      custom.map((s) => jsonEncode(s.toJson())).toList(),
    );
  }

  // ── Подписка ──────────────────────────────────────────────────────────────

  /// Активировать базовый тариф (вызывать после оплаты).
  Future<void> activateBase() async {
    _plan = SubscriptionPlan.base;
    await _persist();
    notifyListeners();
  }

  /// Активировать расширенный тариф.
  Future<void> activateExtended() async {
    _plan = SubscriptionPlan.extended;
    await _persist();
    notifyListeners();
  }

  // ── Выбор базовых сервисов ────────────────────────────────────────────────

  /// Выбрать сервис в базовый слот.
  /// Возвращает false если план не активен или слоты заполнены.
  Future<bool> selectBase(String id) async {
    if (_plan == SubscriptionPlan.none)  return false;
    if (_baseSelected.contains(id))      return true;
    if (_baseSelected.length >= kBaseSlots) return false;
    _baseSelected.add(id);
    await _persist();
    notifyListeners();
    return true;
  }

  /// Убрать сервис из базовых слотов (замена на другой).
  Future<void> deselectBase(String id) async {
    _baseSelected.remove(id);
    await _persist();
    notifyListeners();
  }

  /// Заменить один базовый сервис на другой за один шаг.
  Future<void> swapBase(String removeId, String addId) async {
    _baseSelected.remove(removeId);
    if (_baseSelected.length < kBaseSlots) _baseSelected.add(addId);
    await _persist();
    notifyListeners();
  }

  // ── Дополнительные сервисы ────────────────────────────────────────────────

  /// Разблокировать дополнительный сервис (после оплаты).
  Future<void> unlockExtra(String id) async {
    _extraUnlocked.add(id);
    if (_plan == SubscriptionPlan.base) {
      _plan = SubscriptionPlan.extended;
    }
    await _persist();
    notifyListeners();
  }

  /// Отозвать доступ к дополнительному сервису.
  Future<void> revokeExtra(String id) async {
    _extraUnlocked.remove(id);
    if (_extraUnlocked.isEmpty && _plan == SubscriptionPlan.extended) {
      _plan = SubscriptionPlan.base;
    }
    await _persist();
    notifyListeners();
  }

  // ── Овнер-панель ──────────────────────────────────────────────────────────

  Future<WebService> ownerAddService({
    required String name,
    required String url,
  }) async {
    final id = 'custom_${DateTime.now().millisecondsSinceEpoch}';
    final service = WebService(
      id:       id,
      name:     name,
      url:      _normalizeUrl(url),
      icon:     _guessIcon(url),
      color:    _guessColor(url),
      isCustom: true,
    );
    _catalog = [..._catalog, service];
    await _persist();
    notifyListeners();
    return service;
  }

  Future<void> ownerRemoveService(String id) async {
    _catalog       = _catalog.where((s) => s.id != id).toList();
    _baseSelected  = _baseSelected.where((s) => s != id).toList();
    _extraUnlocked = _extraUnlocked.where((s) => s != id).toSet();
    await _persist();
    notifyListeners();
  }

  Future<void> ownerEditService(String id, {String? name, String? url}) async {
    _catalog = _catalog.map((s) {
      if (s.id != id || !s.isCustom) return s;
      return s.copyWith(
        name: name,
        url:  url != null ? _normalizeUrl(url) : null,
      );
    }).toList();
    await _persist();
    notifyListeners();
  }

  // ── Утилиты ───────────────────────────────────────────────────────────────

  static String _normalizeUrl(String url) =>
      url.startsWith('http') ? url : 'https://$url';

  static IconData _guessIcon(String url) {
    final u = url.toLowerCase();
    if (u.contains('mail') || u.contains('email'))             return Icons.email;
    if (u.contains('music') || u.contains('spotify'))          return Icons.music_note;
    if (u.contains('news'))                                     return Icons.newspaper;
    if (u.contains('shop') || u.contains('store'))             return Icons.shopping_bag;
    if (u.contains('video') || u.contains('tube'))             return Icons.play_circle_fill;
    if (u.contains('chat') || u.contains('mess'))              return Icons.chat;
    return Icons.language;
  }

  static Color _guessColor(String url) {
    const palette = [
      Color(0xFF6200EA), Color(0xFF0288D1), Color(0xFF00897B),
      Color(0xFFF4511E), Color(0xFF8D6E63), Color(0xFF039BE5),
      Color(0xFFD81B60), Color(0xFF43A047),
    ];
    return palette[url.hashCode.abs() % palette.length];
  }
}
