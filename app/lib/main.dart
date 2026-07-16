import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'services/transport_service.dart';
import 'services/settings_service.dart';
import 'services/catalog_service.dart';
import 'services/subscription_service.dart';
import 'screens/home_screen.dart';

void main() async {
  WidgetsFlutterBinding.ensureInitialized();

  final settings = SettingsService();
  await settings.load();

  final catalog = CatalogService();
  await catalog.load();

  final subscriptionService = SubscriptionService();
  await subscriptionService.init();

  runApp(
    MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => TransportService(settings, subscriptionService)),
        ChangeNotifierProvider.value(value: settings),
        ChangeNotifierProvider.value(value: catalog),
        Provider.value(value: subscriptionService),
      ],
      child: const IsolatedBrowserApp(),
    ),
  );
}

class IsolatedBrowserApp extends StatelessWidget {
  const IsolatedBrowserApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Isolated Browser',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF1A73E8),
          brightness: Brightness.light,
        ),
        useMaterial3: true,
      ),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF1A73E8),
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
      ),
      home: const HomeScreen(),
    );
  }
}
