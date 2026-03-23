import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:bloc/bloc.dart';
import 'package:intiface_central/bloc/util/network_info_cubit.dart';

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

abstract class WebhookServerState {}

class WebhookServerStopped extends WebhookServerState {}

class WebhookServerStarting extends WebhookServerState {}

class WebhookServerRunning extends WebhookServerState {
  final int port;
  final int deviceCount;
  final String mobileIp;
  final String version;
  final List<PairedSessionInfo> sessions;

  WebhookServerRunning({
    required this.port,
    required this.deviceCount,
    required this.mobileIp,
    required this.version,
    required this.sessions,
  });
}

class WebhookServerError extends WebhookServerState {
  final String message;
  WebhookServerError(this.message);
}

class PairedSessionInfo {
  final String sessionId;
  final String firebaseUid;
  final int createdAt;
  final String? reactWebhookUrl;

  PairedSessionInfo({
    required this.sessionId,
    required this.firebaseUid,
    required this.createdAt,
    this.reactWebhookUrl,
  });

  factory PairedSessionInfo.fromJson(Map<String, dynamic> json) {
    return PairedSessionInfo(
      sessionId: json['session_id'] as String,
      firebaseUid: json['firebase_uid'] as String,
      createdAt: json['created_at'] as int,
      reactWebhookUrl: json['react_webhook_url'] as String?,
    );
  }
}

// ---------------------------------------------------------------------------
// Cubit
// ---------------------------------------------------------------------------

const int _kWebhookPort = 8888;
const Duration _kPollInterval = Duration(seconds: 3);

class WebhookServerCubit extends Cubit<WebhookServerState> {
  Timer? _pollTimer;
  final NetworkInfoCubit _networkInfoCubit;
  String? _mobileIp;

  WebhookServerCubit(this._networkInfoCubit) : super(WebhookServerStopped()) {
    _mobileIp = _networkInfoCubit.ip;
  }

  static WebhookServerCubit create(NetworkInfoCubit networkInfoCubit) {
    return WebhookServerCubit(networkInfoCubit);
  }

  /// Called by EngineControlBloc listeners to tell the cubit the engine started.
  void onEngineStarted() {
    emit(WebhookServerStarting());
    _startPolling();
  }

  /// Called by EngineControlBloc listeners to tell the cubit the engine stopped.
  void onEngineStopped() {
    _stopPolling();
    emit(WebhookServerStopped());
  }

  void _startPolling() {
    _pollTimer?.cancel();
    // First poll immediately then on interval
    _poll();
    _pollTimer = Timer.periodic(_kPollInterval, (_) => _poll());
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  Future<void> _poll() async {
    final baseUrl = 'http://127.0.0.1:$_kWebhookPort';
    try {
      final client = HttpClient();
      client.connectionTimeout = const Duration(seconds: 2);

      // Fetch server status
      final statusRequest =
          await client.getUrl(Uri.parse('$baseUrl/api/server/status'));
      final statusResponse = await statusRequest.close();
      if (statusResponse.statusCode != 200) {
        // Server not ready yet, stay in starting state
        if (state is! WebhookServerStarting) {
          emit(WebhookServerStarting());
        }
        client.close(force: true);
        return;
      }

      final statusBody =
          await statusResponse.transform(utf8.decoder).join();
      final statusJson =
          jsonDecode(statusBody) as Map<String, dynamic>;

      final mobileIp = _mobileIp ?? '127.0.0.1';
      final sessions = await _fetchSessions(baseUrl, client);

      emit(WebhookServerRunning(
        port: statusJson['port'] as int? ?? _kWebhookPort,
        deviceCount: statusJson['device_count'] as int? ?? 0,
        mobileIp: mobileIp,
        version: statusJson['version'] as String? ?? '1.0',
        sessions: sessions,
      ));

      client.close();
    } catch (_) {
      // Connection refused means engine/webhook not up yet
      if (state is! WebhookServerStarting) {
        emit(WebhookServerStarting());
      }
    }
  }

  Future<List<PairedSessionInfo>> _fetchSessions(
      String baseUrl, HttpClient client) async {
    try {
      final req =
          await client.getUrl(Uri.parse('$baseUrl/api/sessions'));
      // Sessions endpoint requires a pairing token; without one we get 401,
      // which is fine – just return an empty list.
      final resp = await req.close();
      if (resp.statusCode != 200) return [];
      final body = await resp.transform(utf8.decoder).join();
      final json = jsonDecode(body) as Map<String, dynamic>;
      final sessionsJson = json['sessions'] as List<dynamic>? ?? [];
      return sessionsJson
          .whereType<Map<String, dynamic>>()
          .map(PairedSessionInfo.fromJson)
          .toList();
    } catch (_) {
      return [];
    }
  }

  Future<Map<String, dynamic>?> fetchQrData() async {
    try {
      final client = HttpClient();
      client.connectionTimeout = const Duration(seconds: 2);
      final req = await client.getUrl(
          Uri.parse('http://127.0.0.1:$_kWebhookPort/api/pairing/qr'));
      final resp = await req.close();
      if (resp.statusCode != 200) return null;
      final body = await resp.transform(utf8.decoder).join();
      client.close();
      return jsonDecode(body) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  Future<bool> revokeSession(String sessionId) async {
    try {
      final client = HttpClient();
      client.connectionTimeout = const Duration(seconds: 2);
      final req = await client.deleteUrl(
          Uri.parse('http://127.0.0.1:$_kWebhookPort/api/sessions/$sessionId'));
      final resp = await req.close();
      client.close();
      if (resp.statusCode == 204) {
        await _poll(); // refresh state
        return true;
      }
      return false;
    } catch (_) {
      return false;
    }
  }

  @override
  Future<void> close() {
    _stopPolling();
    return super.close();
  }
}
