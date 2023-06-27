build-all:
	@echo ""
	@echo "**********************************"
	@echo "* Building the source code"	
	@echo "**********************************"
	@echo ""
	@cargo build --all

RANDOM_NUMBER := $$((1 + RANDOM % 1000))
TEST_USER := "test_user_$(RANDOM_NUMBER)"

e2e-test: build-all
	@./tests/functional_tests.sh 