import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/catalog_service.dart';
import '../models/web_service.dart';

/// Овнер-панель: управление каталогом сервисов.
/// Доступна только владельцу (защита PIN-кодом).
class OwnerPanelScreen extends StatefulWidget {
  const OwnerPanelScreen({super.key});

  @override
  State<OwnerPanelScreen> createState() => _OwnerPanelScreenState();
}

class _OwnerPanelScreenState extends State<OwnerPanelScreen> {
  bool _authenticated = false;

  @override
  Widget build(BuildContext context) {
    if (!_authenticated) {
      return _PinGate(onSuccess: () => setState(() => _authenticated = true));
    }
    return _OwnerPanelBody();
  }
}

// ── PIN-экран ────────────────────────────────────────────────────────────────

class _PinGate extends StatefulWidget {
  final VoidCallback onSuccess;
  const _PinGate({required this.onSuccess});

  @override
  State<_PinGate> createState() => _PinGateState();
}

class _PinGateState extends State<_PinGate> {
  final _ctrl = TextEditingController();
  String? _error;

  // TODO: хранить хэш PIN в зашифрованном хранилище
  static const _kOwnerPin = '1234';

  void _verify() {
    if (_ctrl.text == _kOwnerPin) {
      widget.onSuccess();
    } else {
      setState(() => _error = 'Wrong PIN');
      _ctrl.clear();
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Owner Panel')),
      body: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Icon(Icons.admin_panel_settings, size: 64, color: Colors.grey),
            const SizedBox(height: 24),
            const Text('Enter owner PIN', style: TextStyle(fontSize: 18)),
            const SizedBox(height: 16),
            TextField(
              controller: _ctrl,
              obscureText: true,
              keyboardType: TextInputType.number,
              maxLength: 8,
              decoration: InputDecoration(
                labelText: 'PIN',
                errorText: _error,
                border: const OutlineInputBorder(),
              ),
              onSubmitted: (_) => _verify(),
            ),
            const SizedBox(height: 16),
            FilledButton(onPressed: _verify, child: const Text('Enter')),
          ],
        ),
      ),
    );
  }
}

// ── Основной экран панели ─────────────────────────────────────────────────────

class _OwnerPanelBody extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final catalog = context.watch<CatalogService>();
    final all     = catalog.catalog;
    final custom  = all.where((s) => s.isCustom).toList();
    final builtin = all.where((s) => !s.isCustom).toList();

    return Scaffold(
      appBar: AppBar(
        title: const Text('Owner Panel — Catalog'),
        actions: [
          IconButton(
            icon: const Icon(Icons.add),
            tooltip: 'Add service',
            onPressed: () => _showAddDialog(context, catalog),
          ),
        ],
      ),
      body: ListView(
        children: [

          // ── Встроенные сервисы ─────────────────────────────────────────
          const _SectionHeader('Built-in Services'),
          ...builtin.map((s) => _BuiltinTile(service: s)),

          // ── Кастомные сервисы ──────────────────────────────────────────
          const _SectionHeader('Custom Services (added by you)'),
          if (custom.isEmpty)
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Text('No custom services yet. Tap + to add.',
                  style: TextStyle(color: Colors.grey)),
            ),
          ...custom.map((s) => _CustomTile(
            service: s,
            onEdit:   () => _showEditDialog(context, catalog, s),
            onDelete: () => _confirmDelete(context, catalog, s),
          )),

        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _showAddDialog(context, catalog),
        icon: const Icon(Icons.add),
        label: const Text('Add Service'),
      ),
    );
  }

  void _showAddDialog(BuildContext context, CatalogService catalog) {
    final nameCtrl = TextEditingController();
    final urlCtrl  = TextEditingController();

    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: const Text('Add Custom Service'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: nameCtrl,
              decoration: const InputDecoration(
                labelText: 'Service name',
                hintText:  'e.g. My News Site',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: urlCtrl,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                labelText: 'URL',
                hintText:  'https://example.com',
                border: OutlineInputBorder(),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              final name = nameCtrl.text.trim();
              final url  = urlCtrl.text.trim();
              if (name.isEmpty || url.isEmpty) return;
              Navigator.pop(context);
              final svc = await catalog.ownerAddService(name: name, url: url);
              if (context.mounted) {
                ScaffoldMessenger.of(context).showSnackBar(
                  SnackBar(content: Text('"${svc.name}" added to catalog')),
                );
              }
            },
            child: const Text('Add'),
          ),
        ],
      ),
    );
  }

  void _showEditDialog(BuildContext context, CatalogService catalog, WebService s) {
    final nameCtrl = TextEditingController(text: s.name);
    final urlCtrl  = TextEditingController(text: s.url);

    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: Text('Edit "${s.name}"'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: nameCtrl,
              decoration: const InputDecoration(
                labelText: 'Name', border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: urlCtrl,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                labelText: 'URL', border: OutlineInputBorder(),
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              Navigator.pop(context);
              await catalog.ownerEditService(
                s.id,
                name: nameCtrl.text.trim(),
                url:  urlCtrl.text.trim(),
              );
            },
            child: const Text('Save'),
          ),
        ],
      ),
    );
  }

  void _confirmDelete(BuildContext context, CatalogService catalog, WebService s) {
    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        title: Text('Remove "${s.name}"?'),
        content: const Text('This will remove the service from the catalog '
            'and revoke access for all users.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () async {
              Navigator.pop(context);
              await catalog.ownerRemoveService(s.id);
            },
            style: TextButton.styleFrom(foregroundColor: Colors.red),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
  }
}

// ── Вспомогательные виджеты ──────────────────────────────────────────────────

class _SectionHeader extends StatelessWidget {
  final String title;
  const _SectionHeader(this.title);
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
    child: Text(title,
        style: Theme.of(context).textTheme.labelLarge?.copyWith(
              color: Theme.of(context).colorScheme.primary,
            )),
  );
}

class _BuiltinTile extends StatelessWidget {
  final WebService service;
  const _BuiltinTile({required this.service});
  @override
  Widget build(BuildContext context) => ListTile(
    leading: Icon(service.icon, color: service.color),
    title: Text(service.name),
    subtitle: Text(service.url,
        style: const TextStyle(fontSize: 11), overflow: TextOverflow.ellipsis),
    trailing: const Chip(label: Text('Built-in', style: TextStyle(fontSize: 11))),
  );
}

class _CustomTile extends StatelessWidget {
  final WebService  service;
  final VoidCallback onEdit;
  final VoidCallback onDelete;
  const _CustomTile({required this.service, required this.onEdit, required this.onDelete});

  @override
  Widget build(BuildContext context) => ListTile(
    leading: Icon(service.icon, color: service.color),
    title: Text(service.name),
    subtitle: Text(service.url,
        style: const TextStyle(fontSize: 11), overflow: TextOverflow.ellipsis),
    trailing: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        IconButton(icon: const Icon(Icons.edit, size: 20),   onPressed: onEdit),
        IconButton(icon: const Icon(Icons.delete, size: 20,
            color: Colors.red), onPressed: onDelete),
      ],
    ),
  );
}
