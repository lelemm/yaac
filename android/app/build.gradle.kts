plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.yaac.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.yaac.android"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // The dev transport: a desktop-built controller bridged to TCP, reached through
        // `adb reverse tcp:47100 tcp:47100`. Used only when the native library is absent.
        buildConfigField("String", "DEV_HOST", "\"127.0.0.1\"")
        buildConfigField("int", "DEV_PORT", "47100")
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs { useLegacyPackaging = false }
    }
}


composeCompiler {
    stabilityConfigurationFiles.add(project.layout.projectDirectory.file("compose-stability.conf"))
}

dependencies {
    val compose = "1.10.0"
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.compose.ui:ui:$compose")
    implementation("androidx.compose.foundation:foundation:$compose")
    implementation("androidx.compose.material3:material3:1.4.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.1")
    // The Android browser host (P380, A2b BrowserHost.kt): ProxyController, document-start scripts.
    implementation("androidx.webkit:webkit:1.14.0")
    debugImplementation("androidx.compose.ui:ui-tooling:$compose")
    testImplementation("junit:junit:4.13.2")
}
