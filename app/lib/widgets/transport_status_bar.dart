import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/transport_service.dart';

class TransportStatusBar extends StatelessWidget {
  const TransportStatusBar({super.key});

  @override
  Widget build(BuildContext context) {
    final transport = context.watch<TransportService>();

    final (color, icon, label) = switch (transport.state) {
      TransportState.idle     => (Colors.grey,   Icons.circle_outlined,  'Not connected'),
      TransportState.starting => (Colors.orange, Icons.sync,             'Connecting…'),
      TransportState.running  => (Colors.green,  Icons.circle,           'Protected'),
      TransportState.error    => (Colors.red,    Icons.error_outline,    'Error'),
    };

    return Container(
      color: color.withOpacity(0.1),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: Row(
        children: [
          Icon(icon, color: color, size: 14),
          const SizedBox(width: 8),
          Text(
            label,
            style: TextStyle(
              color: color,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (transport.state == TransportState.error) ...[
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                transport.errorMessage,
                style: TextStyle(color: color, fontSize: 11),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            TextButton(
              onPressed: () => context.read<TransportService>().start(),
              child: const Text('Retry', style: TextStyle(fontSize: 11)),
            ),
          ],
          if (transport.state == TransportState.running)
            const Spacer(),
        ],
      ),
    );
  }
}
