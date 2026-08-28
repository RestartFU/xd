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

ARG BUILD_JOBS=1
ENV GRADLE_OPTS=-Dorg.gradle.workers.max=${BUILD_JOBS}
WORKDIR /src
COPY . .

FROM source AS test

# Android's PathParser reads numbers greedily and cannot split packed SVG arc
# flags, so "0 014.21" becomes the number 14.21 instead of flags 0 and 1
# followed by 4.21, and the shape comes out mangled. SVG optimisers emit that
# form routinely, so an icon pasted straight from one looks fine everywhere
# except here. Nothing in the build catches it: the drawable compiles happily.
RUN if grep -rlP '[aA][\d.\s,\-]*?\s0\s0[01][\d.\-]' \
        androidApp/src/main/res/drawable/ 2>/dev/null; then \
      echo "Those drawables pack SVG arc flags; separate them." >&2; \
      exit 1; \
    fi

RUN --mount=type=cache,id=xd-mobile-gradle,target=/root/.gradle \
    ./gradlew --no-daemon :shared:allTests :androidApp:compileDebugKotlin

FROM source AS verify

RUN --mount=type=cache,id=xd-mobile-gradle,target=/root/.gradle \
    ./gradlew --no-daemon :shared:allTests :androidApp:assembleDebug \
 && "${ANDROID_HOME}/build-tools/34.0.0/apksigner" verify \
      androidApp/build/outputs/apk/debug/androidApp-debug.apk \
 && "${ANDROID_HOME}/build-tools/34.0.0/zipalign" -c 4 \
      androidApp/build/outputs/apk/debug/androidApp-debug.apk

FROM scratch AS apk

COPY --from=verify \
  /src/androidApp/build/outputs/apk/debug/androidApp-debug.apk \
  /xd-mobile-debug.apk
