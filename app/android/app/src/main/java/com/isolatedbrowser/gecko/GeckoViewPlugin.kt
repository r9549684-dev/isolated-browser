package com.isolatedbrowser.gecko

import android.content.Context
import android.view.View
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.StandardMessageCodec
import io.flutter.plugin.platform.PlatformView
import io.flutter.plugin.platform.PlatformViewFactory
import mozilla.components.browser.engine.gecko.GeckoEngine
import mozilla.components.concept.engine.DefaultSettings
import mozilla.components.concept.engine.EngineSession
import mozilla.components.concept.engine.EngineView
import mozilla.components.concept.engine.request.RequestInterceptor

/**
 * Фабрика PlatformView — регистрирует GeckoViewWrapper как нативный View.
 * Вызывается из MainActivity при регистрации viewType "com.isolatedbrowser/geckoview".
 */
class GeckoViewPlugin(
    private val messenger: BinaryMessenger
) : PlatformViewFactory(StandardMessageCodec.INSTANCE) {

    override fun create(context: Context, viewId: Int, args: Any?): PlatformView {
        val params = args as Map<*, *>
        val url           = params["url"]             as? String ?: "about:blank"
        val proxyHost     = params["socks_proxy_host"] as? String ?: "127.0.0.1"
        val proxyPort     = (params["socks_proxy_port"] as? Int)   ?: 18080

        return GeckoViewWrapper(context, viewId, url, proxyHost, proxyPort, messenger)
    }
}

/**
 * Обёртка вокруг GeckoEngine/EngineView (Mozilla Android Components).
 *
 * Конфигурация прокси: GeckoEngine принимает proxyConfig через DefaultSettings.
 * Весь трафик GeckoView идёт через SOCKS5 127.0.0.1:proxyPort — наш транспортный туннель.
 */
class GeckoViewWrapper(
    private val context: Context,
    private val viewId: Int,
    private val initialUrl: String,
    private val proxyHost: String,
    private val proxyPort: Int,
    messenger: BinaryMessenger,
) : PlatformView {

    private val channel = MethodChannel(messenger, "com.isolatedbrowser/browser_$viewId")

    // GeckoEngine — Mozilla Android Components поверх GeckoView
    private val engine: GeckoEngine by lazy {
        val settings = DefaultSettings(
            remoteDebuggingEnabled = false,
            testingModeEnabled     = false,
            // Настройка SOCKS5 прокси через GeckoRuntime preferences
            // (устанавливается через GeckoRuntimeSettings.Builder)
        )
        GeckoEngine(context, settings, geckoRuntime(proxyHost, proxyPort))
    }

    private val engineSession: EngineSession by lazy {
        engine.createSession().also { session ->
            session.register(object : EngineSession.Observer {
                override fun onLocationChange(url: String, hasUserGesture: Boolean) {
                    channel.invokeMethod("onPageStarted", url)
                }
                override fun onLoadingStateChange(loading: Boolean) {
                    if (!loading) channel.invokeMethod("onPageFinished", null)
                }
            })
        }
    }

    private val engineView: EngineView by lazy {
        engine.createView(context).also { view ->
            view.render(engineSession)
            engineSession.loadUrl(initialUrl)
        }
    }

    init {
        channel.setMethodCallHandler { call, result ->
            when (call.method) {
                "reload" -> { engineSession.reload(); result.success(null) }
                "goBack" -> { engineSession.goBack(); result.success(null) }
                "loadUrl" -> {
                    val url = call.argument<String>("url") ?: return@setMethodCallHandler
                    engineSession.loadUrl(url)
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    override fun getView(): View = engineView.asView()

    override fun dispose() {
        engineSession.close()
    }

    companion object {
        /**
         * Создаёт GeckoRuntime с SOCKS5-прокси через native GeckoRuntimeSettings.
         * proxyConfig задаётся через preferences напрямую в GeckoRuntime.
         */
        private fun geckoRuntime(
            proxyHost: String,
            proxyPort: Int,
        ): org.mozilla.geckoview.GeckoRuntime {
            val runtimeSettings = org.mozilla.geckoview.GeckoRuntimeSettings.Builder()
                .proxyOverride(
                    org.mozilla.geckoview.GeckoRuntimeSettings.ProxyConfig(
                        "socks",
                        proxyHost,
                        proxyPort,
                    )
                )
                .build()

            return org.mozilla.geckoview.GeckoRuntime.create(
                // Context передаётся через getApplicationContext() из Application
                android.app.Application().applicationContext,
                runtimeSettings,
            )
        }
    }
}
