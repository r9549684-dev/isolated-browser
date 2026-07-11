import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/catalog_service.dart';
import '../services/promo_service.dart';
import '../services/subscription_service.dart';
import '../models/web_service.dart';

/// Экран выбора сервисов.
///
/// Базовый тариф: пользователь выбирает любые 2 из каталога.
/// Каждый следующий — отдельная кнопка «+ Добавить за X».
class SelectServicesScreen extends StatelessWidget {
  const SelectServicesScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final catalog = context.watch<CatalogService>();
    final items   = catalog.catalogWithStatus;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Your Services'),
        actions: [
          IconButton(
            icon: const Icon(Icons.card_giftcard),
            tooltip: 'Enter promo code',
            onPressed: () => _showPromoDialog(context),
          ),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(36),
          child: _BaseSlotsBar(
            used:  catalog.baseSlotsUsed,
            total: catalog.baseSlotsTotal,
            plan:  catalog.plan,
          ),
        ),
      ),
      body: ListView.separated(
        padding: const EdgeInsets.symmetric(vertical: 8),
        itemCount: items.length,
        separatorBuilder: (_, __) => const Divider(height: 1),
        itemBuilder: (context, i) {
          final item = items[i];
          return _ServiceTile(
            service:     item.service,
            isBase:      item.isBase,
            isExtra:     item.isExtra,
            hasBaseSlot: catalog.hasBaseSlot,
            onSelectBase:   () => _onSelectBase(context, catalog, item.service),
            onDeselectBase: () => catalog.deselectBase(item.service.id),
            onUnlockExtra:  () => _onUnlockExtra(context, catalog, item.service),
            onRevokeExtra:  () => catalog.revokeExtra(item.service.id),
          );
        },
      ),
    );
  }

  Future<void> _onSelectBase(
    BuildContext context,
    CatalogService catalog,
    WebService service,
  ) async {
    if (catalog.plan == SubscriptionPlan.none) {
      _showNoPlan(context);
      return;
    }
    final ok = await catalog.selectBase(service.id);
    if (!ok && context.mounted) {
      // Слоты заполнены — предложить поменять один из текущих
      _showSwapDialog(context, catalog, service);
    }
  }

  void _onUnlockExtra(
    BuildContext context,
    CatalogService catalog,
    WebService service,
  ) {
    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: Text('Add ${service.name}'),
        content: const Text(
          'This service is an add-on to your base plan.\n'
          'Proceed to payment?',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              Navigator.pop(context);
              // TODO: payment flow
              await catalog.unlockExtra(service.id);
              if (context.mounted) {
                ScaffoldMessenger.of(context).showSnackBar(
                  SnackBar(content: Text('${service.name} added!')),
                );
              }
            },
            child: const Text('Add & Pay'),
          ),
        ],
      ),
    );
  }

  void _showSwapDialog(
    BuildContext context,
    CatalogService catalog,
    WebService newService,
  ) {
    final current = catalog.catalog
        .where((s) => catalog.isInBase(s.id))
        .toList();

    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: const Text('Replace a service'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('Your base slots are full. Replace one of:'),
            const SizedBox(height: 12),
            ...current.map((s) => ListTile(
              leading: Icon(s.icon, color: s.color),
              title: Text(s.name),
              onTap: () async {
                Navigator.pop(context);
                await catalog.swapBase(s.id, newService.id);
              },
            )),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
        ],
      ),
    );
  }

  void _showNoPlan(BuildContext context) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text('Subscribe to a plan to select services.')),
    );
  }

  void _showPromoDialog(BuildContext context) {
    final controller = TextEditingController();
    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: const Text('Promo Code'),
        content: TextField(
          controller: controller,
          autofocus: true,
          textCapitalization: TextCapitalization.characters,
          decoration: const InputDecoration(
            hintText: 'Enter code',
            border: OutlineInputBorder(),
          ),
          onSubmitted: (value) async {
            Navigator.pop(context);
            await _activatePromo(context, value);
          },
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              Navigator.pop(context);
              await _activatePromo(context, controller.text);
            },
            child: const Text('Activate'),
          ),
        ],
      ),
    );
  }

  Future<void> _activatePromo(BuildContext context, String code) async {
    if (code.trim().isEmpty) return;

    final subscriptionService = context.read<SubscriptionService>();
    final result = await PromoService.activate(code, subscriptionService);

    if (!context.mounted) return;

    switch (result) {
      case PromoResult.success:
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Code "$code" activated!'),
            backgroundColor: Colors.green,
          ),
        );
      case PromoResult.invalid:
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Invalid promo code'),
            backgroundColor: Colors.red,
          ),
        );
      case PromoResult.alreadyUsed:
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('This code has already been used'),
            backgroundColor: Colors.orange,
          ),
        );
    }
  }
}

