import 'package:flutter/material.dart';
import '../models/subscription.dart';
import '../services/subscription_service.dart';

class SubscriptionScreen extends StatefulWidget {
  final SubscriptionService subscriptionService;

  const SubscriptionScreen({
    super.key,
    required this.subscriptionService,
  });

  @override
  State<SubscriptionScreen> createState() => _SubscriptionScreenState();
}

class _SubscriptionScreenState extends State<SubscriptionScreen> {
  bool _isLoading = false;

  @override
  Widget build(BuildContext context) {
    final subscription = widget.subscriptionService.currentSubscription;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Подписка'),
      ),
      body: subscription == null
          ? const Center(child: CircularProgressIndicator())
          : Column(
              children: [
                _buildCurrentSubscription(subscription),
                const Divider(),
                Expanded(
                  child: ListView(
                    children: [
                      _buildTierCard(
                        tier: SubscriptionTier.base,
                        currentTier: subscription.tier,
                        onTap: () => _upgradeTo(SubscriptionTier.base),
                      ),
                      _buildTierCard(
                        tier: SubscriptionTier.pro,
                        currentTier: subscription.tier,
                        onTap: () => _upgradeTo(SubscriptionTier.pro),
                      ),
                      _buildTierCard(
                        tier: SubscriptionTier.premium,
                        currentTier: subscription.tier,
                        onTap: () => _upgradeTo(SubscriptionTier.premium),
                      ),
                    ],
                  ),
                ),
              ],
            ),
    );
  }

  Widget _buildCurrentSubscription(Subscription subscription) {
    return Container(
      padding: const EdgeInsets.all(16),
      color: Colors.blue.shade50,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(
                'Текущий тариф: ${subscription.tier.name}',
                style: const TextStyle(
                  fontSize: 20,
                  fontWeight: FontWeight.bold,
                ),
              ),
              if (subscription.isTrial) ...[
                const SizedBox(width: 8),
                Container(
                  padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                  decoration: BoxDecoration(
                    color: Colors.orange.shade100,
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Text(
                    'ТРИАЛ',
                    style: TextStyle(
                      color: Colors.orange.shade900,
                      fontSize: 10,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                ),
              ],
            ],
          ),
          const SizedBox(height: 8),
          _buildInfoRow('Цена', '${subscription.monthlyPriceRub} ₽/мес'),
          _buildInfoRow('Скорость', '${subscription.tier.rateLimitMbps} МБ/с'),
          if (subscription.speedBoostEnabled)
            _buildInfoRow('Буст скорости', 'Активен (10 МБ/с)'),
          _buildInfoRow('Сервисы', '${subscription.selectedServices.length} из ${subscription.tier.maxServices}'),
          _buildInfoRow('Доп. устройства', '${subscription.additionalDevices}'),
          if (subscription.expiresAt != null)
            _buildInfoRow('Действует до', _formatDate(subscription.expiresAt!)),
        ],
      ),
    );
  }

  Widget _buildInfoRow(String label, String value) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text(label, style: const TextStyle(color: Colors.grey)),
          Text(value, style: const TextStyle(fontWeight: FontWeight.w500)),
        ],
      ),
    );
  }

  Widget _buildTierCard({
    required SubscriptionTier tier,
    required SubscriptionTier currentTier,
    required VoidCallback onTap,
  }) {
    final isCurrentTier = tier == currentTier;
    final isDowngrade = _isDowngrade(tier, currentTier);

    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: ListTile(
        contentPadding: const EdgeInsets.all(16),
        title: Text(
          tier.name,
          style: const TextStyle(
            fontSize: 18,
            fontWeight: FontWeight.bold,
          ),
        ),
        subtitle: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const SizedBox(height: 8),
            Text('${tier.priceRub} ₽/мес'),
            const SizedBox(height: 4),
            Text('Сервисы: ${tier.maxServices == 999 ? "Все" : tier.maxServices}'),
            Text('Скорость: ${tier.rateLimitMbps} МБ/с'),
            if (tier.hasPriority)
              const Text('Приоритет: Да'),
          ],
        ),
        trailing: isCurrentTier
            ? const Chip(label: Text('Текущий'))
            : isDowngrade
                ? const Chip(label: Text('Понижение'))
                : ElevatedButton(
                    onPressed: _isLoading ? null : onTap,
                    child: const Text('Выбрать'),
                  ),
      ),
    );
  }

  bool _isDowngrade(SubscriptionTier target, SubscriptionTier current) {
    const order = [
      SubscriptionTier.trial,
      SubscriptionTier.base,
      SubscriptionTier.pro,
      SubscriptionTier.premium,
    ];
    return order.indexOf(target) < order.indexOf(current);
  }

  String _formatDate(DateTime date) {
    return '${date.day.toString().padLeft(2, '0')}.${date.month.toString().padLeft(2, '0')}.${date.year}';
  }

  Future<void> _upgradeTo(SubscriptionTier tier) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Переход на ${tier.name}'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Цена: ${tier.priceRub} ₽/мес'),
            const SizedBox(height: 8),
            Text('Сервисы: ${tier.maxServices == 999 ? "Все" : tier.maxServices}'),
            Text('Скорость: ${tier.rateLimitMbps} МБ/с'),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Отмена'),
          ),
          ElevatedButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Подтвердить'),
          ),
        ],
      ),
    );

    if (confirmed == true) {
      setState(() => _isLoading = true);

      try {
        await widget.subscriptionService.upgradeToTier(tier);

        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text('Тариф изменён на ${tier.name}'),
              backgroundColor: Colors.green,
            ),
          );
          setState(() {});
        }
      } catch (e) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(
              content: Text('Ошибка: $e'),
              backgroundColor: Colors.red,
            ),
          );
        }
      } finally {
        if (mounted) {
          setState(() => _isLoading = false);
        }
      }
    }
  }
}
