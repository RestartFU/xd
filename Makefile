# Everything runs in Docker; the host only needs docker itself.

.PHONY: build test run clean mobile-test mobile-android

## build: compile in Docker and export the runnable bundle to ./dist
build:
	@./scripts/build.sh

## test: run the headless test suite in Docker
test:
	@./scripts/test.sh

## run: rebuild (cached, so cheap when nothing changed) and launch
run: build
	@./dist/xd.sh

dist/xd.sh:
	@./scripts/build.sh

clean:
	@rm -rf dist

## mobile-test: run all shared Kotlin tests
mobile-test:
	@./scripts/mobile-test.sh

## mobile-android: build the Android debug APK
mobile-android:
	@./scripts/mobile-build.sh
