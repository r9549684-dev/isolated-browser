import 'package:flutter/material.dart';
import '../models/subscription.dart';
import '../services/subscription_service.dart';

class PaymentSimulationScreen extends StatefulWidget {
  final SubscriptionService subscriptionService;

  const PaymentSimulationScreen({
    super.key,
    required this.subscriptionService,
  });

  @override
  State<PaymentSimulationScreen> createState() => _PaymentSimulationScreenState();
}

class _PaymentSimulationScreenState extends State<PaymentSimulationScreen> {
  bool _isProcessing = false;

  @override
  Widget build(BuildContext context) {
    final subscription = widget.subscriptionService.currentSubscription;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Оплата'),
      ),
      body: subscription == null
          ? const Center(child: CircularProgressIndicator())
          : SingleChildScrollView(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _buildCurrentSubscriptionCard(subscription),
                  const SizedBox(height: 24),
                  const Text(
                    'Дополнительные опции',
                    style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                  const SizedBox(height: 12),
                  _buildOptionCard(
                    title: 'Буст скорости',
                    subtitle: 'Увеличение скорости до 10 МБ/с',
                    price: '50 ₽/мес',
                    isEnabled: subscription.speedBoostEnabled,
                    onToggle: () => _toggleSpeedBoost(),
                  ),
                  _buildOptionCard(
                    title: 'Дополнительное устройство',
                    subtitle: 'Подключить ещё одно устройство',
                    price: '50 ₽/мес',
                    isEnabled: false,
                    onToggle: () => _addDevice(),
                  ),
                  const SizedBox(height: 24),
                  _buildPaymentSummary(subscription),
                  const SizedBox(height: 24),
                  ElevatedButton(
                    onPressed: _isProcessing ? null : _simulatePayment,
                    style: ElevatedButton.styleFrom(
                      padding: const EdgeInsets.all(16),
                    ),
                    child: _isProcessing
                        ? const SizedBox(
                            height: 20,
                            width: 20,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Text(
                            'Оплатить',
                            style: TextStyle(fontSize: 16),
                          ),
                  ),
                  const SizedBox(height: 16),
                  Container(
                    padding: const EdgeInsets.all(12),
                    decoration: BoxDecoration(
                      color: Colors.orange.shade50,
                      borderRadius: BorderRadius.circular(8),
                      border: Border.all(color: Colors.orange.shade200),
                    ),
                    child: const Row(
                      children: [
                        Icon(Icons.info, color: Colors.orange),
                        SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            'Это симуляция оплаты. Реальная оплата не производится.',
                            style: TextStyle(fontSize: 12),
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
    );
  }

  Widget _buildCurrentSubscriptionCard(Subscription subscription) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Тариф: ${subscription.tier.name}',
              style: const TextStyle(
                fontSize: 20,
                fontWeight: FontWeight.bold,
              ),
            ),
            const SizedBox(height: 8),
            Text('Базовая цена: ${subscription.tier.priceRub} ₽/мес'),
            if (subscription.additionalDevices > 0)
              Text('Доп. устройства: ${subscription.additionalDevices * 50} ₽'),
            if (subscription.speedBoostEnabled)
              const Text('Буст скорости: 50 ₽'),
          ],
        ),
      ),
    );
  }

  Widget _buildOptionCard({
    required String title,
    required String subtitle,
    required String price,
    required bool isEnabled,
    required VoidCallback onToggle,
  }) {
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: ListTile(
        title: Text(title),
        subtitle: Text(subtitle),
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(price, style: const TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(width: 8),
            Switch(
              value: isEnabled,
              onChanged: (_) => onToggle(),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildPaymentSummary(Subscription subscription) {
    return Card(
      color: Colors.green.shade50,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Итого к оплате',
              style: TextStyle(
                fontSize: 16,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              '${subscription.monthlyPriceRub} ₽/мес',
              style: const TextStyle(
                fontSize: 32,
                fontWeight: FontWeight.bold,
                color: Colors.green,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _toggleSpeedBoost() async {
    final subscription = widget.subscriptionService.currentSubscription;
    if (subscription == null) return;

    if (subscription.speedBoostEnabled) {
      await widget.subscriptionService.disableSpeedBoost();
    } else {
      await widget.subscriptionService.enableSpeedBoost();
    }
    setState(() {});
  }

  Future<void> _addDevice() async {
    await widget.subscriptionService.addDevice();
    setState(() {});
  }

  Future<void> _simulatePayment() async {
    setState(() => _isProcessing = true);

    // Симуляция задержки обработки платежа
    await Future.delayed(const Duration(seconds: 2));

    setState(() => _isProcessing = false);

    if (mounted) {
      showDialog(
        context: context,
        builder: (context) => AlertDialog(
          title: const Row(
            children: [
              Icon(Icons.check_circle, color: Colors.green, size: 28),
              SizedBox(width: 8),
              Text('Оплата успешна'),
            ],
          ),
          content: const Text('Подписка продлена на 1 месяц'),
          actions: [
            ElevatedButton(
              onPressed: () {
                Navigator.pop(context);
                Navigator.pop(context);
              },
              child: const Text('OK'),
            ),
          ],
        ),
      );
    }
  }
}
