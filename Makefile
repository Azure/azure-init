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
