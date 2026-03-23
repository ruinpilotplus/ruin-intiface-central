import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';
import 'package:intiface_central/bloc/webhook_server/webhook_server_cubit.dart';
import 'package:qr_flutter/qr_flutter.dart';

/// Displays a QR code that a React web application can scan to pair with
/// the mobile device, plus a list of currently paired sessions that can
/// be revoked.
class PairingPage extends StatefulWidget {
  const PairingPage({super.key});

  @override
  State<PairingPage> createState() => _PairingPageState();
}

class _PairingPageState extends State<PairingPage> {
  Map<String, dynamic>? _qrData;
  bool _loadingQr = false;
  String? _qrError;

  @override
  void initState() {
    super.initState();
    _loadQrData();
  }

  Future<void> _loadQrData() async {
    setState(() {
      _loadingQr = true;
      _qrError = null;
    });
    final cubit = context.read<WebhookServerCubit>();
    final data = await cubit.fetchQrData();
    if (!mounted) return;
    setState(() {
      _loadingQr = false;
      if (data != null) {
        _qrData = data;
      } else {
        _qrError = 'Could not load pairing QR code.\n'
            'Make sure the Intiface engine is running.';
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return BlocBuilder<WebhookServerCubit, WebhookServerState>(
      builder: (context, state) {
        return SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _buildHeader(context, state),
              const SizedBox(height: 16),
              _buildQrSection(context, state),
              const SizedBox(height: 24),
              _buildSessionsSection(context, state),
            ],
          ),
        );
      },
    );
  }

  Widget _buildHeader(BuildContext context, WebhookServerState state) {
    final theme = Theme.of(context);
    final isRunning = state is WebhookServerRunning;
    return Row(
      children: [
        Icon(
          isRunning ? Icons.wifi : Icons.wifi_off,
          color: isRunning ? Colors.green : Colors.grey,
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Web App Pairing',
                style: theme.textTheme.titleLarge,
              ),
              Text(
                isRunning
                    ? 'Webhook server running on port '
                        '${(state as WebhookServerRunning).port}'
                    : state is WebhookServerStarting
                        ? 'Webhook server starting…'
                        : 'Start the Intiface engine to enable pairing',
                style: theme.textTheme.bodySmall,
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildQrSection(BuildContext context, WebhookServerState state) {
    final theme = Theme.of(context);
    if (state is WebhookServerStopped) {
      return Card(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Center(
            child: Text(
              'Start the Intiface engine to generate a pairing QR code.',
              style: theme.textTheme.bodyMedium,
              textAlign: TextAlign.center,
            ),
          ),
        ),
      );
    }

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            Text('Scan with your React web app',
                style: theme.textTheme.titleMedium),
            const SizedBox(height: 12),
            if (_loadingQr)
              const CircularProgressIndicator()
            else if (_qrError != null)
              Column(
                children: [
                  Text(
                    _qrError!,
                    style: theme.textTheme.bodySmall
                        ?.copyWith(color: Colors.red),
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 8),
                  ElevatedButton.icon(
                    onPressed: _loadQrData,
                    icon: const Icon(Icons.refresh),
                    label: const Text('Retry'),
                  ),
                ],
              )
            else if (_qrData != null)
              Column(
                children: [
                  QrImageView(
                    data: jsonEncode(_qrData),
                    version: QrVersions.auto,
                    size: 200,
                    backgroundColor: Colors.white,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'IP: ${_qrData!['mobile_ip']}:${_qrData!['mobile_port']}',
                    style: theme.textTheme.bodySmall,
                  ),
                  const SizedBox(height: 8),
                  OutlinedButton.icon(
                    onPressed: _loadQrData,
                    icon: const Icon(Icons.refresh, size: 16),
                    label: const Text('Regenerate'),
                  ),
                ],
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildSessionsSection(
      BuildContext context, WebhookServerState state) {
    final theme = Theme.of(context);
    final sessions =
        state is WebhookServerRunning ? state.sessions : <PairedSessionInfo>[];

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text('Paired Sessions', style: theme.textTheme.titleMedium),
        const SizedBox(height: 8),
        if (sessions.isEmpty)
          Card(
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Center(
                child: Text(
                  'No sessions paired yet.',
                  style: theme.textTheme.bodySmall,
                ),
              ),
            ),
          )
        else
          ...sessions.map((session) => _buildSessionCard(context, session)),
      ],
    );
  }

  Widget _buildSessionCard(BuildContext context, PairedSessionInfo session) {
    return Card(
      child: ListTile(
        leading: const Icon(Icons.devices_other),
        title: Text(
          session.firebaseUid,
          overflow: TextOverflow.ellipsis,
        ),
        subtitle: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Session: ${session.sessionId.substring(0, 8)}…'),
            if (session.reactWebhookUrl != null)
              Text(
                'Webhook: ${session.reactWebhookUrl}',
                overflow: TextOverflow.ellipsis,
              ),
          ],
        ),
        isThreeLine: session.reactWebhookUrl != null,
        trailing: IconButton(
          icon: const Icon(Icons.delete_outline),
          tooltip: 'Revoke session',
          onPressed: () => _revokeSession(context, session),
        ),
      ),
    );
  }

  Future<void> _revokeSession(
      BuildContext context, PairedSessionInfo session) async {
    final confirm = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Revoke Session'),
        content: Text(
          'Revoke the session for ${session.firebaseUid}?\n'
          'The web app will need to re-pair to reconnect.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child:
                const Text('Revoke', style: TextStyle(color: Colors.red)),
          ),
        ],
      ),
    );
    if (confirm != true || !mounted) return;

    final cubit = context.read<WebhookServerCubit>();
    final success = await cubit.revokeSession(session.sessionId);
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(success ? 'Session revoked.' : 'Failed to revoke session.'),
      ),
    );
  }
}
