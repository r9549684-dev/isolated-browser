import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../services/transport_service.dart';
import '../models/web_service.dart';
import '../widgets/service_grid.dart';
import '../widgets/transport_status_bar.dart';
import 'settings_screen.dart';
import 'browser_screen.dart';
import 'select_services_screen.dart';
import 'owner_panel_screen.dart';

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
          // Управление подпиской / выбор сервисов
          IconButton(
            icon: const Icon(Icons.apps),
            tooltip: 'Manage services',
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
              }
            },
            itemBuilder: (_) => const [
              PopupMenuItem(value: 'owner', child: Text('Owner Panel')),
            ],
          ),
        ],
      ),
      body: Column(
        children: [
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
