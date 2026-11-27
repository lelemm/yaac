plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// A value from the environment, else a Gradle property (`-P` or ~/.gradle/gradle.properties).
fun setting(env: String, property: String): String? =
    (System.getenv(env) ?: providers.gradleProperty(property).orNull)?.trim()?.takeIf { it.isNotEmpty() }

// The build id (D135): releases pass `YAAC_BUILD_ID` (1000 + the workflow's run number), the same
// number the controller in `libyaac_android.so` was built with. Local builds stay at 1 / "+dev",
// below every release, so a release always updates a local build's versionCode.
val yaacVersion = file("../../Cargo.toml").readLines()
    .dropWhile { it.trim() != "[workspace.package]" }.drop(1)
    .takeWhile { !it.trim().startsWith("[") }
    .firstNotNullOfOrNull { Regex("""^version\s*=\s*"([^"]+)"""").find(it.trim())?.groupValues?.get(1) }
    ?: "0.0.0"
val yaacBuild = setting("YAAC_BUILD_ID", "yaac.buildId")?.takeIf { it.all(Char::isDigit) }?.toInt()

// The release key (docs/install.md#android): only when all four values are given, so a local
// `assembleRelease` without them is simply unsigned and debug builds never change.
val releaseKeystore = setting("YAAC_ANDROID_KEYSTORE", "yaac.android.keystore")
val releaseKeystorePassword = setting("YAAC_ANDROID_KEYSTORE_PASSWORD", "yaac.android.keystorePassword")
val releaseKeyAlias = setting("YAAC_ANDROID_KEY_ALIAS", "yaac.android.keyAlias")
val releaseKeyPassword = setting("YAAC_ANDROID_KEY_PASSWORD", "yaac.android.keyPassword")
val releaseSigned = listOf(releaseKeystore, releaseKeystorePassword, releaseKeyAlias, releaseKeyPassword).all { it != null }

android {
    namespace = "dev.yaac.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.yaac.android"
        minSdk = 29
        targetSdk = 36
        versionCode = yaacBuild ?: 1
        versionName = "$yaacVersion+${yaacBuild ?: "dev"}"
        // The dev transport: a desktop-built controller bridged to TCP, reached through
        // `adb reverse tcp:47100 tcp:47100`. Used only when the native library is absent.
        buildConfigField("String", "DEV_HOST", "\"127.0.0.1\"")
        buildConfigField("int", "DEV_PORT", "47100")
    }

    signingConfigs {
        if (releaseSigned) create("release") {
            storeFile = file(releaseKeystore!!)
            storePassword = releaseKeystorePassword
            keyAlias = releaseKeyAlias
            keyPassword = releaseKeyPassword
        }
    }

    buildTypes {
        // Phones are arm64; x86_64 is for the emulator only, so release APKs leave it out
        // (its WebRTC library alone is 16 MB).
        debug { ndk { abiFilters += listOf("arm64-v8a", "x86_64") } }
        release {
            ndk { abiFilters += listOf("arm64-v8a") }
            // No R8: the JNI entry points and the serialized frames are reached by name, and the
            // app is small enough that shrinking buys little against that risk.
            isMinifyEnabled = false
            if (releaseSigned) signingConfig = signingConfigs.getByName("release")
        }
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
    // A voice call's WebRTC peer (core/CallPeer.kt): upstream libwebrtc M150 for Android, built
    // and published by the webrtc-sdk project (BSD-3, no transitive dependencies). Adds
    // libjingle_peerconnection_so.so: ~12 MB for arm64-v8a, ~16 MB for x86_64 (the ABIs kept).
    implementation("io.github.webrtc-sdk:android:150.7871.09")
    debugImplementation("androidx.compose.ui:ui-tooling:$compose")
    testImplementation("junit:junit:4.13.2")
}
