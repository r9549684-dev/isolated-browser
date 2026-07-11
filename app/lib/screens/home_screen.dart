import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../services/transport_service.dart';
import '../services/settings_service.dart';
import '../services/subscription_service.dart';
import '../models/web_service.dart';
import '../widgets/service_grid.dart';
import '../widgets/transport_status_bar.dart';
import 'settings_screen.dart';
import 'browser_screen.dart';
import 'select_services_screen.dart';
import 'owner_panel_screen.dart';
import 'subscription_screen.dart';
import 'payment_simulation_screen.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      context.read<TransportService>().start();
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Isolated Browser'),
        centerTitle: true,
        actions: [
          // Подписка
          IconButton(
            icon: const Icon(Icons.card_membership),
            tooltip: 'Подписка',
            onPressed: () => Navigator.push(
              context,
              MaterialPageRoute(
                builder: (_) => SubscriptionScreen(
                  subscriptionService: context.read<SubscriptionService>(),
                ),
              ),
            ),
          ),
          // Управление подпиской / выбор сервисов
          IconButton(
            icon: const Icon(Icons.apps),
            tooltip: 'Сервисы',
            onPressed: () => Navigator.push(
              context,
              MaterialPageRoute(builder: (_) => const SelectServicesScreen()),
            ),
          ),
          // Настройки транспорта
          IconButton(
            icon: const Icon(Icons.settings),
            tooltip: 'Settings',
            onPressed: () => Navigator.push(
              context,
              MaterialPageRoute(builder: (_) => const SettingsScreen()),
            ),
          ),
          // Панель овнера (длинное нажатие на settings — скрытый вход)
          PopupMenuButton<String>(
            icon: const Icon(Icons.more_vert),
            onSelected: (v) {
              if (v == 'owner') {
                Navigator.push(
                  context,
                  MaterialPageRoute(builder: (_) => const OwnerPanelScreen()),
                );
              } else if (v == 'payment') {
                Navigator.push(
                  context,
                  MaterialPageRoute(
                    builder: (_) => PaymentSimulationScreen(
                      subscriptionService: context.read<SubscriptionService>(),
                    ),
                  ),
                );
              }
            },
            itemBuilder: (_) => const [
              PopupMenuItem(value: 'owner', child: Text('Owner Panel')),
              PopupMenuItem(value: 'payment', child: Text('Оплата (тест)')),
            ],
          ),
        ],
      ),
      body: Column(
        children: [
          Consumer<TransportService>(
            builder: (context, transport, _) {
              return Container(
                padding: const EdgeInsets.all(16),
                child: Row(
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            'Status: ${transport.state.name.toUpperCase()}',
                            style: Theme.of(context).textTheme.bodyMedium,
                          ),
                          if (transport.errorMessage.isNotEmpty)
                            Padding(
                              padding: const EdgeInsets.only(top: 4),
                              child: Text(
                                transport.errorMessage,
                                style: Theme.of(context).textTheme.bodySmall?.copyWith(color: Colors.red),
                              ),
                            ),
                          if (!context.watch<SettingsService>().isConfigured)
                            Padding(
                              padding: const EdgeInsets.only(top: 4),
                              child: Text(
                                'Tap Settings to configure gateway',
                                style: Theme.of(context).textTheme.bodySmall?.copyWith(color: Colors.orange),
                              ),
                            ),
                        ],
                      ),
                    ),
                    ElevatedButton.icon(
                      onPressed: transport.isRunning
                          ? () => transport.stop()
                          : () => transport.start(),
                      icon: Icon(transport.isRunning ? Icons.stop : Icons.play_arrow),
                      label: Text(transport.isRunning ? 'Stop' : 'Start'),
                      style: ElevatedButton.styleFrom(
                        backgroundColor: transport.isRunning ? Colors.red : Colors.green,
                        foregroundColor: Colors.white,
                      ),
                    ),
                  ],
                ),
              );
            },
          ),
          const TransportStatusBar(),
          Expanded(
            child: ServiceGrid(
              onServiceTap: (service) => _openBrowser(context, service),
            ),
          ),
        ],
      ),
    );
  }

  void _openBrowser(BuildContext context, WebService service) {
    final transport = context.read<TransportService>();
    if (!transport.isRunning) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            transport.state == TransportState.starting
                ? 'Connecting…'
                : 'Transport error: ${transport.errorMessage}',
          ),
          backgroundColor: Colors.orange,
        ),
      );
      return;
    }

    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => BrowserScreen(service: service),
      ),
    );
  }
}
