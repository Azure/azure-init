build-all:
	@echo ""
	@echo "**********************************"
	@echo "* Building the source code"	
	@echo "**********************************"
	@echo ""
	@cargo build --all

e2e-test: build-all
	@./tests/functional_tests.sh 