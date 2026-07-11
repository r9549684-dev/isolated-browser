import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/settings_service.dart';
import '../services/transport_service.dart';

class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key});

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  late final TextEditingController _hostCtrl;
  late final TextEditingController _portCtrl;
  late final TextEditingController _secretCtrl;
  late final TextEditingController _socksPortCtrl;
  late final TextEditingController _sniListCtrl;
  late final TextEditingController _serverPubCtrl;

  @override
  void initState() {
    super.initState();
    final s = context.read<SettingsService>();
    _hostCtrl     = TextEditingController(text: s.gatewayHost);
    _portCtrl     = TextEditingController(text: s.gatewayPort.toString());
    _secretCtrl   = TextEditingController(text: s.sharedSecret);
    _socksPortCtrl = TextEditingController(text: s.socksPort.toString());
    _sniListCtrl  = TextEditingController(text: s.sniList);
    _serverPubCtrl = TextEditingController(text: s.serverPublic);
  }

  @override
  void dispose() {
    _hostCtrl.dispose();
    _portCtrl.dispose();
    _secretCtrl.dispose();
    _socksPortCtrl.dispose();
    _sniListCtrl.dispose();
    _serverPubCtrl.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final s = context.read<SettingsService>();
    s.gatewayHost  = _hostCtrl.text.trim();
    s.gatewayPort  = int.tryParse(_portCtrl.text.trim()) ?? 443;
    s.sharedSecret = _secretCtrl.text.trim();
    s.socksPort    = int.tryParse(_socksPortCtrl.text.trim()) ?? 18080;
    s.sniList      = _sniListCtrl.text.trim();
    s.serverPublic = _serverPubCtrl.text.trim();
    await s.save();

    if (!mounted) return;
    // Перезапускаем транспорт с новыми настройками
    await context.read<TransportService>().start();
    Navigator.pop(context);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Settings'),
        actions: [
          TextButton(
            onPressed: _save,
            child: const Text('Save', style: TextStyle(color: Colors.white)),
          ),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          _SectionHeader('Gateway Server'),
          _Field(
            controller: _hostCtrl,
            label: 'Host',
            hint: 'gw.example.com',
            keyboardType: TextInputType.url,
          ),
          const SizedBox(height: 12),
          _Field(
            controller: _portCtrl,
            label: 'Port',
            hint: '443',
            keyboardType: TextInputType.number,
          ),
          const SizedBox(height: 24),
          _SectionHeader('Encryption'),
          _Field(
            controller: _secretCtrl,
            label: 'Shared Secret (64 hex chars)',
            hint: 'a1b2c3d4…',
            obscureText: true,
            maxLength: 64,
          ),
          const SizedBox(height: 24),
          _SectionHeader('Stealth Mode'),
          _Field(
            controller: _sniListCtrl,
            label: 'SNI Pool (comma-separated)',
            hint: 'cloudflare.com,google.com',
            keyboardType: TextInputType.url,
          ),
          const SizedBox(height: 12),
          _Field(
            controller: _serverPubCtrl,
            label: 'Server X25519 Public Key (64 hex chars)',
            hint: 'a1b2c3d4…',
            obscureText: true,
            maxLength: 64,
          ),
          const SizedBox(height: 24),
          _SectionHeader('Local Proxy'),
          _Field(
            controller: _socksPortCtrl,
            label: 'SOCKS5 Port',
            hint: '18080',
            keyboardType: TextInputType.number,
          ),
          const SizedBox(height: 32),
          FilledButton(
            onPressed: _save,
            child: const Text('Save & Connect'),
          ),
        ],
      ),
    );
  }
}

class _SectionHeader extends StatelessWidget {
  final String title;
  const _SectionHeader(this.title);

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Text(
        title,
        style: Theme.of(context).textTheme.labelLarge?.copyWith(
              color: Theme.of(context).colorScheme.primary,
            ),
      ),
    );
  }
}

class _Field extends StatelessWidget {
  final TextEditingController controller;
  final String label;
  final String hint;
  final bool obscureText;
  final TextInputType? keyboardType;
  final int? maxLength;

  const _Field({
    required this.controller,
    required this.label,
    required this.hint,
    this.obscureText = false,
    this.keyboardType,
    this.maxLength,
  });

  @override
  Widget build(BuildContext context) {
    return TextField(
      controller: controller,
      obscureText: obscureText,
      keyboardType: keyboardType,
      maxLength: maxLength,
      decoration: InputDecoration(
        labelText: label,
        hintText: hint,
        border: const OutlineInputBorder(),
        filled: true,
      ),
    );
  }
}
