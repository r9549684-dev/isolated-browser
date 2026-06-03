import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/catalog_service.dart';
import '../models/web_service.dart';

class ServiceGrid extends StatelessWidget {
  final void Function(WebService service) onServiceTap;

  const ServiceGrid({super.key, required this.onServiceTap});

  @override
  Widget build(BuildContext context) {
    final services = context.watch<CatalogService>().activeServices;

    if (services.isEmpty) {
      return const Center(
        child: Text(
          'No services selected.\nTap "Manage" to choose your plan.',
          textAlign: TextAlign.center,
          style: TextStyle(color: Colors.grey),
        ),
      );
    }

    return GridView.builder(
      padding: const EdgeInsets.all(16),
      gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
        crossAxisCount: 4,
        mainAxisSpacing: 16,
        crossAxisSpacing: 16,
        childAspectRatio: 0.8,
      ),
      itemCount: services.length,
      itemBuilder: (context, index) {
        final service = services[index];
        return _ServiceTile(
          service: service,
          onTap: () => onServiceTap(service),
        );
      },
    );
  }
}

class _ServiceTile extends StatelessWidget {
  final WebService service;
  final VoidCallback onTap;

  const _ServiceTile({required this.service, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 56,
            height: 56,
            decoration: BoxDecoration(
              color: service.color.withOpacity(0.15),
              borderRadius: BorderRadius.circular(16),
              border: Border.all(
                color: service.color.withOpacity(0.3),
                width: 1.5,
              ),
            ),
            child: Icon(service.icon, color: service.color, size: 28),
          ),
          const SizedBox(height: 6),
          Text(
            service.name,
            style: Theme.of(context).textTheme.labelSmall,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.center,
          ),
        ],
      ),
    );
  }
}
