BUILD_MODE ?= debug
BUILD_FLAG := $(if $(filter release,$(BUILD_MODE)),--release,)

build-all:
	@echo ""
	@echo "**********************************"
	@echo "* Building the source code"	
	@echo "**********************************"
	@echo ""
	@cargo build --all $(BUILD_FLAG)

tests: build-all
	@echo ""
	@echo "**********************************"
	@echo "* Unit testing"
	@echo "**********************************"
	@echo ""
	@cargo test --all --verbose

e2e-test: build-all
	@./tests/functional_tests.sh

fmt:
	@echo ""
	@echo "**********************************"
	@echo "* Formatting"
	@echo "**********************************"
	@echo ""
	@cargo fmt --all --check

clippy:
	@echo ""
	@echo "**********************************"
	@echo "* Linting with clippy"
	@echo "**********************************"
	@echo ""
	@cargo clippy --verbose -- --deny warnings


install: build-all
	@echo ""
	@echo "**********************************"
	@echo "* Installing binaries"
	@echo "**********************************"
	@echo ""
	@/bin/install -d $(DESTDIR)/usr/bin
	@/bin/install -m 0755 target/$(BUILD_MODE)/azure-init $(DESTDIR)/usr/bin/

	@echo ""
	@echo "**********************************"
	@echo "* Installing systemd service file"
	@echo "**********************************"
	@echo ""
	@/bin/install -d $(DESTDIR)/usr/lib/systemd/system
	@/bin/install -m 0644 config/azure-init.service $(DESTDIR)/usr/lib/systemd/system/azure-init.service