// ── Виджеты ──────────────────────────────────────────────────────────────────

class _BaseSlotsBar extends StatelessWidget {
  final int used;
  final int total;
  final SubscriptionPlan plan;
  const _BaseSlotsBar({required this.used, required this.total, required this.plan});

  @override
  Widget build(BuildContext context) {
    if (plan == SubscriptionPlan.none) {
      return Container(
        color: Colors.orange.withOpacity(0.1),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
        child: const Row(children: [
          Icon(Icons.warning_amber, color: Colors.orange, size: 14),
          SizedBox(width: 6),
          Text('No active plan', style: TextStyle(color: Colors.orange, fontSize: 12)),
        ]),
      );
    }
    final color = used >= total ? Colors.green : Colors.blue;
    return Container(
      color: color.withOpacity(0.08),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
      child: Row(children: [
        Icon(Icons.check_circle, color: color, size: 14),
        const SizedBox(width: 6),
        Text(
          'Base plan: $used / $total selected',
          style: TextStyle(color: color, fontSize: 12, fontWeight: FontWeight.w600),
        ),
      ]),
    );
  }
}

class _ServiceTile extends StatelessWidget {
  final WebService  service;
  final bool        isBase;
  final bool        isExtra;
  final bool        hasBaseSlot;
  final VoidCallback onSelectBase;
  final VoidCallback onDeselectBase;
  final VoidCallback onUnlockExtra;
  final VoidCallback onRevokeExtra;

  const _ServiceTile({
    required this.service,
    required this.isBase,
    required this.isExtra,
    required this.hasBaseSlot,
    required this.onSelectBase,
    required this.onDeselectBase,
    required this.onUnlockExtra,
    required this.onRevokeExtra,
  });

  @override
  Widget build(BuildContext context) {
    return ListTile(
      leading: Container(
        width: 44, height: 44,
        decoration: BoxDecoration(
          color: service.color.withOpacity(0.12),
          borderRadius: BorderRadius.circular(12),
        ),
        child: Icon(service.icon, color: service.color, size: 22),
      ),
      title: Text(service.name),
      subtitle: Text(
        isBase  ? 'Base plan'  :
        isExtra ? 'Add-on'     :
        hasBaseSlot ? 'Tap to select (base slot)' : 'Available as add-on',
        style: TextStyle(
          fontSize: 11,
          color: isBase ? Colors.green : isExtra ? Colors.blue : Colors.grey,
        ),
      ),
      trailing: _buildTrailing(context),
    );
  }

  Widget _buildTrailing(BuildContext context) {
    if (isBase) {
      return TextButton(
        onPressed: onDeselectBase,
        child: const Text('Remove', style: TextStyle(color: Colors.red, fontSize: 12)),
      );
    }
    if (isExtra) {
      return TextButton(
        onPressed: onRevokeExtra,
        child: const Text('Revoke', style: TextStyle(color: Colors.red, fontSize: 12)),
      );
    }
    if (hasBaseSlot) {
      return TextButton(onPressed: onSelectBase, child: const Text('Select'));
    }
    return TextButton(onPressed: onUnlockExtra, child: const Text('+ Add'));
  }
}
