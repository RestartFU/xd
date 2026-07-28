# Mobile development

The mobile client is a separate Gradle project under `mobile/`. It is a client
of `xd serve`, not a port of the GTK application or the agent subprocesses.

Phase 0 contains the shared Kotlin Multiplatform module, Android application
shell, and build wiring. The shared module targets Android and the JVM. Native
iOS targets arrive with the iOS client phase.

## Requirement

- Docker with BuildKit

The checked-in wrapper, JDK 21, and Android SDK 35 run inside the build image.
No host Gradle, JDK, or Android SDK is required. Android Studio may still
create `mobile/local.properties` for IDE use; that file is ignored.

## Commands

From the repository root:

```sh
make mobile-test
make mobile-android
```

The test command builds the Docker `test` stage. The Android command exports
the debug APK to `dist/mobile/xd-mobile-debug.apk`.

For IDE use, the Gradle wrapper remains available from `mobile/`:

```sh
./gradlew :shared:allTests
./gradlew :androidApp:assembleDebug
```

Direct wrapper use requires a local JDK 21 and Android SDK 35.
