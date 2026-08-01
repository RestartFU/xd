import org.jetbrains.kotlin.gradle.dsl.JvmTarget

val nightlyKeystorePath = providers.environmentVariable("XD_ANDROID_KEYSTORE").orNull
val nightlyKeystorePassword =
    providers.environmentVariable("XD_ANDROID_KEYSTORE_PASSWORD").orNull
val nightlyKeyAlias = providers.environmentVariable("XD_ANDROID_KEY_ALIAS").orNull
val nightlyKeyPassword = providers.environmentVariable("XD_ANDROID_KEY_PASSWORD").orNull
val androidVersionCode = providers.environmentVariable("XD_ANDROID_VERSION_CODE")
    .orNull
    ?.toIntOrNull()
    ?: 1

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
        versionCode = androidVersionCode
        versionName = "0.1.0"
        resValue("string", "app_name", "xd")
    }

    signingConfigs {
        if (
            nightlyKeystorePath != null &&
            nightlyKeystorePassword != null &&
            nightlyKeyAlias != null &&
            nightlyKeyPassword != null
        ) {
            create("nightly") {
                storeFile = file(nightlyKeystorePath)
                storePassword = nightlyKeystorePassword
                keyAlias = nightlyKeyAlias
                keyPassword = nightlyKeyPassword
            }
        }
    }

    buildTypes {
        create("nightly") {
            initWith(getByName("release"))
            applicationIdSuffix = ".nightly"
            versionNameSuffix = "-nightly"
            isDebuggable = false
            matchingFallbacks += listOf("release")
            resValue("string", "app_name", "xd Nightly")
            signingConfig = signingConfigs.findByName("nightly")
        }
    }

    buildFeatures {
        compose = true
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
