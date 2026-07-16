// app/lib/screens/settings_screen.dart

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/endpoint.dart';
import '../services/settings_service.dart';
import '../services/transport_service.dart';

class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key});

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  late final TextEditingController _socksPortCtrl;
  late final TextEditingController _secretCtrl;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    final settings = context.read<SettingsService>();
    _socksPortCtrl = TextEditingController(text: settings.socksPort.toString());
    _secretCtrl = TextEditingController(text: settings.sharedSecret);
  }

  @override
  void dispose() {
    _socksPortCtrl.dispose();
    _secretCtrl.dispose();
    super.dispose();
  }

  Future<void> _openEndpointDialog({GatewayEndpoint? existing}) async {
    final settings = context.read<SettingsService>();
    final result = await showDialog<GatewayEndpoint>(
      context: context,
      builder: (_) => _EndpointEditDialog(existing: existing),
    );
    if (result == null) return;

    final bool ok;
    if (existing != null) {
      ok = await settings.replaceEndpoint(existing, result);
    } else {
      ok = await settings.addEndpoint(result);
    }

    if (!mounted) return;
    if (!ok) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Duplicate host:port already exists in the list')),
      );
    }
  }

  Future<void> _removeEndpoint(GatewayEndpoint endpoint) async {
    // Удаление по значению endpoint (не по индексу) — индекс в момент свайпа
    // мог устареть из-за reorder/предыдущих удалений (Ревизор A7, критично).
    await context.read<SettingsService>().removeEndpoint(endpoint);
  }

  Future<void> _reorder(int oldIndex, int newIndex) async {
    await context.read<SettingsService>().reorderEndpoint(oldIndex, newIndex);
  }

  /// Парсит и валидирует socksPort перед записью. Значение вне диапазона
  /// отклоняется с явным уведомлением, а не тихо заменяется дефолтом
  /// (Ревизор A7: "тихая потеря ввода").
  int? _parseAndValidateSocksPort() {
    final parsed = int.tryParse(_socksPortCtrl.text.trim());
    if (parsed == null || parsed < 1 || parsed > 65535) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('SOCKS port must be a number between 1 and 65535')),
      );
      return null;
    }
    return parsed;
  }

  Future<void> _save() async {
    final settings = context.read<SettingsService>();
    final transport = context.read<TransportService>();

    final port = _parseAndValidateSocksPort();
    if (port == null) return;

    setState(() => _saving = true);

    await settings.setSocksPort(port);
    await settings.setSharedSecret(_secretCtrl.text.trim());

    if (!mounted) return;

    // Валидация ПЕРЕД запуском транспорта и ПЕРЕД Navigator.pop (Ревизор A7,
    // критично): раньше экран закрывался независимо от результата start(),
    // и пользователь не видел, что конфигурация невалидна.
    if (!settings.isConfigured) {
      setState(() => _saving = false);
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Configuration incomplete: add at least one valid endpoint and a 64-character hex shared secret'),
        ),
      );
      return;
    }

    await transport.start();

    if (!mounted) return;
    setState(() => _saving = false);

    if (transport.state == TransportState.error) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Failed to start transport: ${transport.errorMessage}')),
      );
      return; // не закрываем экран — пользователь должен видеть ошибку и исходные значения.
    }

    if (!mounted) return;
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final settings = context.watch<SettingsService>();
    final endpoints = settings.endpoints;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Gateway Settings'),
        actions: [
          IconButton(
            icon: const Icon(Icons.add),
            tooltip: 'Add endpoint',
            onPressed: () => _openEndpointDialog(),
          ),
        ],
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              children: [
                TextField(
                  controller: _socksPortCtrl,
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(labelText: 'SOCKS port'),
                ),
                const SizedBox(height: 8),
                TextField(
                  controller: _secretCtrl,
                  maxLength: 64,
                  decoration: const InputDecoration(
                    labelText: 'Shared secret (64 hex chars, reserved field)',
                    helperText: 'Currently not used for encryption; reserved by the frozen FFI contract.',
                    helperMaxLines: 2,
                  ),
                ),
              ],
            ),
          ),
          Expanded(
            child: endpoints.isEmpty
                ? const Center(child: Text('No endpoints configured'))
                : ReorderableListView.builder(
                    itemCount: endpoints.length,
                    onReorder: _reorder,
                    itemBuilder: (context, index) {
                      final endpoint = endpoints[index];
                      // Ключ строится по значению endpoint (hostPortKey), не по
                      // индексу — стабилен при reorder, уникален по построению,
                      // т.к. SettingsService отклоняет дубликаты host:port
                      // (Ревизор A7, критично).
                      final key = ValueKey(endpoint.hostPortKey);
                      return Dismissible(
                        key: key,
                        direction: DismissDirection.endToStart,
                        background: Container(
                          color: Colors.red,
                          alignment: Alignment.centerRight,
                          padding: const EdgeInsets.only(right: 16),
                          child: const Icon(Icons.delete, color: Colors.white),
                        ),
                        onDismissed: (_) => _removeEndpoint(endpoint),
                        child: ListTile(
                          key: ValueKey('tile_${endpoint.hostPortKey}'),
                          title: Text('${endpoint.host}:${endpoint.port}'),
                          subtitle: Text('SNI: ${endpoint.sni}'),
                          trailing: IconButton(
                            icon: const Icon(Icons.edit),
                            onPressed: () => _openEndpointDialog(existing: endpoint),
                          ),
                        ),
                      );
                    },
                  ),
          ),
          Padding(
            padding: const EdgeInsets.all(16),
            child: SizedBox(
              width: double.infinity,
              child: ElevatedButton(
                onPressed: _saving ? null : _save,
                child: _saving
                    ? const SizedBox(height: 20, width: 20, child: CircularProgressIndicator(strokeWidth: 2))
                    : const Text('Save & Connect'),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Диалог добавления/редактирования одного endpoint. При `existing != null`
/// поля предзаполняются значениями существующего endpoint (редактирование —
/// функциональность, отсутствовавшая в черновике, отмеченная ревизором как
/// пробел: "только add/remove/reorder").
class _EndpointEditDialog extends StatefulWidget {
  final GatewayEndpoint? existing;
  const _EndpointEditDialog({this.existing});

  @override
  State<_EndpointEditDialog> createState() => _EndpointEditDialogState();
}

class _EndpointEditDialogState extends State<_EndpointEditDialog> {
  late final TextEditingController _host;
  late final TextEditingController _port;
  late final TextEditingController _sni;
  late final TextEditingController _pub;
  late final TextEditingController _psk;

  @override
  void initState() {
    super.initState();
    final e = widget.existing;
    _host = TextEditingController(text: e?.host ?? '');
    _port = TextEditingController(text: e != null ? e.port.toString() : '443');
    _sni = TextEditingController(text: e?.sni ?? '');
    _pub = TextEditingController(text: e?.serverPub ?? '');
    _psk = TextEditingController(text: e?.psk ?? '');
  }

  @override
  void dispose() {
    _host.dispose();
    _port.dispose();
    _sni.dispose();
    _pub.dispose();
    _psk.dispose();
    super.dispose();
  }

  void _submit() {
    final port = int.tryParse(_port.text.trim()) ?? 0;
    final candidate = GatewayEndpoint(
      host: _host.text.trim(),
      port: port,
      sni: _sni.text.trim(),
      serverPub: _pub.text.trim(),
      psk: _psk.text.trim(),
    );

    // Единая функция валидации (GatewayEndpoint.isValid, включая isHex64) —
    // устраняет дублирование "валидация hex только по длине" в диалоге
    // (Ревизор A5/A7, сквозное замечание).
    if (!candidate.isValid) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Invalid endpoint: check host, port (1-65535), sni, and that '
            'server_pub/psk are exactly 64 hex characters',
          ),
        ),
      );
      return;
    }

    Navigator.of(context).pop(candidate);
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.existing == null ? 'Add endpoint' : 'Edit endpoint'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(controller: _host, decoration: const InputDecoration(labelText: 'Host')),
            TextField(
              controller: _port,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(labelText: 'Port'),
            ),
            TextField(controller: _sni, decoration: const InputDecoration(labelText: 'SNI')),
            TextField(
              controller: _pub,
              maxLength: 64,
              decoration: const InputDecoration(labelText: 'Server public key (64 hex)'),
            ),
            TextField(
              controller: _psk,
              maxLength: 64,
              decoration: const InputDecoration(labelText: 'PSK (64 hex)'),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        ElevatedButton(onPressed: _submit, child: const Text('Save')),
      ],
    );
  }
}