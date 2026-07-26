# Everything runs in Docker; the host only needs docker itself.

.PHONY: build test run clean

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
