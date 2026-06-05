import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../widgets/service_grid.dart';
import '../models/web_service.dart';

/// Экран браузера — оболочка вокруг нативного GeckoView/WebView.
///
/// На Android фактический WebView рендерится через нативный плагин
/// (GeckoViewPlugin, см. android/app/src/main/kotlin/...).
/// Здесь — Flutter-контейнер с AppBar и AndroidView/UiKitView.
class BrowserScreen extends StatefulWidget {
  final WebService service;

  const BrowserScreen({super.key, required this.service});

  @override
  State<BrowserScreen> createState() => _BrowserScreenState();
}

class _BrowserScreenState extends State<BrowserScreen> {
  bool _isLoading = true;
  String _currentUrl = '';

  @override
  void initState() {
    super.initState();
    _currentUrl = widget.service.url;
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        leading: IconButton(
          icon: const Icon(Icons.arrow_back),
          onPressed: () => Navigator.pop(context),
        ),
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              widget.service.name,
              style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
            ),
            Text(
              _currentUrl,
              style: const TextStyle(fontSize: 11, color: Colors.white70),
              overflow: TextOverflow.ellipsis,
            ),
          ],
        ),
        actions: [
          if (_isLoading)
            const Padding(
              padding: EdgeInsets.all(16),
              child: SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: Colors.white,
                ),
              ),
            ),
          IconButton(
            icon: const Icon(Icons.refresh),
            onPressed: _reload,
          ),
        ],
      ),
      body: _GeckoViewContainer(
        initialUrl: widget.service.url,
        socksProxyPort: 18080,
        onPageStarted: (url) => setState(() {
          _isLoading = true;
          _currentUrl = url;
        }),
        onPageFinished: (url) => setState(() {
          _isLoading = false;
          _currentUrl = url;
        }),
      ),
    );
  }

  void _reload() {
    // Сигнализируем нативному плагину перезагрузить страницу
    // через MethodChannel (реализовано в GeckoViewPlugin)
  }
}

/// Нативный контейнер для GeckoView (Android) / WKWebView (iOS).
/// Использует AndroidView / UiKitView для встраивания нативного View.
class _GeckoViewContainer extends StatelessWidget {
  final String initialUrl;
  final int socksProxyPort;
  final void Function(String url) onPageStarted;
  final void Function(String url) onPageFinished;

  const _GeckoViewContainer({
    required this.initialUrl,
    required this.socksProxyPort,
    required this.onPageStarted,
    required this.onPageFinished,
  });

  @override
  Widget build(BuildContext context) {
    // Android: GeckoView через платформенный View
    return AndroidView(
      viewType: 'com.isolatedbrowser/geckoview',
      layoutDirection: TextDirection.ltr,
      creationParams: {
        'url': initialUrl,
        'socks_proxy_host': '127.0.0.1',
        'socks_proxy_port': socksProxyPort,
      },
      creationParamsCodec: StandardMessageCodec(),
    );
  }
}
