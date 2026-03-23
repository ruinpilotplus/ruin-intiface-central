import 'package:bloc/bloc.dart';

enum NavigationPage {
  appControl,
  settings,
  deviceConfig,
  deviceControl,
  logs,
  news,
  about,
  help,
  sendLogs,
  pairing,
  exit,
}

class NavigationCubit extends Cubit<NavigationPage> {
  NavigationCubit() : super(NavigationPage.news);

  void goAppControl() => emit(NavigationPage.appControl);
  void goSettings() => emit(NavigationPage.settings);
  void goNews() => emit(NavigationPage.news);
  void goDeviceControl() => emit(NavigationPage.deviceControl);
  void goDeviceConfig() => emit(NavigationPage.deviceConfig);
  void goLogs() => emit(NavigationPage.logs);
  void goAbout() => emit(NavigationPage.about);
  void goSendLogs() => emit(NavigationPage.sendLogs);
  void goPairing() => emit(NavigationPage.pairing);
  void goExit() => emit(NavigationPage.exit);
}
