package com.isolatedbrowser.app

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import com.isolatedbrowser.gecko.GeckoViewPlugin

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // Регистрируем фабрику нативного GeckoView
        flutterEngine.platformViewsController.registry.registerViewFactory(
            "com.isolatedbrowser/geckoview",
            GeckoViewPlugin(flutterEngine.dartExecutor.binaryMessenger)
        )
    }
}
