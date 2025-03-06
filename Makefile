build-all:
	@echo ""
	@echo "**********************************"
	@echo "* Building the source code"	
	@echo "**********************************"
	@echo ""
	@cargo build --all

tests: build-all
	@echo ""
	@echo "**********************************"
	@echo "* Unit testing"
	@echo "**********************************"
	@echo ""
	@cargo test --all --verbose

e2e-test: docker-build-e2e
	@./tests/functional_tests.sh

docker-build-e2e:
	@echo ""
	@echo "**********************************"
	@echo "* Building functional_tests in Docker (Ubuntu 22.04)"
	@echo "**********************************"
	@echo ""
	@docker-compose up -d build-functional-tests
	@docker-compose logs -f build-functional-tests
	@echo "Checking if binary was built successfully..."
	@if [ ! -f "./target/debug/functional_tests" ]; then \
		echo "Error: functional_tests binary not built properly"; \
		exit 1; \
	fi

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
