# Everything runs in Docker; the host only needs docker itself.

.PHONY: build test run clean

## build: compile in Docker and export the runnable bundle to ./dist
build:
	@./scripts/build.sh

## test: run the headless test suite in Docker
test:
	@./scripts/test.sh

## run: build if needed, then launch on the host
run: dist/hy.sh
	@./dist/hy.sh

dist/hy.sh:
	@./scripts/build.sh

clean:
	@rm -rf dist
