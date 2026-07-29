# syntax=docker/dockerfile:1

FROM eclipse-temurin:21-jdk-jammy AS android-sdk

ARG ANDROID_COMMAND_LINE_TOOLS=15859902
ARG ANDROID_COMMAND_LINE_TOOLS_SHA256=4e4c464f145a7512b57d088ac6c278c03c9eea610886b35a5e0804e74eedf583

ENV ANDROID_HOME=/opt/android-sdk
ENV PATH=/opt/android-sdk/cmdline-tools/latest/bin:/opt/android-sdk/platform-tools:${PATH}

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl unzip \
 && rm -rf /var/lib/apt/lists/*

RUN curl --fail --location --retry 3 \
      "https://dl.google.com/android/repository/commandlinetools-linux-${ANDROID_COMMAND_LINE_TOOLS}_latest.zip" \
      --output /tmp/android-command-line-tools.zip \
 && echo "${ANDROID_COMMAND_LINE_TOOLS_SHA256}  /tmp/android-command-line-tools.zip" \
      | sha256sum --check - \
 && mkdir -p "${ANDROID_HOME}/cmdline-tools/latest" \
 && unzip -q /tmp/android-command-line-tools.zip -d /tmp/android-command-line-tools \
 && mv /tmp/android-command-line-tools/cmdline-tools/* \
      "${ANDROID_HOME}/cmdline-tools/latest/" \
 && rm -rf /tmp/android-command-line-tools /tmp/android-command-line-tools.zip \
 && yes | sdkmanager --licenses >/dev/null \
 && sdkmanager \
      "build-tools;34.0.0" \
      "platform-tools" \
      "platforms;android-35"

FROM android-sdk AS source

WORKDIR /src
COPY . .

FROM source AS test

RUN --mount=type=cache,id=xd-mobile-gradle,target=/root/.gradle \
    ./gradlew --no-daemon :shared:allTests

FROM source AS verify

RUN --mount=type=cache,id=xd-mobile-gradle,target=/root/.gradle \
    ./gradlew --no-daemon :shared:allTests :androidApp:assembleDebug \
 && "${ANDROID_HOME}/build-tools/34.0.0/apksigner" verify \
      androidApp/build/outputs/apk/debug/androidApp-debug.apk \
 && "${ANDROID_HOME}/build-tools/34.0.0/zipalign" -c 4 \
      androidApp/build/outputs/apk/debug/androidApp-debug.apk

FROM source AS nightly

ARG ANDROID_VERSION_CODE=1

RUN --mount=type=cache,id=xd-mobile-gradle,target=/root/.gradle \
    --mount=type=secret,id=android_keystore,target=/run/secrets/android-keystore.p12 \
    --mount=type=secret,id=android_keystore_password \
    --mount=type=secret,id=android_key_alias \
    --mount=type=secret,id=android_key_password \
    export XD_ANDROID_KEYSTORE=/run/secrets/android-keystore.p12 \
           XD_ANDROID_VERSION_CODE="${ANDROID_VERSION_CODE}" \
           XD_ANDROID_KEYSTORE_PASSWORD="$(cat /run/secrets/android_keystore_password)" \
           XD_ANDROID_KEY_ALIAS="$(cat /run/secrets/android_key_alias)" \
           XD_ANDROID_KEY_PASSWORD="$(cat /run/secrets/android_key_password)" \
 && ./gradlew --no-daemon :shared:allTests :androidApp:assembleNightly \
 && "${ANDROID_HOME}/build-tools/34.0.0/apksigner" verify --verbose \
      androidApp/build/outputs/apk/nightly/androidApp-nightly.apk \
 && "${ANDROID_HOME}/build-tools/34.0.0/zipalign" -c 4 \
      androidApp/build/outputs/apk/nightly/androidApp-nightly.apk

FROM scratch AS apk

COPY --from=verify \
  /src/androidApp/build/outputs/apk/debug/androidApp-debug.apk \
  /xd-mobile-debug.apk

FROM scratch AS nightly-apk

COPY --from=nightly \
  /src/androidApp/build/outputs/apk/nightly/androidApp-nightly.apk \
  /xd-nightly-android.apk
