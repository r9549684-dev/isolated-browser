import 'package:flutter/material.dart';
import '../models/subscription.dart';
import '../models/web_service.dart';
import '../services/catalog_service.dart';
import '../services/subscription_service.dart';

class ServiceSelectionScreen extends StatefulWidget {
  final SubscriptionService subscriptionService;
  final CatalogService catalogService;

  const ServiceSelectionScreen({
    super.key,
    required this.subscriptionService,
    required this.catalogService,
  });

  @override
  State<ServiceSelectionScreen> createState() => _ServiceSelectionScreenState();
}

class _ServiceSelectionScreenState extends State<ServiceSelectionScreen> {
  Set<String> _pendingSelection = {};
  bool _isLoading = false;

  @override
  void initState() {
    super.initState();
    _loadCurrentSelection();
  }

  void _loadCurrentSelection() {
    final sub = widget.subscriptionService.currentSubscription;
    if (sub != null) {
      _pendingSelection = Set.from(sub.selectedServices);
    }
  }

  @override
  Widget build(BuildContext context) {
    final subscription = widget.subscriptionService.currentSubscription;
    if (subscription == null) {
      return const Scaffold(
        body: Center(child: Text('Подписка не активна')),
      );
    }

    final availableSlots = subscription.availableServiceSlots;
    final services = widget.catalogService.catalog;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Выбор сервисов'),
        actions: [
          if (_hasChanges())
            TextButton(
              onPressed: _isLoading ? null : _showConfirmDialog,
              child: const Text('Подтвердить'),
            ),
        ],
      ),
      body: Column(
        children: [
          _buildSubscriptionInfo(subscription),
          _buildSlotsIndicator(availableSlots, subscription.tier.maxServices),
          const Divider(),
          Expanded(
            child: ListView.builder(
              itemCount: services.length,
              itemBuilder: (context, index) {
                final service = services[index];
                final isSelected = _pendingSelection.contains(service.id);
                final canSelect = availableSlots > 0 || isSelected;

                return ListTile(
                  leading: Icon(
                    isSelected ? Icons.check_circle : Icons.circle_outlined,
                    color: isSelected ? Colors.green : Colors.grey,
                  ),
                  title: Text(service.name),
                  subtitle: Text(service.description),
                  enabled: canSelect,
                  onTap: canSelect
                      ? () => _toggleService(service.id)
                      : null,
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildSubscriptionInfo(Subscription subscription) {
    return Container(
      padding: const EdgeInsets.all(16),
      color: Colors.blue.shade50,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Тариф: ${subscription.tier.name}',
            style: const TextStyle(
              fontSize: 18,
              fontWeight: FontWeight.bold,
            ),
          ),
          const SizedBox(height: 4),
          Text(
            'Доступно сервисов: ${subscription.tier.maxServices}',
            style: const TextStyle(fontSize: 14),
          ),
          if (subscription.isTrial)
            Container(
              margin: const EdgeInsets.only(top: 8),
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              decoration: BoxDecoration(
                color: Colors.orange.shade100,
                borderRadius: BorderRadius.circular(4),
              ),
              child: Text(
                'Тестовый период',
                style: TextStyle(
                  color: Colors.orange.shade900,
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildSlotsIndicator(int available, int total) {
    final selected = total - available;
    return Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Выбрано: $selected из $total',
            style: const TextStyle(
              fontSize: 16,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 8),
          LinearProgressIndicator(
            value: selected / total,
            backgroundColor: Colors.grey.shade200,
            valueColor: AlwaysStoppedAnimation<Color>(
              available > 0 ? Colors.green : Colors.orange,
            ),
          ),
          if (available == 0)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                'Все слоты заняты. Перейдите на более высокий тариф для выбора дополнительных сервисов.',
                style: TextStyle(
                  fontSize: 12,
                  color: Colors.grey.shade600,
                ),
              ),
            ),
        ],
      ),
    );
  }

  void _toggleService(String serviceId) {
    setState(() {
      if (_pendingSelection.contains(serviceId)) {
        _pendingSelection.remove(serviceId);
      } else {
        _pendingSelection.add(serviceId);
      }
    });
  }

  bool _hasChanges() {
    final current = widget.subscriptionService.currentSubscription;
    if (current == null) return false;
    
    final currentSet = Set.from(current.selectedServices);
    return !_pendingSelection.containsAll(currentSet) ||
        !currentSet.containsAll(_pendingSelection);
  }

  Future<void> _showConfirmDialog() async {
    final subscription = widget.subscriptionService.currentSubscription;
    if (subscription == null) return;

    final serviceNames = _pendingSelection.map((id) {
      final service = widget.catalogService.catalog.firstWhere(
        (s) => s.id == id,
        orElse: () => WebService(
          id: id,
          name: 'Неизвестно',
          description: '',
          url: '',
          icon: Icons.help,
          color: Colors.grey,
        ),
      );
      return service.name;
    }).toList();

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Подтверждение выбора'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Вы выбрали следующие сервисы:',
              style: TextStyle(fontWeight: FontWeight.w500),
            ),
            const SizedBox(height: 12),
            ...serviceNames.map((name) => Padding(
              padding: const EdgeInsets.only(left: 8, bottom: 4),
              child: Text('• $name'),
            )),
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
                  Icon(Icons.warning, color: Colors.orange, size: 20),
                  SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      'После подтверждения изменить выбор можно только при переходе на более высокий тариф.',
                      style: TextStyle(fontSize: 12),
                    ),
                  ),
                ],
              ),
            ),
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
      await _confirmSelection();
    }
  }

  Future<void> _confirmSelection() async {
    setState(() => _isLoading = true);

    try {
      final subscription = widget.subscriptionService.currentSubscription;
      if (subscription == null) return;

      // Удаляем старые сервисы
      for (final serviceId in subscription.selectedServices) {
        await widget.subscriptionService.removeSelectedService(serviceId);
      }

      // Добавляем новые
      for (final serviceId in _pendingSelection) {
        await widget.subscriptionService.addSelectedService(serviceId);
      }

      await widget.subscriptionService.confirmServiceSelection();

      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Выбор сервисов подтверждён'),
            backgroundColor: Colors.green,
          ),
        );
        Navigator.pop(context);
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
