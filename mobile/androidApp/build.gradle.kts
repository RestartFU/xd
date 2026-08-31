import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.restartfu.xd.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.restartfu.xd.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1010
        versionName = "0.1.10"
        resValue("string", "app_name", "xd")
    }

    // A fixed key, checked in on purpose.
    //
    // Gradle otherwise generates one per machine, and a CI runner starts
    // empty, so every published APK would be signed differently and Android
    // would refuse to update an installed build: "package conflicts with an
    // existing package". Signing every build with the same key makes updates
    // work across nightly and stable release assets.
    //
    // This is not a secret and must never be treated as one. It is the same
    // idea as Android's own debug keystore, whose password is public: it
    // provides update continuity, not proof of who built the APK.
    signingConfigs {
        getByName("debug") {
            storeFile = file("debug.p12")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources.excludes += "META-INF/versions/**/OSGI-INF/MANIFEST.MF"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_21)
    }
}

dependencies {
    implementation(project(":shared"))
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.navigation.compose)
    debugImplementation(libs.androidx.compose.ui.tooling)
}
