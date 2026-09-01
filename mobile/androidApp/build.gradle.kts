import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

val xdChannel = providers.gradleProperty("xd.channel").orElse("release").get()
val xdRemoteDataName = when (xdChannel) {
    "release" -> "xd"
    "nightly" -> "xd-nightly"
    "dev" -> "xd-dev"
    else -> error("xd.channel must be release, nightly, or dev")
}
val xdApplicationId = when (xdChannel) {
    "release" -> "com.restartfu.xd.app"
    "nightly" -> "com.restartfu.xd.app.nightly"
    "dev" -> "com.restartfu.xd.app.dev"
    else -> error("xd.channel must be release, nightly, or dev")
}
val xdDisplayName = when (xdChannel) {
    "release" -> "xd"
    "nightly" -> "xd nightly"
    "dev" -> "xd dev"
    else -> error("xd.channel must be release, nightly, or dev")
}

android {
    namespace = "com.restartfu.xd.mobile"
    compileSdk = 35

    defaultConfig {
        // Protected signing starts with a new package identity. APKs published
        // before this change used a public debug key and cannot safely rotate
        // in place; a distinct ID makes the migration an explicit side-by-side
        // install instead of an opaque Android signature-conflict failure.
        applicationId = xdApplicationId
        minSdk = 26
        targetSdk = 35
        versionCode = 1010
        versionName = "0.1.10"
        resValue("string", "app_name", xdDisplayName)
        resValue("string", "remote_data_name", xdRemoteDataName)
    }

    val releaseRequested = gradle.startParameter.taskNames.any {
        it.contains("Release", ignoreCase = true)
    }
    val releaseKeystore = System.getenv("XD_ANDROID_KEYSTORE")
    val releaseStorePassword = System.getenv("XD_ANDROID_STORE_PASSWORD")
    val releaseKeyAlias = System.getenv("XD_ANDROID_KEY_ALIAS")
    val releaseKeyPassword = System.getenv("XD_ANDROID_KEY_PASSWORD")
    if (releaseRequested) {
        require(!releaseKeystore.isNullOrBlank()) { "XD_ANDROID_KEYSTORE is required" }
        require(!releaseStorePassword.isNullOrEmpty()) { "XD_ANDROID_STORE_PASSWORD is required" }
        require(!releaseKeyAlias.isNullOrBlank()) { "XD_ANDROID_KEY_ALIAS is required" }
        require(!releaseKeyPassword.isNullOrEmpty()) { "XD_ANDROID_KEY_PASSWORD is required" }
    }
    val releaseSigning = signingConfigs.create("release") {
        releaseKeystore?.let { storeFile = file(it) }
        storePassword = releaseStorePassword
        keyAlias = releaseKeyAlias
        keyPassword = releaseKeyPassword
        storeType = "PKCS12"
    }

    buildTypes {
        getByName("debug") {
            // A locally signed APK must never claim the protected stable ID,
            // or installing it would block the real release (and vice versa).
            applicationIdSuffix = ".debug"
            resValue("string", "app_name", "xd dev")
        }
        getByName("release") {
            signingConfig = releaseSigning
            isDebuggable = false
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
