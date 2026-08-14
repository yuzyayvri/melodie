import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.melodie.companion"
    // Not a preference: media3 1.11.0's AAR metadata declares minCompileSdk=36.
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.melodie.companion"
        minSdk = 24          // PLAN.md §7
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            // R8 is the whole reason this build type exists: Media3 alone is
            // several MB unshrunk.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Debug-signed on purpose: this is a personal sideload, so there
            // is no keystore for the user to keep track of.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // Nothing here needs BuildConfig, view binding, Compose, or resource
    // values — leaving them off keeps the build and the APK smaller.
    buildFeatures {
        buildConfig = false
    }

    packaging {
        resources.excludes += setOf(
            "META-INF/*.version",
            "META-INF/*.kotlin_module",
            "DebugProbesKt.bin",
        )
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    // PLAN.md §1/§7: the researched, non-negotiable way to get background
    // playback, audio focus, and lockscreen controls on Android.
    implementation("androidx.media3:media3-exoplayer:1.11.0")
    implementation("androidx.media3:media3-session:1.11.0")
    // QR pairing. Chosen over ML Kit because ML Kit requires Google Play
    // Services; this works on stock AOSP and depends only on zxing-core.
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")

    testImplementation("junit:junit:4.13.2")
    // Test-only XmlPullParser implementation, so the response parser can be
    // tested on the JVM. Never packaged into the APK.
    testImplementation("net.sf.kxml:kxml2:2.3.0")
}
